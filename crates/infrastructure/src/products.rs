//! Adaptador SQLx da porta `ProductRepository`.
//!
//! Os filtros são montados com **placeholders** e só os valores são vinculados;
//! o fragmento `ORDER BY` vem do enum `ProductSort`, nunca do texto do
//! cliente. É essa combinação que fecha SQL injection nesta listagem.

use application::{AppError, ProductFilter, ProductRepository, ProductSort};
use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use domain::{EntityStatus, Money, Product, Sku, UserId};
use sqlx::PgPool;

/// Colunas selecionadas em todas as consultas de produto.
const COLUMNS: &str = "id, name, sku, description, price_cents, stock, category_id, status, \
                       created_at, updated_at, deleted_at";

/// Repositório de produtos sobre SQLite.
#[derive(Debug, Clone)]
pub struct SqlxProductRepository {
    /// Pool usado em todas as operações.
    pool: PgPool,
}

impl SqlxProductRepository {
    /// Monta o repositório sobre um pool já configurado.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Monta a cláusula `WHERE` e os valores correspondentes aos filtros.
    ///
    /// Devolve a condição já com `$n` para cada filtro ativo, na mesma ordem dos
    /// valores. Nunca interpola dado do cliente no SQL.
    ///
    /// A numeração começa em `$start` porque o `LIMIT`/`OFFSET` da consulta usa
    /// os números seguintes. No PostgreSQL, placeholder fora de ordem (ou
    /// repetido) é erro de execução, então a contagem precisa acompanhar
    /// exatamente os `bind` feitos pelo chamador.
    fn build_where(filter: &ProductFilter, start: usize) -> (String, Vec<String>) {
        let mut conditions = vec!["deleted_at IS NULL".to_owned()];
        let mut values = Vec::new();

        // Próximo número de placeholder disponível.
        let mut next = start;

        if let Some(category_id) = filter.category_id {
            conditions.push(format!("category_id = ${next}"));
            values.push(category_id.to_string());
            next += 1;
        }
        if let Some(name) = &filter.name_contains {
            // `LOWER(...) LIKE LOWER($n)` dá busca case-insensitive sem depender
            // de collation configurada no banco.
            conditions.push(format!("LOWER(name) LIKE LOWER(${next})"));
            values.push(format!("%{}%", escape_like(name)));
        }
        if let Some(only_active) = filter.only_active {
            if only_active {
                conditions.push("status = 'active'".to_owned());
            }
        }

        (conditions.join(" AND "), values)
    }
}

