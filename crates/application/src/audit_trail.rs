//! Caso de uso de auditoria: registra e consulta a trilha.
//!
//! A gravação é feita pelos casos de uso que alteram entidades, com snapshots
//! **sem segredos**: o hash de senha nunca entra no `before`/`after`.
//!
//! Falha ao gravar auditoria é propagada, não engolida: um sistema que prefere
//! "funcionar" a registrar quem mudou um papel não tem trilha confiável — tem
//! uma trilha que só existe quando dá sorte.

use chrono::{DateTime, Utc};
use domain::UserId;

use crate::audit::{AuditAction, AuditEntity, AuditEntry, AuditFilter, AuditPage, AuditRepository};
use crate::error::AppError;

/// Serviço de auditoria ligado à porta de persistência.
pub struct AuditTrail<'a> {
    /// Repositório da trilha.
    repository: &'a dyn AuditRepository,
}

impl<'a> AuditTrail<'a> {
    /// Monta o serviço com a porta injetada.
    #[must_use]
    pub fn new(repository: &'a dyn AuditRepository) -> Self {
        Self { repository }
    }

    /// Registra uma criação.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a gravação falhar.
    pub async fn created(
        &self,
        actor: Option<UserId>,
        entity: AuditEntity,
        entity_id: impl Into<String>,
        after: serde_json::Value,
        at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        let entry = AuditEntry::created(actor, entity, entity_id, after, at);
        self.repository.record(&entry).await
    }

    /// Registra uma alteração, com os dois snapshots.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a gravação falhar.
    #[allow(clippy::too_many_arguments)]
    pub async fn updated(
        &self,
        actor: Option<UserId>,
        action: AuditAction,
        entity: AuditEntity,
        entity_id: impl Into<String>,
        before: serde_json::Value,
        after: serde_json::Value,
        at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        let entry = AuditEntry::updated(actor, action, entity, entity_id, before, after, at);
        self.repository.record(&entry).await
    }

    /// Lista a trilha com filtro e paginação.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a consulta falhar.
    pub async fn list(
        &self,
        filter: &AuditFilter,
        offset: i64,
        limit: i64,
    ) -> Result<AuditPage, AppError> {
        self.repository.list(filter, offset, limit).await
    }
}
