//! Casos de uso e portas da aplicação.
//!
//! A camada depende apenas do domínio e define as portas que a infraestrutura
//! implementa. Nenhum tipo de HTTP, SQL ou Redis aparece aqui.

/// Trilha de auditoria: quem alterou o quê, quando e com qual valor.
pub mod audit;
/// Caso de uso que registra e consulta a trilha de auditoria.
mod audit_trail;
/// Caso de uso de login, rotação de refresh e logout.
mod authenticate;
/// Portas de persistência do catálogo.
mod catalog_ports;
/// Portas de estado distribuído: rate limit e denylist de access token.
pub mod distributed;
/// Caso de uso que provisiona o Admin inicial de forma idempotente.
mod ensure_admin;
/// Erros dos casos de uso e o embrulho de falhas de infraestrutura.
mod error;
/// Porta de histórico do catálogo (estoque e preço).
mod history_ports;
/// Casos de uso de categorias e produtos.
mod manage_catalog;
/// Casos de uso de gerenciamento de usuários.
mod manage_users;
/// Limites de paginação compartilhados pelas listagens.
mod pagination;
/// Política de autorização centralizada (RBAC).
mod policy;
/// Portas (traits) que a infraestrutura deve implementar.
mod ports;

pub use audit::{AuditAction, AuditEntity, AuditEntry, AuditFilter, AuditPage, AuditRepository};
pub use audit_trail::AuditTrail;
pub use authenticate::{AuthenticateUser, REFRESH_TTL_DAYS, hash_refresh, random_hex_token};
pub use catalog_ports::{
    CategoryRepository, ProductFilter, ProductRepository, ProductSort, format_timestamp,
};
pub use distributed::{AccessTokenDenylist, RateLimitDecision, RateLimitPolicy, RateLimitStore};
pub use ensure_admin::{ADMIN_NAME, EnsureAdmin, SeedOutcome};
pub use error::{AppError, InfrastructureError};
pub use history_ports::{CatalogHistoryRepository, ProductChange, expected_values};
pub use manage_catalog::{
    ManageCategories, ManageProducts, NewCategoryInput, NewProductInput, Page, UpdateCategoryInput,
    UpdateProductInput,
};
pub use manage_users::{ManageUsers, NewUserInput, UpdateProfileInput, UserPage};
pub use pagination::{DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, normalize as normalize_page};
pub use policy::{Actor, UserAction};
pub use ports::{PasswordHasher, RefreshRecord, RefreshTokenStore, TokenService, UserRepository};