#[async_trait]
impl ProductRepository for SqlxProductRepository {
    /// Busca o registro pelo identificador e retorna `Some` quando encontrado, `None` quando ausente ou erro de persistência.
    async fn find_by_id(&self, id: UserId) -> Result<Option<Product>, AppError> {
        let sql = format!("SELECT {COLUMNS} FROM products WHERE id = $1 AND deleted_at IS NULL");

        let row = sqlx::query_as::<_, ProductRow>(&sql)
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        row.map(ProductRow::into_product).transpose()
    }
    /// Busca o produto pelo SKU e retorna `Some` quando encontrado, `None` quando ausente ou erro de persistência.
    async fn find_by_sku(&self, sku: &Sku) -> Result<Option<Product>, AppError> {
        let sql = format!("SELECT {COLUMNS} FROM products WHERE sku = $1 AND deleted_at IS NULL");

        let row = sqlx::query_as::<_, ProductRow>(&sql)
            .bind(sku.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        row.map(ProductRow::into_product).transpose()
    }
    /// Persiste o novo registro e retorna sucesso após a gravação; violações de unicidade, integridade ou falhas SQL são convertidas em erro da aplicação.
    async fn insert(&self, product: &Product) -> Result<(), AppError> {
        let sql = "INSERT INTO products \
                   (id, name, sku, description, price_cents, stock, category_id, status, \
                    created_at, updated_at, deleted_at) \
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)";

        let result = sqlx::query(sql)
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
            .execute(&self.pool)
            .await;

        result.map(|_| ()).map_err(map_product_write_error)
    }
    /// Atualiza o registro persistido e retorna sucesso após a gravação; ausência, conflito de versão ou falha SQL são convertidos em erro da aplicação.
    async fn update(&self, product: &Product) -> Result<(), AppError> {
        let sql = "UPDATE products SET name = $1, sku = $2, description = $3, price_cents = $4, \
                   stock = $5, category_id = $6, status = $7, updated_at = $8, deleted_at = $9 \
                   WHERE id = $10";

        let result = sqlx::query(sql)
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
            .execute(&self.pool)
            .await;

        let executed = result.map_err(map_product_write_error)?;
        if executed.rows_affected() == 0 {
            return Err(AppError::NotFound);
        }
        Ok(())
    }
    /// Consulta uma página de registros aplicando offset e limite e retorna os itens ordenados ou erro de persistência.
    async fn list(
        &self,
        filter: &ProductFilter,
        sort: ProductSort,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<Product>, AppError> {
        // Os filtros ocupam `$1..$n`; `LIMIT`/`OFFSET` continuam a numeração a
        // partir de onde os filtros pararam.
        let (condition, values) = Self::build_where(filter, 1);
        let limit_index = values.len() + 1;
        let offset_index = values.len() + 2;

        // `sort.order_by()` é uma constante do enum: nenhum texto do cliente
        // entra no SQL.
        let sql = format!(
            "SELECT {COLUMNS} FROM products WHERE {condition} ORDER BY {} \
             LIMIT ${limit_index} OFFSET ${offset_index}",
            sort.order_by()
        );

        let mut query = sqlx::query_as::<_, ProductRow>(&sql);
        for value in values {
            query = query.bind(value);
        }
        query = query.bind(limit).bind(offset);

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        rows.into_iter().map(ProductRow::into_product).collect()
    }
    /// Conta os registros considerados ativos pelo repositório e retorna o total ou erro de persistência.
    async fn count(&self, filter: &ProductFilter) -> Result<i64, AppError> {
        let (condition, values) = Self::build_where(filter, 1);
        let sql = format!("SELECT count(*) FROM products WHERE {condition}");

        let mut query = sqlx::query_scalar::<_, i64>(&sql);
        for value in values {
            query = query.bind(value);
        }

        query
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::infrastructure)
    }
}

/// Linha crua da tabela `products`.
#[derive(sqlx::FromRow)]
struct ProductRow {
    /// UUID em texto.
    id: String,
    /// Nome persistido.
    name: String,
    /// SKU persistido.
    sku: String,
    /// Descrição opcional.
    description: Option<String>,
    /// Preço em centavos.
    price_cents: i64,
    /// Estoque.
    stock: i64,
    /// Categoria vinculada.
    category_id: String,
    /// Estado persistido.
    status: String,
    /// Criação em RFC 3339.
    created_at: String,
    /// Última atualização em RFC 3339.
    updated_at: String,
    /// Soft delete em RFC 3339, quando houver.
    deleted_at: Option<String>,
}

impl ProductRow {
    /// Converte a linha para a entidade, validando todos os campos.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Domain`] para dado inválido e
    /// [`AppError::Infrastructure`] para data corrompida.
    fn into_product(self) -> Result<Product, AppError> {
        Ok(Product::restore(
            UserId::parse(&self.id)?,
            self.name,
            Sku::parse(self.sku)?,
            self.description,
            Money::from_cents(self.price_cents)?,
            self.stock,
            UserId::parse(&self.category_id)?,
            EntityStatus::parse(&self.status)?,
            parse_timestamp("created_at", &self.created_at)?,
            parse_timestamp("updated_at", &self.updated_at)?,
            self.deleted_at
                .as_deref()
                .map(|value| parse_timestamp("deleted_at", value))
                .transpose()?,
        )?)
    }
}

/// Escapa os curingas do `LIKE` para que sejam tratados como texto literal.
///
/// Sem isso, um cliente buscando `100%` casaria qualquer nome começando com
/// `100`, o que não é o que ele pediu.
fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Serializa um instante em RFC 3339 UTC com microssegundos.
fn format_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Converte uma coluna de data em `DateTime<Utc>`.
///
/// # Errors
///
/// Retorna [`AppError::Infrastructure`] quando o texto não é RFC 3339 válido.
fn parse_timestamp(column: &str, value: &str) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|error| {
            AppError::infrastructure(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("coluna {column} com data inválida: {error}"),
            ))
        })
}

