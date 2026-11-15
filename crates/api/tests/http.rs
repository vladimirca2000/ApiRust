//! Testes HTTP/E2E do roteador montado em memória (sem abrir porta).
//!
//! Cobre `/health`, `/ready` (inclusive com banco indisponível), propagação de
//! request ID, 404 sem content-type e presença condicional das rotas de
//! desenvolvimento.

mod common;

use api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use common::isolated_database;
use http_body_util::BodyExt;
use tower::ServiceExt;
/// Monta e retorna um `AppState` de teste conectado ao banco isolado.
async fn test_state(shutdown_on_tab_close: bool) -> AppState {
    let database = isolated_database("teste").await;
    AppState::for_tests(shutdown_on_tab_close, database)
}
/// Executa uma requisição GET no roteador de teste e retorna status, JSON e request ID da resposta.
async fn get(uri: &str) -> (StatusCode, serde_json::Value, Option<String>) {
    let response = api::router(test_state(false).await)
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();

    let status = response.status();
    let request_id = response
        .headers()
        .get("x-request-id")
        .map(|value| value.to_str().unwrap().to_owned());
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap();

    (status, json, request_id)
}

/// Verifica o contrato identificado por `health_reports_ok` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn health_reports_ok() {
    let (status, body, _) = get("/health").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

/// Verifica o contrato identificado por `ready_reports_ready_with_database_up` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn ready_reports_ready_with_database_up() {
    let (status, body, _) = get("/ready").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ready");
}

/// Verifica o contrato identificado por `ready_reports_unavailable_when_database_is_down` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn ready_reports_unavailable_when_database_is_down() {
    let database = isolated_database("teste").await;
    let state = AppState::for_tests(false, database.clone());
    database.close().await;

    let response = api::router(state)
        .oneshot(
            Request::builder()
                .uri("/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();

    assert_eq!(json["status"], "unavailable");
    assert!(
        json.get("detail").is_none(),
        "/ready não pode vazar a causa interna"
    );
    assert!(
        !text.to_ascii_lowercase().contains("sqlite")
            && !text.to_ascii_lowercase().contains("sqlx")
            && !text.contains("error"),
        "/ready não pode incluir detalhes de infraestrutura: {text}"
    );
}

/// Verifica o contrato identificado por `propagates_request_id` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn propagates_request_id() {
    let (_, _, request_id) = get("/health").await;

    assert!(request_id.is_some_and(|value| !value.is_empty()));
}

/// Verifica o contrato identificado por `unknown_route_returns_not_found` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn unknown_route_returns_not_found() {
    let response = api::router(test_state(false).await)
        .oneshot(
            Request::builder()
                .uri("/desconhecida")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(
        response.headers().get(header::CONTENT_TYPE).is_none(),
        "404 não deve alegar um content-type de sucesso"
    );
}

/// Verifica o contrato identificado por `session_page_is_absent_when_disabled` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn session_page_is_absent_when_disabled() {
    let response = api::router(test_state(false).await)
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Verifica o contrato identificado por `session_page_is_served_when_enabled` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn session_page_is_served_when_enabled() {
    let response = api::router(test_state(true).await)
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("EventSource"));
    assert!(html.contains("/api/dev/session"));
}
