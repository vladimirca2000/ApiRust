//! Adaptador SQLx da porta `CatalogHistoryRepository`.
//!
//! As operações de escrita abrem uma transação que envolve **produto e
//! histórico**. O motivo é simples: se o `UPDATE` do produto confirmar e o
//! `INSERT` do histórico falhar, o saldo muda sem nenhuma linha explicando a
//! mudança — exatamente o ponto cego que esta fase existe para fechar.
//!
//! As leituras montam o histórico reconstruindo os tipos de domínio, então uma
//! linha corrompida vira erro de infraestrutura em vez de dado silenciosamente
//! errado.
//!
//! A escrita usa **controle otimista**: o `UPDATE` do produto só confirma se o
//! saldo e o preço ainda são os lidos por quem calculou o delta. Sem isso, duas
//! requisições concorrentes partiriam da mesma leitura, gravariam saldos
//! absolutos diferentes e registrariam movimentos que não somam o saldo final.

use application::audit::AuditEntry;
use application::{AppError, CatalogHistoryRepository, ProductChange, format_timestamp};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use domain::{PriceChange, Product, StockMovement, StockReason, UserId};
use sqlx::PgPool;

/// Colunas do histórico de estoque.
const MOVEMENT_COLUMNS: &str = "product_id, delta, reason, actor_id, at";

/// Colunas do histórico de preço.
const PRICE_COLUMNS: &str = "product_id, old_cents, new_cents, actor_id, at";

/// Repositório de histórico do catálogo sobre SQLite.
#[derive(Debug, Clone)]
pub struct SqlxCatalogHistoryRepository {
    /// Pool usado em todas as operações.
    pool: PgPool,
}

impl SqlxCatalogHistoryRepository {
    /// Monta o repositório sobre um pool já configurado.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl CatalogHistoryRepository for SqlxCatalogHistoryRepository {
    /// Insere o produto, o movimento de estoque inicial e a auditoria em uma transação;
    /// retorna sucesso somente se todas as gravações forem confirmadas.
    async fn insert_product(
        &self,
        product: &Product,
        movement: Option<&StockMovement>,
        audit: Option<&AuditEntry>,
    ) -> Result<(), AppError> {
        let mut transaction = self.pool.begin().await.map_err(AppError::infrastructure)?;

        let insert_product = "INSERT INTO products \
                              (id, name, sku, description, price_cents, stock, category_id, \
                               status, created_at, updated_at, deleted_at) \
                              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)";

        sqlx::query(insert_product)
            .bind(product.id().to_string())
            .bind(product.name())
            .bind(product.sku().as_str())
            .bind(product.description())
            .bind(product.price().cents())
            .bind(product.stock())
            .bind(product.category_id().to_string())
            .bind(product.status().as_str())
            .bind(format_timestamp(product.created_at()))
            .bind(format_timestamp(product.updated_at()))
            .bind(product.deleted_at().map(format_timestamp))
            .execute(&mut *transaction)
            .await
            .map_err(map_product_error)?;

        if let Some(movement) = movement {
            insert_movement(&mut transaction, movement).await?;
        }
        if let Some(entry) = audit {
            crate::audit_tx::insert_audit(&mut transaction, entry).await?;
        }

        transaction.commit().await.map_err(AppError::infrastructure)
    }
    /// Atualiza o produto e grava as mudanças de estoque, preço e auditoria na mesma
    /// transação; retorna sucesso ou erro se a gravação não for válida.
    async fn update_product(
        &self,
        product: &Product,
        change: &ProductChange<'_>,
    ) -> Result<(), AppError> {
        let mut transaction = self.pool.begin().await.map_err(AppError::infrastructure)?;

        // Controle otimista: o `UPDATE` só vale se saldo e preço ainda forem os
        // lidos. Sem a condição, duas requisições concorrentes gravariam valores
        // absolutos diferentes e registraram deltas sobre a mesma leitura.
        let (expected_stock, expected_price) = application::expected_values(product, change);

        let update_product = "UPDATE products SET name = $1, sku = $2, description = $3, \
                              price_cents = $4, stock = $5, category_id = $6, status = $7, \
                              updated_at = $8, deleted_at = $9 \
                              WHERE id = $10 AND stock = $11 AND price_cents = $12";

        let executed = sqlx::query(update_product)
            .bind(product.name())
            .bind(product.sku().as_str())
            .bind(product.description())
            .bind(product.price().cents())
            .bind(product.stock())
            .bind(product.category_id().to_string())
            .bind(product.status().as_str())
            .bind(format_timestamp(product.updated_at()))
            .bind(product.deleted_at().map(format_timestamp))
            .bind(product.id().to_string())
            .bind(expected_stock)
            .bind(expected_price)
            .execute(&mut *transaction)
            .await
            .map_err(map_product_error)?;

        if executed.rows_affected() == 0 {
            // Distingue "não existe" (404) de "alguém alterou primeiro" (409):
            // no segundo caso o cliente pode reler e repetir.
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM products WHERE id = $1)")
                    .bind(product.id().to_string())
                    .fetch_one(&mut *transaction)
                    .await
                    .map_err(AppError::infrastructure)?;

            return if exists {
                Err(concurrent_update())
            } else {
                Err(AppError::NotFound)
            };
        }

