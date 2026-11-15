//! Adaptador SQLite do armazenamento de refresh tokens.
//!
//! A tabela `refresh_tokens` guarda apenas o hash SHA-256 do token, nunca o
//! valor em claro. A cadeia `replaced_by_id` registra a rotação, permitindo
//! identificar reutilização de token já consumido.

use application::{AppError, RefreshRecord, RefreshTokenStore};
use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::PgPool;

/// Insere um refresh token recém-emitido.
const INSERT: &str = "INSERT INTO refresh_tokens \
                      (id, user_id, token_hash, expires_at, created_at) \
                      VALUES ($1, $2, $3, $4, $5)";

/// Busca o registro pelo hash, incluindo revogados (necessário para detectar reuso).
const FIND_BY_HASH: &str = "SELECT id, user_id, expires_at, revoked_at \
                            FROM refresh_tokens WHERE token_hash = $1";

/// Revoga um token específico, registrando o sucessor quando houver rotação.
const REVOKE: &str = "UPDATE refresh_tokens SET revoked_at = $1, replaced_by_id = $2 \
                      WHERE token_hash = $3 AND revoked_at IS NULL";

/// Revoga todos os tokens ainda vivos de um usuário.
const REVOKE_ALL: &str = "UPDATE refresh_tokens SET revoked_at = $1 \
                          WHERE user_id = $2 AND revoked_at IS NULL";

/// Repositório de refresh tokens sobre SQLite.
#[derive(Debug, Clone)]
pub struct SqlxRefreshTokenStore {
    /// Pool usado em todas as operações de persistência.
    pool: PgPool,
}

impl SqlxRefreshTokenStore {
    /// Monta o repositório sobre um pool já configurado.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl RefreshTokenStore for SqlxRefreshTokenStore {
    /// Persiste o novo registro e retorna sucesso após a gravação; violações de unicidade, integridade ou falhas SQL são convertidas em erro da aplicação.
    async fn insert(
        &self,
        token_hash: &str,
        user_id: &str,
        expires_at: DateTime<Utc>,
        created_at: DateTime<Utc>,
    ) -> Result<RefreshRecord, AppError> {
        let id = uuid::Uuid::new_v4().to_string();

        sqlx::query(INSERT)
            .bind(&id)
            .bind(user_id)
            .bind(token_hash)
            .bind(format_timestamp(expires_at))
            .bind(format_timestamp(created_at))
            .execute(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        Ok(RefreshRecord {
            id,
            user_id: user_id.to_owned(),
            expires_at,
            revoked_at: None,
        })
    }
    /// Busca o refresh token pelo hash persistido; retorna o registro encontrado,
    /// `None` quando não existe ou erro de persistência.
    async fn find_by_hash(&self, token_hash: &str) -> Result<Option<RefreshRecord>, AppError> {
        let row = sqlx::query_as::<_, RefreshRow>(FIND_BY_HASH)
            .bind(token_hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        row.map(RefreshRow::into_record).transpose()
    }
    /// Marca o identificador do token como revogado até sua expiração; retorna sucesso ou erro se o armazenamento distribuído falhar.
    async fn revoke(
        &self,
        token_hash: &str,
        replaced_by_id: Option<&str>,
        revoked_at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        sqlx::query(REVOKE)
            .bind(format_timestamp(revoked_at))
            .bind(replaced_by_id)
            .bind(token_hash)
            .execute(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        Ok(())
    }
    /// Revoga todos os refresh tokens do usuário e retorna sucesso ou erro de persistência.
    async fn revoke_all_for_user(
        &self,
        user_id: &str,
        revoked_at: DateTime<Utc>,
    ) -> Result<u64, AppError> {
        let result = sqlx::query(REVOKE_ALL)
            .bind(format_timestamp(revoked_at))
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        Ok(result.rows_affected())
    }
}

/// Linha crua da tabela `refresh_tokens`.
#[derive(sqlx::FromRow)]
struct RefreshRow {
    /// Chave primária do registro.
    id: String,
    /// Usuário dono do token.
    user_id: String,
    /// Expiração em RFC 3339.
    expires_at: String,
    /// Revogação em RFC 3339, quando houver.
    revoked_at: Option<String>,
}

impl RefreshRow {
    /// Converte a linha para o tipo de domínio, validando as datas.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se alguma data estiver corrompida.
    fn into_record(self) -> Result<RefreshRecord, AppError> {
        Ok(RefreshRecord {
            id: self.id,
            user_id: self.user_id,
            expires_at: parse_timestamp("expires_at", &self.expires_at)?,
            revoked_at: self
                .revoked_at
                .as_deref()
                .map(|value| parse_timestamp("revoked_at", value))
                .transpose()?,
        })
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
