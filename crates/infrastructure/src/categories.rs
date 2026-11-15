//! Adaptador SQLx da porta `CategoryRepository`.

use application::{AppError, CategoryRepository};
use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use domain::{Category, EntityStatus, UserId};
use sqlx::SqlitePool;

/// Busca uma categoria viva por identificador.
const FIND_BY_ID: &str = "SELECT id, name, description, status, created_at, updated_at, deleted_at \
                          FROM categories WHERE id = ? AND deleted_at IS NULL";

/// Busca uma categoria viva por nome (comparação exata, sensível a caixa).
const FIND_BY_NAME: &str = "SELECT id, name, description, status, created_at, updated_at, deleted_at \
                            FROM categories WHERE name = ? AND deleted_at IS NULL";

/// Insere uma categoria nova.
const INSERT: &str = "INSERT INTO categories \
                      (id, name, description, status, created_at, updated_at, deleted_at) \
                      VALUES (?, ?, ?, ?, ?, ?, ?)";

/// Atualiza todos os campos mutáveis de uma categoria.
const UPDATE: &str = "UPDATE categories SET name = ?, description = ?, status = ?, \
                      updated_at = ?, deleted_at = ? WHERE id = ?";

/// Lista categorias vivas com ordenação estável.
const LIST: &str = "SELECT id, name, description, status, created_at, updated_at, deleted_at \
                    FROM categories WHERE deleted_at IS NULL \
                    ORDER BY name ASC, id ASC LIMIT ? OFFSET ?";

/// Conta as categorias vivas.
const COUNT: &str = "SELECT count(*) FROM categories WHERE deleted_at IS NULL";

/// Conta produtos vivos vinculados à categoria.
const COUNT_PRODUCTS: &str =
    "SELECT count(*) FROM products WHERE category_id = ? AND deleted_at IS NULL";

/// Repositório de categorias sobre SQLite.
#[derive(Debug, Clone)]
pub struct SqlxCategoryRepository {
    /// Pool usado em todas as operações.
    pool: SqlitePool,
}

impl SqlxCategoryRepository {
    /// Monta o repositório sobre um pool já configurado.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl CategoryRepository for SqlxCategoryRepository {
    async fn find_by_id(&self, id: UserId) -> Result<Option<Category>, AppError> {
        let row = sqlx::query_as::<_, CategoryRow>(FIND_BY_ID)
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        row.map(CategoryRow::into_category).transpose()
    }

    async fn find_by_name(&self, name: &str) -> Result<Option<Category>, AppError> {
        let row = sqlx::query_as::<_, CategoryRow>(FIND_BY_NAME)
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        row.map(CategoryRow::into_category).transpose()
    }

    async fn insert(&self, category: &Category) -> Result<(), AppError> {
        let result = sqlx::query(INSERT)
            .bind(category.id().to_string())
            .bind(category.name())
            .bind(category.description())
            .bind(category.status().as_str())
            .bind(format_timestamp(category.created_at()))
            .bind(format_timestamp(category.updated_at()))
            .bind(category.deleted_at().map(format_timestamp))
            .execute(&self.pool)
            .await;

        result.map(|_| ()).map_err(map_category_write_error)
    }

    async fn update(&self, category: &Category) -> Result<(), AppError> {
        let result = sqlx::query(UPDATE)
            .bind(category.name())
            .bind(category.description())
            .bind(category.status().as_str())
            .bind(format_timestamp(category.updated_at()))
            .bind(category.deleted_at().map(format_timestamp))
            .bind(category.id().to_string())
            .execute(&self.pool)
            .await;

        let executed = result.map_err(map_category_write_error)?;
        if executed.rows_affected() == 0 {
            return Err(AppError::CategoryNotFound);
        }
        Ok(())
    }

    async fn list(&self, offset: i64, limit: i64) -> Result<Vec<Category>, AppError> {
        let rows = sqlx::query_as::<_, CategoryRow>(LIST)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        rows.into_iter().map(CategoryRow::into_category).collect()
    }

    async fn count(&self) -> Result<i64, AppError> {
        sqlx::query_scalar::<_, i64>(COUNT)
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::infrastructure)
    }

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
    if let sqlx::Error::Database(database_error) = &error {
        if database_error.is_unique_violation()
            && database_error.message().contains("categories.name")
        {
            return AppError::AlreadyInUse("nome de categoria".to_owned());
        }
    }
    AppError::infrastructure(error)
}
