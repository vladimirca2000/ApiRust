//! Portas de estado distribuído: rate limit e denylist de access token.
//!
//! Com uma única réplica, um `HashMap` no processo resolve. Com duas ou mais,
//! cada instância contaria em separado: o limite efetivo vira
//! `limite × réplicas` e o logout deixa o token válido na outra instância. As
//! portas aqui permitem trocar a implementação sem tocar nos casos de uso.

use std::time::Duration;

use async_trait::async_trait;

use crate::error::AppError;

/// Política de limite aplicada a uma chave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitPolicy {
    /// Máximo de tentativas dentro da janela.
    pub max_attempts: u32,
    /// Duração da janela.
    pub window: Duration,
}

/// Resultado de uma tentativa de consumo do limite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitDecision {
    /// `true` quando a tentativa foi contabilizada e pode prosseguir.
    pub allowed: bool,
    /// Tempo restante até a janela reabrir; presente apenas quando negado.
    pub retry_after: Option<Duration>,
}

impl RateLimitDecision {
    /// Decisão de tentativa aceita.
    #[must_use]
    pub const fn allowed() -> Self {
        Self {
            allowed: true,
            retry_after: None,
        }
    }

    /// Decisão de tentativa recusada, informando quando tentar de novo.
    #[must_use]
    pub const fn denied(retry_after: Duration) -> Self {
        Self {
            allowed: false,
            retry_after: Some(retry_after),
        }
    }

    /// Converte a decisão no `Result` usado pelos handlers.
    ///
    /// # Errors
    ///
    /// Retorna o tempo de espera quando a tentativa foi recusada.
    pub fn into_result(self) -> Result<(), Duration> {
        if self.allowed {
            Ok(())
        } else {
            Err(self.retry_after.unwrap_or_default())
        }
    }
}

/// Contador de tentativas com janela, potencialmente compartilhado entre réplicas.
#[async_trait]
pub trait RateLimitStore: Send + Sync {
    /// Registra uma tentativa para a chave e informa se ela é permitida.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] quando o backend de contagem está
    /// indisponível. O chamador deve tratar isso como falha fechada nas rotas
    /// sensíveis (login/refresh).
    async fn hit(&self, key: &str, policy: RateLimitPolicy) -> Result<RateLimitDecision, AppError>;
}

/// Denylist de access tokens por `jti`, com expiração natural.
///
/// Sem ela, um logout não invalida o access token: ele continua aceito até o
/// `exp`, porque a verificação é stateless.
#[async_trait]
pub trait AccessTokenDenylist: Send + Sync {
    /// Marca um `jti` como revogado até o instante informado.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a gravação falhar.
    async fn revoke(&self, jti: &str, expires_at_unix: i64) -> Result<(), AppError>;

    /// Informa se o `jti` já foi revogado.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a consulta falhar.
    async fn is_revoked(&self, jti: &str) -> Result<bool, AppError>;
}
