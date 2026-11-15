//! Orquestração de operação: shutdown gracioso e timeout de drain.
//!
//! Depois do sinal (`Ctrl+C` ou fechamento da aba), o servidor para de aceitar
//! conexões novas e espera as requisições em voo. Se elas não terminarem no
//! prazo, o future de *force* resolve e o `select!` do bootstrap aborta o
//! restante — evita um handler travado impedir o encerramento.

use std::future::Future;
use std::time::Duration;

/// Tempo padrão para requisições em voo terminarem após o sinal de shutdown.
pub const DEFAULT_SHUTDOWN_DRAIN: Duration = Duration::from_secs(30);

/// Tempo máximo que `/ready` espera o banco responder.
///
/// Um `SELECT 1` que não volta em 2 s é tratado como dependência indisponível:
/// o load balancer tira a instância de rotação em vez de ficar pendurado.
pub const READY_PING_TIMEOUT: Duration = Duration::from_secs(2);

/// Número padrão de requisições HTTP simultâneas aceitas.
///
/// O pool SQLite tem no máximo 5 conexões; o teto HTTP é maior para caber
/// health checks e leituras curtas, e o excesso é recusado com `503` em vez de
/// enfileirar até estourar memória.
pub const DEFAULT_MAX_CONCURRENT_REQUESTS: usize = 128;

/// Aguarda o sinal de shutdown e, em seguida, o prazo de drain.
///
/// Usado em paralelo com `axum::serve(...).with_graceful_shutdown(signal)`:
/// quando este future resolve, o `select!` cancela o servidor e as conexões
/// restantes são abortadas.
///
/// Não retorna valor; o efeito é só completar após `signal` + `drain`.
pub async fn force_after_drain(signal: impl Future<Output = ()>, drain: Duration) {
    signal.await;
    tokio::time::sleep(drain).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Garante que o *force* só dispara depois do sinal **e** do drain.
    #[tokio::test]
    async fn force_after_drain_waits_for_signal_then_drain() {
        let start = Instant::now();
        force_after_drain(async {}, Duration::from_millis(50)).await;

        assert!(
            start.elapsed() >= Duration::from_millis(50),
            "deveria esperar o prazo de drain"
        );
    }

    /// Garante que um sinal que nunca chega impede o *force* de disparar.
    #[tokio::test]
    async fn force_after_drain_does_not_fire_before_the_signal() {
        let result = tokio::time::timeout(
            Duration::from_millis(40),
            force_after_drain(std::future::pending::<()>(), Duration::from_millis(1)),
        )
        .await;

        assert!(
            result.is_err(),
            "sem sinal de shutdown o drain não deve começar"
        );
    }
}