/// Traduz violações de escrita em erros de negócio.
///
/// - SKU duplicado entre ativos vira `AlreadyInUse`.
/// - Categoria inexistente (FK) vira `CategoryNotFound`.
///
/// A detecção usa `SQLSTATE` e o nome da constraint
/// (`products_sku_active_unique`), não o texto da mensagem: o PostgreSQL
/// traduz a mensagem conforme o locale do servidor.
fn map_product_write_error(error: sqlx::Error) -> AppError {
    if crate::pg_errors::is_unique_violation_on(&error, "products_sku_active_unique") {
        return AppError::AlreadyInUse("SKU".to_owned());
    }
    // A FK de categoria é a única possível nesta tabela, então o código do
    // `SQLSTATE` basta.
    if crate::pg_errors::is_foreign_key_violation(&error) {
        return AppError::CategoryNotFound;
    }

    AppError::infrastructure(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Filtro vazio, base para os testes de `WHERE`.
    fn empty_filter() -> ProductFilter {
        ProductFilter::default()
    }

    /// Verifica o contrato identificado por `where_always_filters_soft_deleted` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn where_always_filters_soft_deleted() {
        let (condition, values) = SqlxProductRepository::build_where(&empty_filter(), 1);

        assert!(condition.contains("deleted_at IS NULL"));
        assert!(values.is_empty());
    }

    /// Verifica o contrato identificado por `category_filter_adds_placeholder_and_value` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn category_filter_adds_placeholder_and_value() {
        let category = UserId::new_v4();
        let filter = ProductFilter {
            category_id: Some(category),
            ..Default::default()
        };

        let (condition, values) = SqlxProductRepository::build_where(&filter, 1);

        assert!(condition.contains("category_id = $1"));
        assert_eq!(values, vec![category.to_string()]);
    }

    /// Verifica o contrato identificado por `name_filter_escapes_like_wildcards` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn name_filter_escapes_like_wildcards() {
        let filter = ProductFilter {
            name_contains: Some("100%".to_owned()),
            ..Default::default()
        };

        let (condition, values) = SqlxProductRepository::build_where(&filter, 1);

        assert!(condition.contains("LOWER(name) LIKE LOWER($1)"));
        assert_eq!(
            values,
            vec!["%100\\%%".to_owned()],
            "o `%` do usuário deve ser escapado"
        );
    }

    /// Verifica o contrato identificado por `filters_combine_with_and` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn filters_combine_with_and() {
        let filter = ProductFilter {
            category_id: Some(UserId::new_v4()),
            name_contains: Some("teclado".to_owned()),
            only_active: Some(true),
        };

        let (condition, values) = SqlxProductRepository::build_where(&filter, 1);

        assert_eq!(values.len(), 2, "dois filtros parametrizados");
        assert!(condition.contains("AND"));
        assert!(condition.contains("status = 'active'"));
    }

    /// Verifica o contrato identificado por `escape_like_handles_backslash_and_underscore` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn escape_like_handles_backslash_and_underscore() {
        assert_eq!(escape_like("a_b"), "a\\_b");
        assert_eq!(escape_like("a\\b"), "a\\\\b");
        assert_eq!(escape_like("normal"), "normal");
    }
}
