//! Rate limit em memória para desenvolvimento e testes.
//!
//! Protege contra força bruta e *credential stuffing* quando não há Redis
//! configurado. A janela é fixa por chave (IP + rota) e o estado vive no
//! processo: some no restart e **cada réplica conta em separado**. Em produção,
//! use [`infrastructure::RedisRateLimitStore`], que compartilha o contador.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use application::{AppError, RateLimitDecision, RateLimitPolicy, RateLimitStore};
use async_trait::async_trait;

/// Tentativas de login permitidas por janela.
const LOGIN_MAX_ATTEMPTS: u32 = 5;
/// Duração da janela de login, em segundos.
const LOGIN_WINDOW_SECS: Duration = Duration::from_secs(60);
/// Tentativas de refresh permitidas por janela (mais generoso: uma por renovação).
const REFRESH_MAX_ATTEMPTS: u32 = 30;
/// Duração da janela de refresh, em segundos.
const REFRESH_WINDOW_SECS: Duration = Duration::from_secs(60);
/// Requisições de leitura autenticada permitidas por janela, por IP.
///
/// Alto o bastante para um cliente legítimo paginando uma listagem, baixo o
/// bastante para tornar scraping custoso.
const READ_MAX_ATTEMPTS: u32 = 300;
/// Duração da janela de leitura, em segundos.
const READ_WINDOW_SECS: Duration = Duration::from_secs(60);

/// Devolve a política padrão de login: 5 tentativas por minuto.
///
/// Baixo o bastante para inviabilizar força bruta, alto o suficiente para não
/// atrapalhar quem errou a senha algumas vezes.
#[must_use]
pub const fn login_policy() -> RateLimitPolicy {
    RateLimitPolicy {
        max_attempts: LOGIN_MAX_ATTEMPTS,
        window: LOGIN_WINDOW_SECS,
    }
}

/// Política para refresh: mais permissivo, pois é chamado em cada renovação.
#[must_use]
pub const fn refresh_policy() -> RateLimitPolicy {
    RateLimitPolicy {
        max_attempts: REFRESH_MAX_ATTEMPTS,
        window: REFRESH_WINDOW_SECS,
    }
}

/// Política das rotas de leitura autenticada (catálogo e usuários).
///
/// Sem ela, `GET /api/v1/products` é um vetor de scraping sem custo para quem
/// coleta.
#[must_use]
pub const fn read_policy() -> RateLimitPolicy {
    RateLimitPolicy {
        max_attempts: READ_MAX_ATTEMPTS,
        window: READ_WINDOW_SECS,
    }
}

/// Contador de tentativas por chave, com expiração por janela.
#[derive(Clone)]
pub struct RateLimiter {
    /// Estado compartilhado entre clones (cada requisição recebe uma cópia).
    inner: Arc<Mutex<HashMap<String, Window>>>,
}

/// Janela de contagem de uma chave específica.
#[derive(Debug)]
struct Window {
    /// Quantidade de tentativas já registradas.
    attempts: u32,
    /// Momento em que a janela começou.
    started_at: Instant,
}

impl RateLimiter {
    /// Cria um limitador vazio.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Registra uma tentativa e informa se ela é permitida.
    ///
    /// Devolve `Ok(())` quando a tentativa é aceita, ou o tempo restante até a
    /// janela reabrir quando o limite foi atingido.
    ///
    /// # Errors
    ///
    /// Retorna o tempo de espera como erro; nunca falha por indisponibilidade,
    /// já que o estado é local.
    pub fn check(&self, key: &str, policy: RateLimitPolicy, now: Instant) -> Result<(), Duration> {
        let mut guard = self.inner.lock().expect("mutex do rate limit");

        let window = guard.entry(key.to_owned()).or_insert(Window {
            attempts: 0,
            started_at: now,
        });

        // Janela expirada: reinicia a contagem.
        if now.duration_since(window.started_at) >= policy.window {
            window.attempts = 0;
            window.started_at = now;
        }

        if window.attempts >= policy.max_attempts {
            let elapsed = now.duration_since(window.started_at);
            return Err(policy.window.saturating_sub(elapsed));
        }

        window.attempts += 1;
        Ok(())
    }

