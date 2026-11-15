//! Erros de validação e de invariantes do domínio.
//!
//! Os casos são independentes de transporte e persistência para que camadas
//! externas possam traduzi-los sem acoplar as regras de negócio.

use thiserror::Error;

/// Violações das invariantes de domínio.
///
/// Erros de domínio descrevem entrada inválida e não devem ser confundidos com
/// falhas de infraestrutura. Cada variante carrega a razão legível para que a
/// borda HTTP possa informar o cliente sem expor detalhes internos.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DomainError {
    /// O nome é vazio, excede [`crate::MAX_NAME_LEN`] ou contém caracteres de controle.
    #[error("nome inválido: {0}")]
    InvalidName(String),

    /// O e-mail não segue o formato `local@dominio` ou excede os limites da RFC 5321.
    #[error("e-mail inválido: {0}")]
    InvalidEmail(String),

    /// A senha em texto puro viola a política de força.
    #[error("senha inválida: {0}")]
    InvalidPassword(String),

    /// O valor não é um hash PHC de Argon2id válido.
    #[error("hash de senha inválido: {0}")]
    InvalidPasswordHash(String),

    /// O papel não é `admin`, `manager` ou `user`.
    #[error("papel inválido: {0}")]
    InvalidRole(String),

    /// O estado não é `active` ou `inactive`.
    #[error("estado inválido: {0}")]
    InvalidStatus(String),

    /// O identificador não é um UUID válido.
    #[error("identificador inválido: {0}")]
    InvalidId(String),

    /// As datas são inconsistentes (por exemplo, `updated_at` anterior a `created_at`).
    #[error("data inválida: {0}")]
    InvalidTimestamp(String),

    /// A operação exige um registro vivo, mas o usuário já recebeu soft delete.
    #[error("usuário excluído não pode ser alterado")]
    DeletedUser,

    /// O valor monetário é zero ou negativo.
    #[error("valor monetário inválido: {0}")]
    InvalidMoney(String),

    /// O SKU não segue o formato permitido.
    #[error("SKU inválido: {0}")]
    InvalidSku(String),

    /// O estoque é negativo.
    #[error("estoque inválido: {0}")]
    InvalidStock(String),

    /// A descrição excede o tamanho máximo ou contém caracteres de controle.
    #[error("descrição inválida: {0}")]
    InvalidDescription(String),

    /// A categoria referenciada não existe ou está excluída.
    #[error("categoria não encontrada")]
    CategoryNotFound,

    /// O recurso referenciado não existe ou está excluído.
    #[error("recurso não encontrado")]
    NotFound,

    /// Já existe outro registro ativo com o mesmo valor único (nome, SKU).
    #[error("valor já está em uso: {0}")]
    AlreadyInUse(String),

    /// O motivo de movimentação de estoque não é conhecido.
    #[error("motivo de estoque inválido: {0}")]
    InvalidStockReason(String),

    /// A movimentação de estoque tem variação zero e não explica nada.
    #[error("movimentação de estoque não pode ter variação zero")]
    EmptyStockMovement,

    /// O histórico de preço exige um valor diferente do atual.
    #[error("alteração de preço exige valor diferente do atual")]
    UnchangedPrice,
}
