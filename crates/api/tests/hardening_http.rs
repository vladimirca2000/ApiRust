//! Testes das camadas de endurecimento: CORS, limite de corpo e headers.
//!
//! Cobrem o último item da Fase 6: origens explícitas (nunca `*`), corpo
//! limitado e headers de segurança presentes em toda resposta.

use std::net::SocketAddr;

use api::AppState;
use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{HeaderValue, Method, Request, StatusCode, header};
use infrastructure::Database;
use tower::ServiceExt;

/// Segredo de teste válido.
const SECRET: &str = "0123456789abcdef0123456789abcdef";

/// Monta o estado com as origens informadas.
async fn state_with_origins(origins: Vec<String>) -> AppState {
    let database = Database::connect_path(":memory:")
        .await
        .expect("banco em memória");
    database.migrate().await.expect("migrations");

    AppState::new(false, database)
        .with_jwt(SECRET, 900)
        .expect("jwt")
        .with_allowed_origins(origins)
}

/// Executa uma requisição e devolve a resposta completa.
async fn send(state: &AppState, request: Request<Body>) -> axum::response::Response {
    let mut request = request;
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 8100))));

    api::router(state.clone())
        .oneshot(request)
        .await
        .expect("resposta")
}

/// GET simples em um caminho.
async fn get(state: &AppState, path: &str) -> axum::response::Response {
    send(
        state,
        Request::builder()
            .uri(path)
            .body(Body::empty())
            .expect("requisição"),
    )
    .await
}

#[tokio::test]
async fn security_headers_are_present_on_success() {
    let state = state_with_origins(Vec::new()).await;
    let response = get(&state, "/health").await;

    assert_eq!(response.status(), StatusCode::OK);

    let headers = response.headers();
    assert_eq!(
        headers.get("x-content-type-options"),
        Some(&HeaderValue::from_static("nosniff"))
    );
    assert_eq!(
        headers.get("x-frame-options"),
        Some(&HeaderValue::from_static("DENY"))
    );
    assert_eq!(
        headers.get("referrer-policy"),
        Some(&HeaderValue::from_static("no-referrer"))
    );
    assert!(headers.contains_key("permissions-policy"));
}

#[tokio::test]
async fn security_headers_are_present_on_error() {
    let state = state_with_origins(Vec::new()).await;
    let response = get(&state, "/nao-existe").await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers().get("x-content-type-options"),
        Some(&HeaderValue::from_static("nosniff")),
        "resposta de erro também precisa dos headers de segurança"
    );
}

#[tokio::test]
async fn cors_denies_unknown_origin_by_default() {
    let state = state_with_origins(Vec::new()).await;

    let response = send(
        &state,
        Request::builder()
            .uri("/health")
            .header(header::ORIGIN, "https://malicioso.example.com")
            .body(Body::empty())
            .expect("requisição"),
    )
    .await;

    assert!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none(),
        "sem origens configuradas, nenhuma origem pode ser liberada"
    );
}

#[tokio::test]
async fn cors_allows_a_configured_origin() {
    let state = state_with_origins(vec!["https://app.exemplo.com".to_owned()]).await;

    let response = send(
        &state,
        Request::builder()
            .uri("/health")
            .header(header::ORIGIN, "https://app.exemplo.com")
            .body(Body::empty())
            .expect("requisição"),
    )
    .await;

    assert_eq!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|v| v.to_str().ok()),
        Some("https://app.exemplo.com")
    );
}

#[tokio::test]
async fn cors_does_not_allow_a_different_origin() {
    let state = state_with_origins(vec!["https://app.exemplo.com".to_owned()]).await;

    let response = send(
        &state,
        Request::builder()
            .uri("/health")
            .header(header::ORIGIN, "https://outro.exemplo.com")
            .body(Body::empty())
            .expect("requisição"),
    )
    .await;

    assert_ne!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|v| v.to_str().ok()),
        Some("https://outro.exemplo.com"),
        "origem não listada não pode ser liberada"
    );
}

#[tokio::test]
async fn preflight_is_answered_for_allowed_origin() {
    let state = state_with_origins(vec!["https://app.exemplo.com".to_owned()]).await;

    let response = send(
        &state,
        Request::builder()
            .method(Method::OPTIONS)
            .uri("/api/v1/users")
            .header(header::ORIGIN, "https://app.exemplo.com")
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
            .header(
                header::ACCESS_CONTROL_REQUEST_HEADERS,
                "authorization, content-type",
            )
            .body(Body::empty())
            .expect("requisição"),
    )
    .await;

    assert!(
        response.status().is_success(),
        "preflight deveria ser aceito, recebido {}",
        response.status()
    );
    assert!(
        response
            .headers()
            .contains_key(header::ACCESS_CONTROL_ALLOW_METHODS)
    );
}

#[tokio::test]
async fn body_over_the_limit_is_rejected() {
    let state = state_with_origins(Vec::new()).await;

    // 1024 * 1024 + 1 byte: exatamente acima do limite.
    let oversized = "a".repeat(api::hardening::MAX_BODY_BYTES + 1);

    let response = send(
        &state,
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/auth/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(oversized))
            .expect("requisição"),
    )
    .await;

    assert_eq!(
        response.status(),
        api::hardening::PAYLOAD_TOO_LARGE,
        "corpo acima do limite deve ser recusado com 413"
    );
}

#[tokio::test]
async fn excess_concurrent_requests_return_503() {
    /// Handler que segura a conexão o suficiente para a segunda requisição
    /// encontrar o limite de concorrência já ocupado.
    async fn hold() -> &'static str {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        "ok"
    }

    let router = axum::Router::new().route("/hold", axum::routing::get(hold));
    let router = api::hardening::apply(router, &[], 1);

    let first = {
        let router = router.clone();
        tokio::spawn(async move {
            router
                .oneshot(
                    Request::builder()
                        .uri("/hold")
                        .body(Body::empty())
                        .expect("requisição"),
                )
                .await
                .expect("resposta")
                .status()
        })
    };

    // Dá tempo do primeiro request adquirir o único slot.
    tokio::time::sleep(std::time::Duration::from_millis(40)).await;

    let second = router
        .oneshot(
            Request::builder()
                .uri("/hold")
                .body(Body::empty())
                .expect("requisição"),
        )
        .await
        .expect("resposta");

    assert_eq!(
        second.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "excesso de concorrência deve ser recusado com 503, não enfileirado"
    );
    assert_eq!(first.await.expect("join"), StatusCode::OK);
}

#[tokio::test]
async fn body_within_the_limit_reaches_the_handler() {
    let state = state_with_origins(Vec::new()).await;

    // Payload válido e pequeno: deve chegar ao handler (401 por credenciais).
    let response = send(
        &state,
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/auth/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"email":"a@b.com","password":"Senha-Forte-2026!"}"#,
            ))
            .expect("requisição"),
    )
    .await;

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "corpo pequeno deve chegar ao handler"
    );
}