    /// Descarta janelas já expiradas, evitando crescimento indefinido do mapa.
    ///
    /// Chamado periodicamente (a cada requisição de login) para manter o mapa
    /// pequeno mesmo com muitos IPs distintos.
    pub fn prune(&self, policy: RateLimitPolicy, now: Instant) {
        let mut guard = self.inner.lock().expect("mutex do rate limit");
        guard.retain(|_, window| now.duration_since(window.started_at) < policy.window);
    }

    /// Quantidade de chaves atualmente rastreadas (usado em testes).
    #[must_use]
    pub fn tracked_keys(&self) -> usize {
        self.inner.lock().expect("mutex do rate limit").len()
    }
}

#[async_trait]
impl RateLimitStore for RateLimiter {
    /// Registra uma tentativa para a chave informada e aplica a janela e o limite configurados; retorna a decisão de permitir ou bloquear, ou um erro de infraestrutura.
    async fn hit(&self, key: &str, policy: RateLimitPolicy) -> Result<RateLimitDecision, AppError> {
        Ok(match self.check(key, policy, Instant::now()) {
            Ok(()) => RateLimitDecision::allowed(),
            Err(retry_after) => RateLimitDecision::denied(retry_after),
        })
    }
}

impl Default for RateLimiter {
    /// Equivale a [`RateLimiter::new`].
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Política curta usada para verificar contagem, bloqueio e expiração da janela.
    const POLICY: RateLimitPolicy = RateLimitPolicy {
        max_attempts: 3,
        window: Duration::from_secs(60),
    };

    /// Verifica o contrato identificado por `allows_up_to_the_limit_then_blocks` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn allows_up_to_the_limit_then_blocks() {
        let limiter = RateLimiter::new();
        let start = Instant::now();

        for attempt in 0..POLICY.max_attempts {
            assert!(
                limiter.check("ip-1", POLICY, start).is_ok(),
                "tentativa {attempt} deveria passar"
            );
        }

        let blocked = limiter.check("ip-1", POLICY, start);
        assert!(blocked.is_err(), "a tentativa excedente deve ser bloqueada");
    }

    /// Verifica o contrato identificado por `tracks_keys_independently` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn tracks_keys_independently() {
        let limiter = RateLimiter::new();
        let start = Instant::now();

        for _ in 0..POLICY.max_attempts {
            limiter.check("ip-1", POLICY, start).expect("permitido");
        }

        assert!(
            limiter.check("ip-2", POLICY, start).is_ok(),
            "outro IP não pode ser afetado"
        );
    }

    /// Verifica o contrato identificado por `window_resets_after_expiry` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn window_resets_after_expiry() {
        let limiter = RateLimiter::new();
        let start = Instant::now();

        for _ in 0..POLICY.max_attempts {
            limiter.check("ip-1", POLICY, start).expect("permitido");
        }
        assert!(limiter.check("ip-1", POLICY, start).is_err());

        let later = start + POLICY.window + Duration::from_secs(1);
        assert!(
            limiter.check("ip-1", POLICY, later).is_ok(),
            "após a janela, a contagem deve reiniciar"
        );
    }

    /// Verifica o contrato identificado por `blocked_response_reports_remaining_time` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn blocked_response_reports_remaining_time() {
        let limiter = RateLimiter::new();
        let start = Instant::now();

        for _ in 0..POLICY.max_attempts {
            limiter.check("ip-1", POLICY, start).expect("permitido");
        }

        let remaining = limiter
            .check("ip-1", POLICY, start + Duration::from_secs(15))
            .expect_err("bloqueado");

        assert!(
            remaining <= Duration::from_secs(45),
            "deve indicar o tempo restante da janela"
        );
    }

    /// Verifica o contrato identificado por `prune_removes_expired_windows` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn prune_removes_expired_windows() {
        let limiter = RateLimiter::new();
        let start = Instant::now();

        limiter.check("ip-1", POLICY, start).expect("permitido");
        assert_eq!(limiter.tracked_keys(), 1);

        limiter.prune(POLICY, start + POLICY.window + Duration::from_secs(1));
        assert_eq!(
            limiter.tracked_keys(),
            0,
            "janela expirada deve ser removida"
        );
    }
}