        if let Some(movement) = change.movement {
            insert_movement(&mut transaction, movement).await?;
        }
        if let Some(price) = change.price {
            insert_price_change(&mut transaction, price).await?;
        }
        if let Some(entry) = change.audit {
            crate::audit_tx::insert_audit(&mut transaction, entry).await?;
        }

        transaction.commit().await.map_err(AppError::infrastructure)
    }
    /// Lista as movimentações de estoque do produto em ordem temporal e retorna a página ou erro de persistência.
    async fn stock_movements(
        &self,
        product_id: UserId,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<StockMovement>, AppError> {
        let sql = format!(
            "SELECT {MOVEMENT_COLUMNS} FROM stock_movements \
             WHERE product_id = $1 ORDER BY at DESC, id DESC LIMIT $2 OFFSET $3"
        );

        let rows = sqlx::query_as::<_, StockMovementRow>(&sql)
            .bind(product_id.to_string())
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        rows.into_iter()
            .map(StockMovementRow::into_movement)
            .collect()
    }
    /// Lista as alterações de preço do produto em ordem temporal e retorna a página ou erro de persistência.
    async fn price_changes(
        &self,
        product_id: UserId,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<PriceChange>, AppError> {
        let sql = format!(
            "SELECT {PRICE_COLUMNS} FROM price_changes \
             WHERE product_id = $1 ORDER BY at DESC, id DESC LIMIT $2 OFFSET $3"
        );

        let rows = sqlx::query_as::<_, PriceChangeRow>(&sql)
            .bind(product_id.to_string())
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        rows.into_iter().map(PriceChangeRow::into_change).collect()
    }
    /// Conta as movimentações de estoque do produto e retorna o total ou erro de persistência.
    async fn count_stock_movements(&self, product_id: UserId) -> Result<i64, AppError> {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM stock_movements WHERE product_id = $1")
            .bind(product_id.to_string())
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::infrastructure)
    }
    /// Conta as alterações de preço do produto e retorna o total ou erro de persistência.
    async fn count_price_changes(&self, product_id: UserId) -> Result<i64, AppError> {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM price_changes WHERE product_id = $1")
            .bind(product_id.to_string())
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::infrastructure)
    }
}

/// Grava uma movimentação de estoque dentro da transação corrente.
///
/// # Errors
///
/// Retorna [`AppError::Infrastructure`] se a inserção falhar.
async fn insert_movement(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    movement: &StockMovement,
) -> Result<(), AppError> {
    let sql = "INSERT INTO stock_movements (id, product_id, delta, reason, actor_id, at) \
               VALUES ($1, $2, $3, $4, $5, $6)";

    sqlx::query(sql)
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(movement.product_id.to_string())
        .bind(movement.delta)
        .bind(movement.reason.as_str())
        .bind(movement.actor_id.map(|id| id.to_string()))
        .bind(format_timestamp(movement.at))
        .execute(&mut **transaction)
        .await
        .map_err(AppError::infrastructure)?;

    Ok(())
}

