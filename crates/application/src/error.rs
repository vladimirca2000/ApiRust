//! Erros tipados da camada de aplicação e conversão de falhas de infraestrutura.
//!
//! Mantém falhas de domínio, autenticação e persistência distinguíveis até a
//! camada HTTP, que escolhe a resposta pública apropriada.

use std::error::Error;
use std::fmt;

use domain::DomainError;
use thiserror::Error as ThisError;

/// Erro de infraestrutura preservando a causa original.
#[derive(Debug)]
pub struct InfrastructureError(Box<dyn Error + Send + Sync>);

impl InfrastructureError {
    /// Embrulha o erro original preservando a cadeia de causas.
    #[must_use]
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for InfrastructureError {
    /// Escreve a mensagem segura do erro de aplicação no formatter e retorna o resultado da escrita.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl Error for InfrastructureError {
    /// Expõe a causa encadeada quando o erro encapsula uma falha de infraestrutura; retorna `None` quando não há causa.
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

/// Falhas dos casos de uso.
#[derive(Debug, ThisError)]
pub enum AppError {
    /// Violação de regra de negócio, propagada do domínio sem alteração.
    #[error(transparent)]
    Domain(#[from] DomainError),

    /// Já existe um usuário ativo com o e-mail informado.
    #[error("e-mail já está em uso")]
    EmailAlreadyInUse,

    /// O usuário consultado não existe ou recebeu soft delete.
    #[error("usuário não encontrado")]
    UserNotFound,

    /// Credenciais inválidas ou token que não passou na verificação.
    ///
    /// Deliberadamente não distingue "e-mail não existe", "senha errada" e
    /// "token inválido" para a borda HTTP, evitando enumerar contas.
    #[error("credenciais inválidas")]
    Authentication,

    /// A operação exige um usuário ativo, mas a conta está desativada.
    #[error("usuário inativo")]
    InactiveUser,

    /// O ator está autenticado, mas não tem o privilégio exigido.
    #[error("acesso negado")]
    Forbidden,

    /// O ator tentou alterar o próprio papel ou excluir a própria conta.
    #[error("operação não permitida sobre a própria conta")]
    SelfModificationForbidden,

    /// O recurso referenciado não existe ou está excluído.
    #[error("recurso não encontrado")]
    NotFound,

    /// A categoria referenciada não existe ou está excluída.
    #[error("categoria não encontrada")]
    CategoryNotFound,

    /// Já existe outro registro ativo com o mesmo valor único (nome, SKU).
    #[error("valor já está em uso: {0}")]
    AlreadyInUse(String),

    /// A ordenação solicitada não está na allowlist.
    #[error("ordenação inválida: {0}")]
    InvalidSort(String),

    /// `offset`/`limit` fora do intervalo aceito.
    ///
    /// Recusar é proposital: corrigir em silêncio faz um cliente com bug
    /// interpretar uma página de um item como "fim dos dados".
    #[error("paginação inválida: {0}")]
    InvalidPagination(String),

    /// A operação violaria uma regra de negócio do catálogo.
    #[error("{0}")]
    Conflict(String),

    /// Falha em banco, hashing, rede ou outra dependência externa.
    #[error("falha de infraestrutura: {0}")]
    Infrastructure(#[from] InfrastructureError),
}

impl AppError {
    /// Embrulha um erro de infraestrutura mantendo a cadeia de causas.
    #[must_use]
    pub fn infrastructure(error: impl Error + Send + Sync + 'static) -> Self {
        Self::Infrastructure(InfrastructureError::new(error))
    }
}
