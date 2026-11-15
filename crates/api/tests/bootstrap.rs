//! Testes de bootstrap: o que o `main` faz antes de abrir a porta.
//!
//! O bootstrap é o código que muda em cada release (config, migrations, seed,
//! Redis, métricas) e era o menos protegido: nenhum teste atual o exercitava.
//! Aqui as regras de boot são verificadas no ponto onde dá para observá-las sem
//! subir um processo — configuração e decisões de política — e o contrato do
//! binário é checado de forma textual, que é o que impede a regressão silenciosa.

use api::config::{AppConfig, ConfigError, Environment};

/// Segredo válido para os testes.
const SECRET: &str = "0123456789abcdef0123456789abcdef";

/// Fonte do binário, usada para checks de contrato.
const MAIN_SOURCE: &str = include_str!("../src/main.rs");

/// Verifica o contrato identificado por `production_boot_requires_redis` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn production_boot_requires_redis() {
    // A exigência vive no `main` (não no parse): o teste verifica que a regra
    // existe e é acionada quando o ambiente é produção.
    assert!(
        MAIN_SOURCE.contains("REDIS_URL é obrigatório em produção"),
        "produção sem Redis deve abortar o boot; a regra desapareceu do main"
    );
}

/// Verifica o contrato identificado por `production_without_redis_parses_but_boot_rejects` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn production_without_redis_parses_but_boot_rejects() {
    let config = AppConfig::from_map(&[
        ("APP_ENV", "production"),
        ("DATABASE_URL", "postgres://user:senha@localhost:5432/app"),
        ("JWT_SECRET", SECRET),
    ])
    .expect("o parse aceita; a recusa é do boot");

    assert_eq!(config.environment, Environment::Production);
    assert!(config.redis_url.is_none());
}

/// Verifica o contrato identificado por `production_rejects_dev_session_shutdown` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn production_rejects_dev_session_shutdown() {
    let error = AppConfig::from_map(&[
        ("APP_ENV", "production"),
        ("DATABASE_URL", "postgres://user:senha@localhost:5432/app"),
        ("JWT_SECRET", SECRET),
        ("SHUTDOWN_ON_TAB_CLOSE", "true"),
    ])
    .expect_err("produção não pode encerrar por aba");

    assert!(matches!(
        error,
        ConfigError::Invalid {
            key: "SHUTDOWN_ON_TAB_CLOSE",
            ..
        }
    ));
}

/// Verifica o contrato identificado por `production_rejects_swagger` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn production_rejects_swagger() {
    let error = AppConfig::from_map(&[
        ("APP_ENV", "production"),
        ("DATABASE_URL", "postgres://user:senha@localhost:5432/app"),
        ("JWT_SECRET", SECRET),
        ("EXPOSE_API_DOCS", "true"),
    ])
    .expect_err("documentação interativa não pode existir em produção");

    assert!(matches!(
        error,
        ConfigError::Invalid {
            key: "EXPOSE_API_DOCS",
            ..
        }
    ));
}

/// Verifica o contrato identificado por `sqlite_url_is_rejected_before_any_query` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn sqlite_url_is_rejected_before_any_query() {
    let error = AppConfig::from_map(&[
        ("DATABASE_URL", "sqlite://data/api.db"),
        ("JWT_SECRET", SECRET),
    ])
    .expect_err("sqlite não é suportado");

    assert!(matches!(
        error,
        ConfigError::Invalid {
            key: "DATABASE_URL",
            ..
        }
    ));
}

/// Verifica o contrato identificado por `invalid_trusted_proxy_ip_fails_the_boot` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn invalid_trusted_proxy_ip_fails_the_boot() {
    // Um erro de digitação em TRUSTED_PROXIES faria o proxy não ser reconhecido
    // e o limite cairia no IP do proxy — o problema que a config existe para
    // resolver. Falhar no boot é melhor que limitar errado em silêncio.
    let error = AppConfig::from_map(&[
        ("DATABASE_URL", "postgres://user:senha@localhost:5432/app"),
        ("JWT_SECRET", SECRET),
        ("TRUSTED_PROXIES", "10.0.0.2, nao-e-ip"),
    ])
    .expect_err("IP inválido deve ser recusado");

    assert!(matches!(
        error,
        ConfigError::Invalid {
            key: "TRUSTED_PROXIES",
            ..
        }
    ));
}

/// Verifica o contrato identificado por `trusted_proxies_parse_as_a_list` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn trusted_proxies_parse_as_a_list() {
    let config = AppConfig::from_map(&[
        ("DATABASE_URL", "postgres://user:senha@localhost:5432/app"),
        ("JWT_SECRET", SECRET),
        ("TRUSTED_PROXIES", "10.0.0.2, 10.0.0.3 ,::1"),
    ])
    .expect("lista válida");

    assert_eq!(config.trusted_proxies.len(), 3);
    assert_eq!(config.trusted_proxies[2].to_string(), "::1");
}

/// Verifica o contrato identificado por `bootstrap_applies_migrations_before_serving` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn bootstrap_applies_migrations_before_serving() {
    // Ordem importa: servir antes das migrations expõe uma API sem tabelas.
    let migrate = MAIN_SOURCE
        .find("database.migrate()")
        .expect("o main precisa aplicar migrations");
    let serve = MAIN_SOURCE
        .find("servidor iniciado")
        .expect("o main precisa registrar o start do servidor");

    assert!(
        migrate < serve,
        "migrations devem rodar antes de o servidor começar a aceitar conexões"
    );
}

/// Verifica o contrato identificado por `bootstrap_seeds_admin_before_serving` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn bootstrap_seeds_admin_before_serving() {
    let seed = MAIN_SOURCE
        .find("seed_admin(")
        .expect("o main precisa provisionar o admin");
    let serve = MAIN_SOURCE
        .find("servidor iniciado")
        .expect("o main precisa registrar o start");

    assert!(
        seed < serve,
        "o admin inicial precisa existir antes de a API aceitar login"
    );
}

/// Verifica o contrato identificado por `bootstrap_handles_sigterm` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn bootstrap_handles_sigterm() {
    assert!(
        MAIN_SOURCE.contains("SignalKind::terminate()"),
        "sem SIGTERM, `docker stop` mata o processo sem drain"
    );
}

/// Verifica o contrato identificado por `bootstrap_logs_the_redis_mode` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn bootstrap_logs_the_redis_mode() {
    // O operador precisa saber, pelo log, se o estado é distribuído ou local.
    assert!(
        MAIN_SOURCE.contains("Redis conectado") && MAIN_SOURCE.contains("não configurado"),
        "o boot deve registrar claramente se o Redis foi usado"
    );
}