/// Grava uma alteração de preço dentro da transação corrente.
///
/// # Errors
///
/// Retorna [`AppError::Infrastructure`] se a inserção falhar.
async fn insert_price_change(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    change: &PriceChange,
) -> Result<(), AppError> {
    let sql = "INSERT INTO price_changes \
               (id, product_id, old_cents, new_cents, actor_id, at) \
               VALUES ($1, $2, $3, $4, $5, $6)";

    sqlx::query(sql)
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(change.product_id.to_string())
        .bind(change.old_cents)
        .bind(change.new_cents)
        .bind(change.actor_id.map(|id| id.to_string()))
        .bind(format_timestamp(change.at))
        .execute(&mut **transaction)
        .await
        .map_err(AppError::infrastructure)?;

    Ok(())
}

/// Erro devolvido quando a leitura usada no cálculo ficou obsoleta.
///
/// É `409` e não `500`: a operação é válida, mas perdeu a corrida para outra
/// escrita. O cliente resolve relendo o produto e repetindo o pedido.
fn concurrent_update() -> AppError {
    AppError::Conflict("produto alterado por outra requisição; leia novamente e repita".to_owned())
}

/// Traduz erros de escrita de produto para o vocabulário da aplicação.
///
/// SKU duplicado e categoria inexistente chegariam como falha de constraint e
/// virariam `500`; aqui viram `409` e `404`, que é o que o cliente pode
/// resolver. A identificação usa `SQLSTATE` e o nome da constraint, nunca o
/// texto da mensagem.
fn map_product_error(error: sqlx::Error) -> AppError {
    if crate::pg_errors::is_unique_violation_on(&error, "products_sku_active_unique") {
        return AppError::AlreadyInUse("SKU".to_owned());
    }
    if crate::pg_errors::is_foreign_key_violation(&error) {
        return AppError::CategoryNotFound;
    }

    AppError::infrastructure(error)
}

/// Linha crua de `stock_movements`.
#[derive(sqlx::FromRow)]
struct StockMovementRow {
    /// UUID do produto.
    product_id: String,
    /// Variação aplicada ao saldo.
    delta: i64,
    /// Motivo persistido.
    reason: String,
    /// Autor da operação, quando houve.
    actor_id: Option<String>,
    /// Instante em RFC 3339.
    at: String,
}

impl StockMovementRow {
    /// Converte a linha para o tipo de domínio, validando todos os campos.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Domain`] para UUID/motivo inválido e
    /// [`AppError::Infrastructure`] para data ou `delta` corrompidos.
    fn into_movement(self) -> Result<StockMovement, AppError> {
        let product_id = UserId::parse(&self.product_id)?;
        let reason = StockReason::parse(&self.reason)?;
        let actor_id = self.actor_id.as_deref().map(UserId::parse).transpose()?;
        let at = parse_timestamp(&self.at)?;

        Ok(StockMovement::new(
            product_id, self.delta, reason, actor_id, at,
        )?)
    }
}

/// Linha crua de `price_changes`.
#[derive(sqlx::FromRow)]
struct PriceChangeRow {
    /// UUID do produto.
    product_id: String,
    /// Preço anterior em centavos.
    old_cents: i64,
    /// Preço novo em centavos.
    new_cents: i64,
    /// Autor da alteração, quando houve.
    actor_id: Option<String>,
    /// Instante em RFC 3339.
    at: String,
}

impl PriceChangeRow {
    /// Converte a linha para o tipo de domínio.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Domain`] para UUID inválido ou preços iguais e
    /// [`AppError::Infrastructure`] para data corrompida.
    fn into_change(self) -> Result<PriceChange, AppError> {
        let product_id = UserId::parse(&self.product_id)?;
        let actor_id = self.actor_id.as_deref().map(UserId::parse).transpose()?;
        let at = parse_timestamp(&self.at)?;

        Ok(PriceChange::new(
            product_id,
            self.old_cents,
            self.new_cents,
            actor_id,
            at,
        )?)
    }
}

/// Converte o texto RFC 3339 gravado no banco de volta para `DateTime<Utc>`.
///
/// # Errors
///
/// Retorna [`AppError::Infrastructure`] quando o texto não é uma data válida.
fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|error| AppError::infrastructure(std::io::Error::other(error.to_string())))
}
