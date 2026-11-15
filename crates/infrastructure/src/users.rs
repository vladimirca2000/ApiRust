use application::{AppError, UserRepository};
use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use domain::{Email, EntityStatus, PasswordHash, Role, User, UserId, UserTimestamps};
use sqlx::SqlitePool;

/// Busca um usuário vivo por identificador, ignorando soft delete.
const FIND_BY_ID: &str = "SELECT id, name, email, password_hash, role, status, created_at, \
                          updated_at, deleted_at \
                          FROM users WHERE id = ? AND deleted_at IS NULL";

/// Busca um usuário vivo por e-mail, base da unicidade entre ativos.
const FIND_BY_EMAIL: &str = "SELECT id, name, email, password_hash, role, status, created_at, \
                             updated_at, deleted_at \
                             FROM users WHERE email = ? AND deleted_at IS NULL";

/// Insere um usuário novo; violações do índice parcial viram `EmailAlreadyInUse`.
const INSERT: &str = "INSERT INTO users \
                      (id, name, email, password_hash, role, status, created_at, updated_at, deleted_at) \
                      VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)";

/// Lista usuários vivos com paginação estável.
///
/// A ordenação por `created_at` seguida de `id` garante que registros criados no
/// mesmo instante não troquem de posição entre páginas.
const LIST: &str = "SELECT id, name, email, password_hash, role, status, created_at, \
                    updated_at, deleted_at \
                    FROM users WHERE deleted_at IS NULL \
                    ORDER BY created_at ASC, id ASC LIMIT ? OFFSET ?";

/// Conta os usuários vivos, para o total da paginação.
const COUNT: &str = "SELECT count(*) FROM users WHERE deleted_at IS NULL";

/// Atualiza todos os campos mutáveis de um usuário existente.
const UPDATE: &str = "UPDATE users SET name = ?, email = ?, password_hash = ?, role = ?, \
                      status = ?, updated_at = ?, deleted_at = ? WHERE id = ?";

/// Repositório SQLx para usuários.
///
/// Consultas ignoram registros com soft delete e a escrita traduz violação do
/// índice parcial de e-mail em [`AppError::EmailAlreadyInUse`].
#[derive(Debug, Clone)]
pub struct SqlxUserRepository {
    /// Pool usado em todas as consultas do repositório.
    pool: SqlitePool,
}

impl SqlxUserRepository {
    /// Monta o repositório sobre um pool já configurado.
    ///
    /// O pool é clonado (handle compartilhado), então o repositório pode viver
    /// além do escopo de quem o criou.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl UserRepository for SqlxUserRepository {
    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, AppError> {
        let row = sqlx::query_as::<_, UserRow>(FIND_BY_ID)
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        row.map(User::try_from).transpose()
    }

    async fn find_by_email(&self, email: &Email) -> Result<Option<User>, AppError> {
        let row = sqlx::query_as::<_, UserRow>(FIND_BY_EMAIL)
            .bind(email.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        row.map(User::try_from).transpose()
    }

    async fn insert(&self, user: &User) -> Result<(), AppError> {
        let result = sqlx::query(INSERT)
            .bind(user.id().to_string())
            .bind(user.name())
            .bind(user.email().as_str())
            .bind(user.password_hash().as_str())
            .bind(user.role().as_str())
            .bind(user.status().as_str())
            .bind(format_timestamp(user.created_at()))
            .bind(format_timestamp(user.updated_at()))
            .bind(user.deleted_at().map(format_timestamp))
            .execute(&self.pool)
            .await;

        result.map(|_| ()).map_err(map_write_error)
    }

    async fn list(&self, offset: i64, limit: i64) -> Result<Vec<User>, AppError> {
        let rows = sqlx::query_as::<_, UserRow>(LIST)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        rows.into_iter().map(User::try_from).collect()
    }

    async fn count(&self) -> Result<i64, AppError> {
        sqlx::query_scalar::<_, i64>(COUNT)
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::infrastructure)
    }

    async fn update(&self, user: &User) -> Result<(), AppError> {
        let result = sqlx::query(UPDATE)
            .bind(user.name())
            .bind(user.email().as_str())
            .bind(user.password_hash().as_str())
            .bind(user.role().as_str())
            .bind(user.status().as_str())
            .bind(format_timestamp(user.updated_at()))
            .bind(user.deleted_at().map(format_timestamp))
            .bind(user.id().to_string())
            .execute(&self.pool)
            .await;

        let executed = result.map_err(map_write_error)?;
        if executed.rows_affected() == 0 {
            return Err(AppError::UserNotFound);
        }
        Ok(())
    }
}

/// Linha crua da tabela `users`, antes da validação do domínio.
///
/// Mantém tudo como `String` porque o banco pode conter dados inválidos (por
/// exemplo, hash em texto puro); a conversão para [`User`] é que rejeita.
#[derive(sqlx::FromRow)]
struct UserRow {
    /// UUID do usuário em texto.
    id: String,
    /// Nome de exibição persistido.
    name: String,
    /// E-mail normalizado persistido.
    email: String,
    /// Hash PHC da senha.
    password_hash: String,
    /// Papel persistido (`admin`/`manager`/`user`).
    role: String,
    /// Estado persistido (`active`/`inactive`).
    status: String,
    /// Data de criação em RFC 3339.
    created_at: String,
    /// Data da última atualização em RFC 3339.
    updated_at: String,
    /// Data do soft delete em RFC 3339, quando houver.
    deleted_at: Option<String>,
}

impl TryFrom<UserRow> for User {
    type Error = AppError;

    fn try_from(row: UserRow) -> Result<Self, Self::Error> {
        let timestamps = UserTimestamps::new(
            parse_timestamp("created_at", &row.created_at)?,
            parse_timestamp("updated_at", &row.updated_at)?,
            row.deleted_at
                .as_deref()
                .map(|value| parse_timestamp("deleted_at", value))
                .transpose()?,
        )?;

        Ok(User::restore(
            UserId::parse(&row.id)?,
            row.name,
            Email::parse(row.email)?,
            PasswordHash::parse(row.password_hash)?,
            Role::parse(&row.role)?,
            EntityStatus::parse(&row.status)?,
            timestamps,
        )?)
    }
}

/// Serializa um instante para o formato de texto aceito pelo SQLite.
///
/// Usa RFC 3339 em UTC com precisão de microssegundos, garantindo ordenação
/// lexicográfica equivalente à cronológica.
fn format_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Converte uma coluna de data em `DateTime<Utc>`.
///
/// # Errors
///
/// Retorna [`AppError::Infrastructure`] quando o texto não é RFC 3339 válido,
/// indicando dado corrompido na coluna `column`.
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

/// Traduz erros de escrita do SQLite para o vocabulário da aplicação.
///
/// Violação do índice parcial `users_email_active_unique` vira
/// [`AppError::EmailAlreadyInUse`], que o caso de uso sabe tratar como
/// "já existe"; os demais erros permanecem como falha de infraestrutura.
fn map_write_error(error: sqlx::Error) -> AppError {
    if let sqlx::Error::Database(database_error) = &error {
        if database_error.is_unique_violation() && database_error.message().contains("users.email")
        {
            return AppError::EmailAlreadyInUse;
        }
    }

    AppError::infrastructure(error)
}
