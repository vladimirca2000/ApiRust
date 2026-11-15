//! Trilha de auditoria: quem alterou o quê, quando e com qual valor.
//!
//! O log estruturado serve para operação, mas some com a rotação de arquivos e
//! não é consultável por entidade. A trilha é a resposta durável para "quem
//! rebaixou este Admin?" ou "quem mudou o preço deste produto?".

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use domain::UserId;

use crate::error::AppError;

/// Tipo de entidade afetada por uma ação auditada.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditEntity {
    /// Registro de usuário.
    User,
    /// Registro de categoria.
    Category,
    /// Registro de produto.
    Product,
}

impl AuditEntity {
    /// Texto persistido no banco.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Category => "category",
            Self::Product => "product",
        }
    }

    /// Converte o texto persistido de volta no tipo.
    ///
    /// Devolve `None` para valor fora do conjunto: o chamador decide se trata
    /// como linha corrompida (a listagem da trilha apenas ignora a linha).
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "category" => Some(Self::Category),
            "product" => Some(Self::Product),
            _ => None,
        }
    }
}

/// Ação registrada na trilha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditAction {
    /// Criação de registro.
    Create,
    /// Alteração de dados do registro.
    Update,
    /// Mudança de papel de acesso.
    ChangeRole,
    /// Exclusão lógica.
    Delete,
}

impl AuditAction {
    /// Texto persistido no banco.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::ChangeRole => "change_role",
            Self::Delete => "delete",
        }
    }

    /// Converte o texto persistido de volta na ação.
    ///
    /// Devolve `None` para valor desconhecido, pelo mesmo motivo de
    /// [`AuditEntity::parse`].
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            "change_role" => Some(Self::ChangeRole),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

/// Linha da trilha de auditoria.
///
/// `before`/`after` são snapshots em JSON. Guardam apenas campos de negócio:
/// nunca hash de senha, token ou segredo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    /// Quem executou a ação. `None` quando a origem é o próprio sistema (seed).
    pub actor_id: Option<UserId>,
    /// Ação executada.
    pub action: AuditAction,
    /// Tipo de entidade afetada.
    pub entity: AuditEntity,
    /// Identificador da entidade afetada.
    pub entity_id: String,
    /// Snapshot anterior, quando aplicável.
    pub before: Option<serde_json::Value>,
    /// Snapshot posterior, quando aplicável.
    pub after: Option<serde_json::Value>,
    /// Instante da ação, em UTC.
    pub at: DateTime<Utc>,
}

impl AuditEntry {
    /// Monta uma entrada de criação.
    #[must_use]
    pub fn created(
        actor_id: Option<UserId>,
        entity: AuditEntity,
        entity_id: impl Into<String>,
        after: serde_json::Value,
        at: DateTime<Utc>,
    ) -> Self {
        Self {
            actor_id,
            action: AuditAction::Create,
            entity,
            entity_id: entity_id.into(),
            before: None,
            after: Some(after),
            at,
        }
    }

    /// Monta uma entrada de alteração, preservando os dois lados.
    #[must_use]
    pub fn updated(
        actor_id: Option<UserId>,
        action: AuditAction,
        entity: AuditEntity,
        entity_id: impl Into<String>,
        before: serde_json::Value,
        after: serde_json::Value,
        at: DateTime<Utc>,
    ) -> Self {
        Self {
            actor_id,
            action,
            entity,
            entity_id: entity_id.into(),
            before: Some(before),
            after: Some(after),
            at,
        }
    }
}

/// Filtro de consulta da trilha.
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    /// Restringe a uma entidade específica.
    pub entity: Option<AuditEntity>,
    /// Restringe a um identificador de entidade.
    pub entity_id: Option<String>,
    /// Restringe a um ator.
    pub actor_id: Option<UserId>,
}

/// Página de entradas da trilha.
#[derive(Debug, Clone)]
pub struct AuditPage {
    /// Entradas da página atual, da mais recente para a mais antiga.
    pub items: Vec<AuditEntry>,
    /// Total de entradas que atendem ao filtro.
    pub total: i64,
    /// Offset aplicado.
    pub offset: i64,
    /// Limite aplicado.
    pub limit: i64,
}

/// Porta de persistência da trilha de auditoria.
#[async_trait]
pub trait AuditRepository: Send + Sync {
    /// Grava uma entrada. Nunca falha silenciosamente: erro é propagado.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a gravação falhar.
    async fn record(&self, entry: &AuditEntry) -> Result<(), AppError>;

    /// Lista entradas com filtro e paginação estável.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a consulta falhar.
    async fn list(
        &self,
        filter: &AuditFilter,
        offset: i64,
        limit: i64,
    ) -> Result<AuditPage, AppError>;
}
