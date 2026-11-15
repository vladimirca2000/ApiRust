use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::Notify;

/// Tempo de tolerância após a última aba desconectar, para permitir refresh.
const CLOSE_GRACE: Duration = Duration::from_secs(10);

/// Rastreia conexões de sessão do navegador e solicita shutdown quando
/// a última aba é fechada.
///
/// O mecanismo é desativado por padrão e nunca dispara quando `enabled` é falso.
#[derive(Clone)]
pub struct SessionTracker {
    /// Estado compartilhado entre todas as conexões e clones do tracker.
    inner: Arc<Inner>,
}

/// Estado interno compartilhado, mantido atrás de `Arc` para ser clonável.
struct Inner {
    /// Se falso, nada é contado e `wait_for_shutdown` nunca resolve.
    enabled: bool,
    /// Janela de tolerância após a última aba sair, para permitir refresh.
    grace: Duration,
    /// Quantidade de abas conectadas neste momento.
    active: AtomicUsize,
    /// Indica que ao menos uma aba já se conectou nesta execução.
    ever_connected: AtomicBool,
    /// Notificação enviada quando o servidor deve encerrar.
    shutdown: Notify,
}

impl SessionTracker {
    /// Cria o tracker com a tolerância padrão de 10 segundos.
    pub fn new(enabled: bool) -> Self {
        Self::with_grace(enabled, CLOSE_GRACE)
    }

    /// Cria o tracker com uma janela de tolerância explícita.
    ///
    /// Existe separado de [`SessionTracker::new`] para que os testes usem uma
    /// janela curta em vez de esperar 10 segundos.
    fn with_grace(enabled: bool, grace: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                enabled,
                grace,
                active: AtomicUsize::new(0),
                ever_connected: AtomicBool::new(false),
                shutdown: Notify::new(),
            }),
        }
    }

    /// Indica se o modo "fechar a aba encerra o servidor" está ativo.
    pub fn is_enabled(&self) -> bool {
        self.inner.enabled
    }

    /// Registra uma nova sessão de navegador. A sessão é encerrada quando a
    /// guarda devolvida é descartada.
    pub fn connection(&self) -> SessionConnection {
        if self.inner.enabled {
            let active = self.inner.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.inner.ever_connected.store(true, Ordering::SeqCst);
            tracing::debug!(active, "sessão de navegador conectada");
        }

        SessionConnection {
            tracker: self.clone(),
        }
    }

    /// Resolve quando a última aba foi fechada, ou fica pendente para sempre
    /// quando o mecanismo está desativado.
    pub async fn wait_for_shutdown(&self) {
        if !self.inner.enabled {
            std::future::pending::<()>().await;
        }

        self.inner.shutdown.notified().await;
    }

    /// Contabiliza a saída de uma aba e agenda o shutdown se foi a última.
    ///
    /// A verificação `previous > 1` ignora desconexões que não zeraram a
    /// contagem, e `ever_connected` evita encerrar um servidor que nunca
    /// recebeu visita (por exemplo, ao expirar uma conexão inicial).
    fn disconnected(&self) {
        if !self.inner.enabled {
            return;
        }

        let previous = self.inner.active.fetch_sub(1, Ordering::SeqCst);
        if previous > 1 || !self.inner.ever_connected.load(Ordering::SeqCst) {
            return;
        }

        let tracker = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tracker.inner.grace).await;
            if tracker.inner.active.load(Ordering::SeqCst) == 0 {
                tracing::info!("nenhuma aba conectada; encerrando servidor");
                tracker.inner.shutdown.notify_one();
            }
        });
    }
}

/// Guarda de uma sessão ativa. Descartar (fechar a aba) dispara a contagem
/// para o shutdown.
pub struct SessionConnection {
    /// Tracker que será decrementado quando esta guarda for descartada.
    tracker: SessionTracker,
}

impl Drop for SessionConnection {
    /// Notifica o tracker de que a aba saiu, disparando a contagem de shutdown.
    fn drop(&mut self) {
        self.tracker.disconnected();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_tracker_never_requests_shutdown() {
        let tracker = SessionTracker::new(false);
        let connection = tracker.connection();
        drop(connection);

        let result =
            tokio::time::timeout(Duration::from_millis(50), tracker.wait_for_shutdown()).await;
        assert!(
            result.is_err(),
            "tracker desativado não deve sinalizar shutdown"
        );
    }

    #[tokio::test]
    async fn shutdown_fires_after_last_connection_drops() {
        let tracker = SessionTracker::with_grace(true, Duration::from_millis(50));
        let first = tracker.connection();
        let second = tracker.connection();

        drop(first);
        let during_grace =
            tokio::time::timeout(Duration::from_millis(20), tracker.wait_for_shutdown()).await;
        assert!(during_grace.is_err(), "ainda há uma aba conectada");

        drop(second);
        let shutdown =
            tokio::time::timeout(Duration::from_secs(2), tracker.wait_for_shutdown()).await;
        assert!(
            shutdown.is_ok(),
            "shutdown deve ocorrer após o grace period"
        );
    }
}
