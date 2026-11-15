//! Testes de configuração e boot da Fase 8.
//!
//! Cobrem as decisões que só se manifestam no processo real: produção exige
//! Redis e `/metrics` só é montado com token.

use api::config::{AppConfig, ConfigError, Environment};

/// Segredo válido para os testes de configuração.
const SECRET: &str = "0123456789abcdef0123456789abcdef";

/// Verifica o contrato identificado por `production_without_redis_is_still_a_valid_config` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn production_without_redis_is_still_a_valid_config() {
    // A exigência de Redis em produção é decidida no boot, não no parse: o
    // `AppConfig` precisa permitir o parse para que o erro cite a variável
    // correta em vez de falhar por um motivo genérico.
    let config = AppConfig::from_map(&[
        ("APP_ENV", "production"),
        ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
        ("JWT_SECRET", SECRET),
    ])
    .expect("configuração válida");

    assert_eq!(config.environment, Environment::Production);
    assert!(
        config.redis_url.is_none(),
        "o parse não injeta Redis; o boot é que exige"
    );
}

/// Verifica o contrato identificado por `metrics_token_is_optional` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn metrics_token_is_optional() {
    let without = AppConfig::from_map(&[
        ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
        ("JWT_SECRET", SECRET),
    ])
    .expect("configuração válida");

    assert!(
        without.metrics_token.is_none(),
        "sem METRICS_TOKEN, /metrics não deve existir"
    );

    let with = AppConfig::from_map(&[
        ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
        ("JWT_SECRET", SECRET),
        ("METRICS_TOKEN", "token-forte-de-metricas"),
    ])
    .expect("configuração válida");

    assert_eq!(
        with.metrics_token.as_deref(),
        Some("token-forte-de-metricas")
    );
}

/// Verifica o contrato identificado por `redis_url_is_read_as_given` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn redis_url_is_read_as_given() {
    let config = AppConfig::from_map(&[
        ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
        ("JWT_SECRET", SECRET),
        ("REDIS_URL", "redis://usuario:senha@127.0.0.1:6379"),
    ])
    .expect("configuração válida");

    assert_eq!(
        config.redis_url.as_deref(),
        Some("redis://usuario:senha@127.0.0.1:6379")
    );
}

/// Verifica o contrato identificado por `empty_metrics_token_is_treated_as_absent` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn empty_metrics_token_is_treated_as_absent() {
    let config = AppConfig::from_map(&[
        ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
        ("JWT_SECRET", SECRET),
        ("METRICS_TOKEN", "   "),
    ])
    .expect("configuração válida");

    assert!(
        config.metrics_token.is_none(),
        "token só com espaços não pode habilitar a rota"
    );
}

/// Verifica o contrato identificado por `invalid_redis_url_shape_is_kept_for_the_boot_to_reject` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn invalid_redis_url_shape_is_kept_for_the_boot_to_reject() {
    // O parse não valida a URL (o crate `redis` faz isso na conexão); o teste
    // documenta que a falha acontece no boot, com mensagem contextual.
    let config = AppConfig::from_map(&[
        ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
        ("JWT_SECRET", SECRET),
        ("REDIS_URL", "não-é-uma-url"),
    ])
    .expect("o parse aceita qualquer texto");

    assert_eq!(config.redis_url.as_deref(), Some("não-é-uma-url"));
}

/// Verifica o contrato identificado por `short_jwt_secret_is_rejected_before_redis_matters` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn short_jwt_secret_is_rejected_before_redis_matters() {
    let error = AppConfig::from_map(&[
        ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
        ("JWT_SECRET", "curto"),
        ("REDIS_URL", "redis://127.0.0.1:6379"),
    ])
    .expect_err("segredo curto deve falhar");

    assert!(matches!(
        error,
        ConfigError::Invalid {
            key: "JWT_SECRET",
            ..
        }
    ));
}
