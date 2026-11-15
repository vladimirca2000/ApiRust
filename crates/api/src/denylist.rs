//! Denylist de access token em memória, para desenvolvimento e testes.
//!
//! Uma denylist local só é correta com **uma** instância: a revogação feita na
//! réplica A não é vista pela réplica B, então o token continuaria aceito lá.
//! Em produção, use `infrastructure::RedisAccessTokenDenylist`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use application::{AccessTokenDenylist, AppError};
use async_trait::async_trait;

/// Denylist em memória, com expiração verificada na leitura.
#[derive(Clone, Default)]
pub struct MemoryDenylist {
    /// Mapa `jti -> instante de expiração (unix)`, compartilhado entre clones.
    inner: Arc<Mutex<HashMap<String, i64>>>,
}

impl MemoryDenylist {
    /// Cria uma denylist vazia.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Quantidade de `jti` ainda dentro da validade (usado em testes).
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().expect("mutex da denylist").len()
    }

    /// Informa se a denylist está vazia (usado em testes e no lint de `len`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl AccessTokenDenylist for MemoryDenylist {
    /// Marca o identificador do token como revogado até sua expiração; retorna sucesso ou erro se o armazenamento distribuído falhar.
    async fn revoke(&self, jti: &str, expires_at_unix: i64) -> Result<(), AppError> {
        self.inner
            .lock()
            .expect("mutex da denylist")
            .insert(jti.to_owned(), expires_at_unix);
        Ok(())
    }
    /// Consulta a revogação do identificador e retorna o estado encontrado ou erro se não for possível verificar a denylist.
    async fn is_revoked(&self, jti: &str) -> Result<bool, AppError> {
        let now = chrono::Utc::now().timestamp();
        let mut guard = self.inner.lock().expect("mutex da denylist");

        // Entradas expiradas não têm efeito: o próprio JWT já é recusado por
        // `exp`. Remover na leitura evita crescimento indefinido.
        guard.retain(|_, expires_at| *expires_at > now);

        Ok(guard.contains_key(jti))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifica o contrato identificado por `revoked_jti_is_reported` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[tokio::test]
    async fn revoked_jti_is_reported() {
        let denylist = MemoryDenylist::new();
        let future = chrono::Utc::now().timestamp() + 600;

        denylist.revoke("abc", future).await.expect("revogar");

        assert!(denylist.is_revoked("abc").await.expect("consultar"));
        assert!(!denylist.is_revoked("outro").await.expect("consultar"));
    }

    /// Verifica o contrato identificado por `expired_entry_is_discarded` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[tokio::test]
    async fn expired_entry_is_discarded() {
        let denylist = MemoryDenylist::new();
        let past = chrono::Utc::now().timestamp() - 1;

        denylist.revoke("abc", past).await.expect("revogar");

        assert!(!denylist.is_revoked("abc").await.expect("consultar"));
        assert!(denylist.is_empty(), "entrada expirada deve sair do mapa");
    }
}
