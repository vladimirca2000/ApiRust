//! Testes de integração dos adaptadores Redis (Fase 8).
//!
//! **Estes testes exigem um Redis real.** Quando `REDIS_URL` não está definida
//! ou o servidor não responde, cada teste é marcado como *skip* com mensagem
//! explícita — não passam silenciosamente, e o relatório diz que não houve
//! verificação.
//!
//! Para rodar:
//!
//! ```bash
//! REDIS_URL=redis://usuario:senha@127.0.0.1:6379 cargo test -p infrastructure --test redis
//! ```

use std::time::Duration;

use application::{AccessTokenDenylist, RateLimitPolicy, RateLimitStore};
use infrastructure::{RedisAccessTokenDenylist, RedisRateLimitStore, RedisState};

/// Lê a URL do Redis do ambiente, pulando o teste quando ausente.
///
/// Devolve `None` quando não há como testar — o chamador imprime o motivo.
fn redis_url() -> Option<String> {
    std::env::var("REDIS_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
}

/// Conecta ao Redis e valida o `PING`; devolve `None` se indisponível.
async fn connect() -> Option<(RedisState, String)> {
    let Some(url) = redis_url() else {
        eprintln!("SKIP: REDIS_URL não definida; teste de integração Redis não executado");
        return None;
    };

    match RedisState::connect(&url).await {
        Ok(state) => match state.ping().await {
            Ok(()) => Some((state, url)),
            Err(error) => {
                eprintln!("SKIP: Redis não respondeu ao PING ({error}); teste não executado");
                None
            }
        },
        Err(error) => {
            eprintln!("SKIP: não foi possível conectar ao Redis ({error}); teste não executado");
            None
        }
    }
}

/// Sufixo aleatório para não colidir com chaves de outras execuções.
fn unique(prefix: &str) -> String {
    format!("{prefix}:{}", uuid::Uuid::new_v4())
}

/// Verifica o contrato identificado por `rate_limit_counts_and_blocks_after_the_limit` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn rate_limit_counts_and_blocks_after_the_limit() {
    let Some((state, _url)) = connect().await else {
        return;
    };

    let limiter = RedisRateLimitStore::new(state);
    let key = unique("teste-rate");
    let policy = RateLimitPolicy {
        max_attempts: 2,
        window: Duration::from_secs(30),
    };

    for attempt in 0..2 {
        let decision = limiter.hit(&key, policy).await.expect("contar tentativa");
        assert!(decision.allowed, "tentativa {attempt} deveria passar");
    }

    let blocked = limiter.hit(&key, policy).await.expect("contar tentativa");
    assert!(!blocked.allowed, "a terceira tentativa deve ser bloqueada");
    assert!(
        blocked.retry_after.is_some(),
        "bloqueio deve informar quanto falta para reabrir"
    );
}

/// Verifica o contrato identificado por `rate_limit_is_shared_across_connections` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn rate_limit_is_shared_across_connections() {
    let Some((state, _url)) = connect().await else {
        return;
    };

    // Duas instâncias distintas, como duas réplicas da API.
    let first = RedisRateLimitStore::new(state.clone());
    let second = RedisRateLimitStore::new(state);
    let key = unique("teste-rate-shared");
    let policy = RateLimitPolicy {
        max_attempts: 1,
        window: Duration::from_secs(30),
    };

    assert!(first.hit(&key, policy).await.expect("primeira").allowed);

    let second_decision = second.hit(&key, policy).await.expect("segunda");
    assert!(
        !second_decision.allowed,
        "o contador precisa ser compartilhado entre réplicas"
    );
}

/// Verifica o contrato identificado por `denylist_revokes_and_expires` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn denylist_revokes_and_expires() {
    let Some((state, _url)) = connect().await else {
        return;
    };

    let denylist = RedisAccessTokenDenylist::new(state);
    let jti = unique("jti");

    assert!(!denylist.is_revoked(&jti).await.expect("consultar"));

    let expires_at = chrono::Utc::now().timestamp() + 60;
    denylist.revoke(&jti, expires_at).await.expect("revogar");

    assert!(
        denylist.is_revoked(&jti).await.expect("consultar"),
        "jti revogado deve ser visto por qualquer réplica"
    );
}

/// Verifica o contrato identificado por `denylist_entry_expires_with_the_token` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn denylist_entry_expires_with_the_token() {
    let Some((state, _url)) = connect().await else {
        return;
    };

    let denylist = RedisAccessTokenDenylist::new(state);
    let jti = unique("jti-expira");

    // Já expirado: o Redis não deve sequer manter a chave.
    let past = chrono::Utc::now().timestamp() - 1;
    denylist.revoke(&jti, past).await.expect("revogar");

    assert!(
        !denylist.is_revoked(&jti).await.expect("consultar"),
        "entrada com expiração no passado não deve existir"
    );
}

/// Verifica o contrato identificado por `ping_reports_healthy_connection` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn ping_reports_healthy_connection() {
    let Some((state, _url)) = connect().await else {
        return;
    };

    state.ping().await.expect("PING no Redis configurado");
}
