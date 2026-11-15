//! Testes dos achados da análise de QA: guarda do construtor de produção,
//! resolução de IP atrás de proxy, paginação estrita e logout sem Bearer.
//!
//! Cada teste aqui corresponde a um item aberto de `docs/analise-qa.md`; se a
//! regressão voltar, a suíte aponta o achado.

use std::net::{IpAddr, SocketAddr};

use api::AppState;
use application::{DEFAULT_PAGE_SIZE, normalize_page};
use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

/// URL de banco usada pelos testes.
const DATABASE_ENV: &str = "TEST_DATABASE_URL";

// ---------------------------------------------------------------------------
// A8 — o construtor de teste nunca pode chegar a produção
// ---------------------------------------------------------------------------

/// Verifica o contrato identificado por `production_bootstrap_does_not_use_the_test_constructor` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn production_bootstrap_does_not_use_the_test_constructor() {
    // Guarda textual: `main.rs` é o único caminho de boot. Se alguém trocar
    // `from_parts` por `for_tests`, o segredo descartável e o Swagger ligado
    // iriam para produção — e nada mais quebraria.
    let main_source = include_str!("../src/main.rs");

    assert!(
        main_source.contains("AppState::from_parts("),
        "o bootstrap deve montar o estado com o construtor de produção"
    );
    assert!(
        !main_source.contains("AppState::for_tests("),
        "o bootstrap jamais pode usar o construtor de teste"
    );
    assert!(
        !main_source.contains("AppState::new("),
        "o construtor neutro foi removido de propósito: ele não indicava o risco"
    );
}

// ---------------------------------------------------------------------------
// A6 — paginação recusa valores inválidos em vez de corrigir em silêncio
// ---------------------------------------------------------------------------

/// Verifica o contrato identificado por `pagination_rejects_limit_outside_the_allowed_range` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn pagination_rejects_limit_outside_the_allowed_range() {
    for invalid in [0_i64, -1, 101, 10_000] {
        let error =
            normalize_page(0, invalid).expect_err("limit fora de 1..=100 precisa ser recusado");
        assert!(
            matches!(error, application::AppError::InvalidPagination(_)),
            "esperado InvalidPagination para limit={invalid}, veio {error:?}"
        );
    }
}

/// Verifica o contrato identificado por `pagination_accepts_the_full_valid_range` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn pagination_accepts_the_full_valid_range() {
    assert_eq!(normalize_page(0, 1).expect("mínimo"), (0, 1));
    assert_eq!(normalize_page(0, 100).expect("máximo"), (0, 100));
    assert_eq!(
        normalize_page(0, DEFAULT_PAGE_SIZE).expect("padrão"),
        (0, DEFAULT_PAGE_SIZE)
    );
}

/// Verifica o contrato identificado por `pagination_still_clamps_negative_offset` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn pagination_still_clamps_negative_offset() {
    // `offset` negativo é ajustado: não há ambiguidade sobre o que o cliente
    // pediu, e o valor 0 é o único coerente.
    assert_eq!(normalize_page(-50, 20).expect("offset ajustado"), (0, 20));
}

// ---------------------------------------------------------------------------
// A2 — IP do cliente atrás de proxy confiável
// ---------------------------------------------------------------------------

/// Verifica o contrato identificado por `forwarded_header_is_ignored_from_untrusted_peer` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn forwarded_header_is_ignored_from_untrusted_peer() {
    let peer: IpAddr = "198.51.100.9".parse().expect("IP válido");
    let proxy: IpAddr = "10.0.0.2".parse().expect("IP válido");

    let resolved = api::client_ip::resolve_client_ip(peer, &[proxy], Some("1.2.3.4"));

    assert_eq!(
        resolved, peer,
        "cliente pode forjar X-Forwarded-For; de peer não confiável ele é ignorado"
    );
}

/// Verifica o contrato identificado por `forwarded_header_is_used_from_trusted_peer` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn forwarded_header_is_used_from_trusted_peer() {
    let peer: IpAddr = "10.0.0.2".parse().expect("IP válido");
    let client: IpAddr = "203.0.113.7".parse().expect("IP válido");

    let resolved = api::client_ip::resolve_client_ip(peer, &[peer], Some("203.0.113.7"));

    assert_eq!(resolved, client, "proxy confiável informa o IP real");
}

