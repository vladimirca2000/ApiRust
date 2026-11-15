//! Modelo de domínio e regras de negócio da API.
//!
//! Este crate não depende de frameworks HTTP, banco de dados, Redis ou
//! qualquer detalhe de infraestrutura. Entidades e Value Objects preservam
//! suas invariantes por meio de construtores e métodos de domínio.

/// Categoria de produtos e a política de exclusão associada.
mod category;
/// Endereço de e-mail normalizado e validado.
mod email;
/// Falhas de regra de negócio compartilhadas pelo domínio.
mod error;
/// Valor monetário em centavos.
mod money;
/// Política de senha e o hash já calculado.
mod password;
/// Produto do catálogo.
mod product;
/// Papel de acesso e as capacidades associadas.
mod role;
/// SKU normalizado do produto.
mod sku;
/// Estado ativo/inativo compartilhado pelas entidades.
mod status;
/// Tokens de autenticação: access JWT e refresh opaco.
mod token;
/// Entidade `User` e seus timestamps.
mod user;
/// Identificador tipado de usuário.
mod user_id;

pub use category::{
    Category, DELETE_WITH_PRODUCTS_POLICY, MAX_CATEGORY_NAME_LEN, MAX_DESCRIPTION_LEN,
    validate_description,
};
pub use email::{Email, MAX_EMAIL_LEN, MAX_LOCAL_PART_LEN};
pub use error::DomainError;
pub use money::Money;
pub use password::{MAX_PASSWORD_LEN, MIN_PASSWORD_LEN, PasswordHash, validate_password};
pub use product::{MAX_PRODUCT_NAME_LEN, Product};
pub use role::Role;
pub use sku::{MAX_SKU_LEN, MIN_SKU_LEN, Sku};
pub use status::EntityStatus;
pub use token::{AccessClaims, RefreshTokenSecret, RotationOutcome, TokenPair};
pub use user::{MAX_NAME_LEN, User, UserTimestamps};
pub use user_id::UserId;
