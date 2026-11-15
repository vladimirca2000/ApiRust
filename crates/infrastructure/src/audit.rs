//! Adaptador SQLx da porta `AuditRepository`.
//!
//! A trilha é append-only: só há `INSERT` e `SELECT`. Uma trilha que permite
//! `UPDATE` ou `DELETE` deixa de servir como prova de quem fez o quê.

use application::{
    AppError, AuditAction, AuditEntity, AuditEntry, AuditFilter, AuditPage, AuditRepository,
    format_timestamp,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use domain::UserId;
use sqlx::PgPool;

/// Colunas lidas em toda consulta da trilha.
const COLUMNS: &str = "actor_id, action, entity, entity_id, before, after, at";

/// Trilha de auditoria sobre PostgreSQL.
#[derive(Debug, Clone)]
pub struct SqlxAuditRepository {
    /// Pool usado nas operações.
    pool: PgPool,
}

impl SqlxAuditRepository {
    /// Monta o repositório sobre um pool já configurado.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AuditRepository for SqlxAuditRepository {
    /// Grava a entrada de auditoria no banco e retorna sucesso ou erro de persistência.
    async fn record(&self, entry: &AuditEntry) -> Result<(), AppError> {
        let sql = "INSERT INTO audit_log (id, actor_id, action, entity, entity_id, before, after, at) \
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8)";

        sqlx::query(sql)
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(entry.actor_id.map(|id| id.to_string()))
            .bind(entry.action.as_str())
            .bind(entry.entity.as_str())
            .bind(&entry.entity_id)
            .bind(
                entry
                    .before
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|error| {
                        AppError::infrastructure(std::io::Error::other(error.to_string()))
                    })?,
            )
            .bind(
                entry
                    .after
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|error| {
                        AppError::infrastructure(std::io::Error::other(error.to_string()))
                    })?,
            )
            .bind(format_timestamp(entry.at))
            .execute(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        Ok(())
    }
    /// Consulta uma página de registros aplicando offset e limite e retorna os itens ordenados ou erro de persistência.
    async fn list(
        &self,
        filter: &AuditFilter,
        offset: i64,
        limit: i64,
    ) -> Result<AuditPage, AppError> {
        let mut conditions = vec!["1 = 1".to_owned()];
        let mut values = Vec::new();
        // Próximo número de placeholder disponível; `LIMIT`/`OFFSET` usam os
        // seguintes.
        let mut next = 1_usize;

        if let Some(entity) = filter.entity {
            conditions.push(format!("entity = ${next}"));
            values.push(entity.as_str().to_owned());
            next += 1;
        }
        if let Some(entity_id) = &filter.entity_id {
            conditions.push(format!("entity_id = ${next}"));
            values.push(entity_id.clone());
            next += 1;
        }
        if let Some(actor_id) = filter.actor_id {
            conditions.push(format!("actor_id = ${next}"));
            values.push(actor_id.to_string());
            next += 1;
        }

        let condition = conditions.join(" AND ");

        let total_sql = format!("SELECT count(*) FROM audit_log WHERE {condition}");
        let mut total_query = sqlx::query_scalar::<_, i64>(&total_sql);
        for value in &values {
            total_query = total_query.bind(value);
        }
        let total = total_query
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        let sql = format!(
            "SELECT {COLUMNS} FROM audit_log WHERE {condition} \
             ORDER BY at DESC, id DESC LIMIT ${next} OFFSET ${}",
            next + 1
        );
        let mut query = sqlx::query_as::<_, AuditRow>(&sql);
        for value in &values {
            query = query.bind(value);
        }

        let rows = query
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
            .map_err(AppError::infrastructure)?;

        let items = rows
            .into_iter()
            .filter_map(|row| row.into_entry().transpose())
            .collect::<Result<Vec<_>, _>>()?;

        Ok(AuditPage {
            items,
            total,
            offset,
            limit,
        })
    }
}

/// Linha crua de `audit_log`.
#[derive(sqlx::FromRow)]
struct AuditRow {
    /// Autor da ação, quando houve.
    actor_id: Option<String>,
    /// Ação persistida.
    action: String,
    /// Entidade persistida.
    entity: String,
    /// Identificador da entidade.
    entity_id: String,
    /// Snapshot anterior em JSON.
    before: Option<String>,
    /// Snapshot posterior em JSON.
    after: Option<String>,
    /// Instante em RFC 3339.
    at: String,
}

impl AuditRow {
    /// Converte a linha no tipo de domínio.
    ///
    /// Devolve `None` quando `action` ou `entity` estão fora do conjunto
    /// conhecido: a listagem ignora a linha corrompida em vez de derrubar a
    /// consulta inteira.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] para UUID/data/JSON inválidos, que
    /// impediriam reconstruir a entrada mesmo com tipo conhecido.
    fn into_entry(self) -> Result<Option<AuditEntry>, AppError> {
        let (Some(action), Some(entity)) = (
            AuditAction::parse(&self.action),
            AuditEntity::parse(&self.entity),
        ) else {
            return Ok(None);
        };

        let actor_id = self.actor_id.as_deref().map(UserId::parse).transpose()?;
        let at = DateTime::parse_from_rfc3339(&self.at)
            .map(|parsed| parsed.with_timezone(&Utc))
            .map_err(|error| AppError::infrastructure(std::io::Error::other(error.to_string())))?;

        let before = parse_json(self.before.as_deref())?;
        let after = parse_json(self.after.as_deref())?;

        Ok(Some(AuditEntry {
            actor_id,
            action,
            entity,
            entity_id: self.entity_id,
            before,
            after,
            at,
        }))
    }
}

/// Desserializa um snapshot JSON opcional.
///
/// # Errors
///
/// Retorna [`AppError::Infrastructure`] quando o texto não é JSON válido.
fn parse_json(value: Option<&str>) -> Result<Option<serde_json::Value>, AppError> {
    value
        .map(|text| {
            serde_json::from_str(text)
                .map_err(|error| AppError::infrastructure(std::io::Error::other(error.to_string())))
        })
        .transpose()
}
