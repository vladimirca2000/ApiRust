//! Adaptador SQLx da porta `CategoryRepository`.

use application::audit::AuditEntry;
use application::{AppError, CategoryRepository};
use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use domain::{Category, EntityStatus, UserId};
use sqlx::PgPool;

/// Busca uma categoria viva por identificador.
const FIND_BY_ID: &str = "SELECT id, name, description, status, created_at, updated_at, deleted_at \
                          FROM categories WHERE id = $1 AND deleted_at IS NULL";

/// Busca uma categoria viva por nome (comparação exata, sensível a caixa).
const FIND_BY_NAME: &str = "SELECT id, name, description, status, created_at, updated_at, deleted_at \
                            FROM categories WHERE name = $1 AND deleted_at IS NULL";

/// Insere uma categoria nova.
const INSERT: &str = "INSERT INTO categories \
                      (id, name, description, status, created_at, updated_at, deleted_at) \
                      VALUES ($1, $2, $3, $4, $5, $6, $7)";

/// Atualiza todos os campos mutáveis de uma categoria.
const UPDATE: &str = "UPDATE categories SET name = $1, description = $2, status = $3, \
                      updated_at = $4, deleted_at = $5 WHERE id = $6";

/// Lista categorias vivas com ordenação estável.
const LIST: &str = "SELECT id, name, description, status, created_at, updated_at, deleted_at \
                    FROM categories WHERE deleted_at IS NULL \
                    ORDER BY name ASC, id ASC LIMIT $1 OFFSET $2";

/// Conta as categorias vivas.
const COUNT: &str = "SELECT count(*) FROM categories WHERE deleted_at IS NULL";

/// Conta produtos vivos vinculados à categoria.
const COUNT_PRODUCTS: &str =
    "SELECT count(*) FROM products WHERE category_id = $1 AND deleted_at IS NULL";

/// Repositório de categorias sobre SQLite.
#[derive(Debug, Clone)]
pub struct SqlxCategoryRepository {
    /// Pool usado em todas as operações.
    pool: PgPool,
}

impl SqlxCategoryRepository {
    /// Monta o repositório sobre um pool já configurado.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl CategoryRepository for SqlxCategoryRepository {
    /// Busca o registro pelo identificador e retorna `Some` quando encontrado, `None` quando ausente ou erro de persistência.
    async fn find_by_id(&self, id: UserId) -> Result<Option<Category>, AppError> {
        let row = sqlx::query_as::<_, CategoryRow>(FIND_BY_ID)
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        row.map(CategoryRow::into_category).transpose()
    }
    /// Busca a categoria pelo nome e retorna `Some` quando encontrada, `None` quando ausente ou erro de persistência.
    async fn find_by_name(&self, name: &str) -> Result<Option<Category>, AppError> {
        let row = sqlx::query_as::<_, CategoryRow>(FIND_BY_NAME)
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        row.map(CategoryRow::into_category).transpose()
    }
    /// Persiste o novo registro e retorna sucesso após a gravação; violações de unicidade, integridade ou falhas SQL são convertidas em erro da aplicação.
    async fn insert(
        &self,
        category: &Category,
        audit: Option<&AuditEntry>,
    ) -> Result<(), AppError> {
        let mut transaction = self.pool.begin().await.map_err(AppError::infrastructure)?;

        sqlx::query(INSERT)
            .bind(category.id().to_string())
            .bind(category.name())
            .bind(category.description())
            .bind(category.status().as_str())
            .bind(format_timestamp(category.created_at()))
            .bind(format_timestamp(category.updated_at()))
            .bind(category.deleted_at().map(format_timestamp))
            .execute(&mut *transaction)
            .await
            .map_err(map_category_write_error)?;

        if let Some(entry) = audit {
            crate::audit_tx::insert_audit(&mut transaction, entry).await?;
        }

        transaction.commit().await.map_err(AppError::infrastructure)
    }
    /// Atualiza o registro persistido e retorna sucesso após a gravação; ausência, conflito de versão ou falha SQL são convertidos em erro da aplicação.
    async fn update(
        &self,
        category: &Category,
        audit: Option<&AuditEntry>,
    ) -> Result<(), AppError> {
        let mut transaction = self.pool.begin().await.map_err(AppError::infrastructure)?;

        let executed = sqlx::query(UPDATE)
            .bind(category.name())
            .bind(category.description())
            .bind(category.status().as_str())
            .bind(format_timestamp(category.updated_at()))
            .bind(category.deleted_at().map(format_timestamp))
            .bind(category.id().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(map_category_write_error)?;

        if executed.rows_affected() == 0 {
            return Err(AppError::CategoryNotFound);
        }

        if let Some(entry) = audit {
            crate::audit_tx::insert_audit(&mut transaction, entry).await?;
        }

        transaction.commit().await.map_err(AppError::infrastructure)
    }
    /// Consulta uma página de registros aplicando offset e limite e retorna os itens ordenados ou erro de persistência.
    async fn list(&self, offset: i64, limit: i64) -> Result<Vec<Category>, AppError> {
        let rows = sqlx::query_as::<_, CategoryRow>(LIST)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        rows.into_iter().map(CategoryRow::into_category).collect()
    }
    /// Conta os registros considerados ativos pelo repositório e retorna o total ou erro de persistência.
    async fn count(&self) -> Result<i64, AppError> {
        sqlx::query_scalar::<_, i64>(COUNT)
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::infrastructure)
    }
    /// Conta os produtos associados à categoria e retorna o total ou erro de persistência.
    async fn count_products(&self, category_id: UserId) -> Result<i64, AppError> {
        sqlx::query_scalar::<_, i64>(COUNT_PRODUCTS)
            .bind(category_id.to_string())
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::infrastructure)
    }
}

/// Linha crua da tabela `categories`.
#[derive(sqlx::FromRow)]
struct CategoryRow {
    /// UUID em texto.
    id: String,
    /// Nome persistido.
    name: String,
    /// Descrição opcional.
    description: Option<String>,
    /// Estado persistido.
    status: String,
    /// Criação em RFC 3339.
    created_at: String,
    /// Última atualização em RFC 3339.
    updated_at: String,
    /// Soft delete em RFC 3339, quando houver.
    deleted_at: Option<String>,
}

impl CategoryRow {
    /// Converte a linha para a entidade, validando os campos.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Domain`] ou [`AppError::Infrastructure`] quando algum
    /// campo persistido não passa na validação.
    fn into_category(self) -> Result<Category, AppError> {
        Ok(Category::restore(
            UserId::parse(&self.id)?,
            self.name,
            self.description,
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

/// Traduz violação do índice parcial de nome em conflito de negócio.
///
/// O índice `categories_name_active_unique` cobre apenas registros vivos, então
/// essa violação significa "já existe categoria ativa com esse nome".
fn map_category_write_error(error: sqlx::Error) -> AppError {
    if crate::pg_errors::is_unique_violation_on(&error, "categories_name_active_unique") {
        return AppError::AlreadyInUse("nome de categoria".to_owned());
    }
    AppError::infrastructure(error)
}