/// Verifica o contrato identificado por `forged_hop_to_the_left_is_not_used` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[test]
fn forged_hop_to_the_left_is_not_used() {
    let proxy: IpAddr = "10.0.0.2".parse().expect("IP válido");
    let client: IpAddr = "203.0.113.7".parse().expect("IP válido");

    // O cliente enviou "9.9.9.9" e o proxy anexou o IP real à direita.
    let resolved = api::client_ip::resolve_client_ip(proxy, &[proxy], Some("9.9.9.9, 203.0.113.7"));

    assert_eq!(
        resolved, client,
        "o último salto não confiável é o único em que se pode confiar"
    );
}

// ---------------------------------------------------------------------------
// A4 — logout sem Bearer ainda revoga o refresh apresentado
// ---------------------------------------------------------------------------

/// Segredo dos tokens usados neste arquivo.
const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";
/// Senha do usuário de teste.
const PASSWORD: &str = "Senha-Forte-2026!";

/// Prepara um estado com um usuário ativo.
///
/// # Panics
///
/// Falha quando o banco de teste não está disponível.
async fn state_with_user() -> AppState {
    use application::{PasswordHasher, UserRepository};
    use chrono::Utc;
    use domain::{Email, Role, User};
    use infrastructure::{Argon2PasswordHasher, SqlxUserRepository};

    let url = std::env::var(DATABASE_ENV)
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| panic!("defina {DATABASE_ENV} para rodar este teste"));

    let schema = format!("t_qa_{}_{}", std::process::id(), unique_suffix());
    let database = infrastructure::Database::connect_isolated(&url, &schema)
        .await
        .expect("preparar schema");
    database.migrate().await.expect("migrations");

    let repository = SqlxUserRepository::new(database.pool().clone());
    let hasher = Argon2PasswordHasher::new();
    let hash = hasher.hash(PASSWORD).await.expect("hash");

    let user = User::new(
        "QA",
        Email::parse("qa@example.com").expect("e-mail"),
        hash,
        Role::User,
        Utc::now(),
    )
    .expect("usuário");
    repository.insert(&user, None).await.expect("inserir");

    AppState::for_tests(false, database)
        .with_jwt(SECRET, 900)
        .expect("jwt")
}

/// Sufixo único para o nome do schema.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("relógio")
        .as_nanos()
}

/// Envia uma requisição e devolve status e corpo.
async fn call(
    state: &AppState,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }

    let payload = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };

    let mut request = builder.body(payload).expect("requisição");
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 6100))));

    let response = api::router(state.clone())
        .oneshot(request)
        .await
        .expect("resposta");

    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("corpo")
        .to_bytes();

    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Verifica o contrato identificado por `logout_without_bearer_revokes_the_presented_refresh_token` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn logout_without_bearer_revokes_the_presented_refresh_token() {
    let state = state_with_user().await;

    let (status, body) = call(
        &state,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({"email": "qa@example.com", "password": PASSWORD})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login: {body}");
    let refresh = body["refresh_token"].as_str().expect("refresh").to_owned();

    // Sem `Authorization`: apenas o refresh do corpo é revogado.
    let (status, _) = call(
        &state,
        "POST",
        "/api/v1/auth/logout",
        None,
        Some(json!({ "refresh_token": refresh })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // O refresh revogado não renova mais.
    let (status, body) = call(
        &state,
        "POST",
        "/api/v1/auth/refresh",
        None,
        Some(json!({ "refresh_token": refresh })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "refresh revogado precisa ser recusado: {body}"
    );
}

/// Verifica o contrato identificado por `invalid_limit_returns_400_with_a_helpful_message` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn invalid_limit_returns_400_with_a_helpful_message() {
    let state = state_with_user().await;

    let (token_status, login) = call(
        &state,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({"email": "qa@example.com", "password": PASSWORD})),
    )
    .await;
    assert_eq!(token_status, StatusCode::OK);
    let token = login["access_token"].as_str().expect("access").to_owned();

    let (status, body) = call(
        &state,
        "GET",
        "/api/v1/products?limit=0",
        Some(&token),
        None,
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "limit=0 deve ser 400: {body}"
    );
    assert_eq!(body["title"], "invalid_request");
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or("")
            .contains("limit deve estar entre"),
        "a mensagem deve apontar o parâmetro: {body}"
    );
}
