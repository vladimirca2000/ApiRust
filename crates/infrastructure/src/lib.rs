//! Adaptadores de infraestrutura: banco de dados, Redis, JWT e hashing.
//!
//! Implementa as portas definidas em `application`/`domain` usando SQLx,
//! Argon2id e JWT. Nenhuma regra de negócio vive aqui.

/// Adaptador SQLite (via SQLx) do repositório de usuários.
pub mod db;
/// Falhas de preparação e acesso ao banco.
pub mod error;
/// Adaptador de hashing e verificação com Argon2id.
pub mod password;
/// Adaptador SQLite da porta `RefreshTokenStore`.
pub mod refresh;
/// Adaptador JWT HS256 da porta `TokenService`.
pub mod token;
/// Adaptador SQLx da porta `UserRepository`.
pub mod users;

/// Adaptador SQLite da porta `CategoryRepository`.
pub mod categories;
/// Adaptador SQLite da porta `ProductRepository`.
pub mod products;

pub use categories::SqlxCategoryRepository;
pub use db::Database;
pub use error::DbError;
pub use password::Argon2PasswordHasher;
pub use products::SqlxProductRepository;
pub use refresh::SqlxRefreshTokenStore;
pub use token::JwtTokenService;
pub use users::SqlxUserRepository;
