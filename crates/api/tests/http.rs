//! Testes HTTP/E2E do roteador montado em memória (sem abrir porta).
//!
//! Cobre `/health`, `/ready` (inclusive com banco indisponível), propagação de
//! request ID, 404 sem content-type e presença condicional das rotas de
//! desenvolvimento.

use api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use infrastructure::Database;
use tower::ServiceExt;

async fn test_state(shutdown_on_tab_close: bool) -> AppState {
    let database = Database::connect_path(":memory:")
        .await
        .expect("banco em memória");
    database.migrate().await.expect("migrations");
    AppState::new(shutdown_on_tab_close, database)
}

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

#[tokio::test]
async fn health_reports_ok() {
    let (status, body, _) = get("/health").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn ready_reports_ready_with_database_up() {
    let (status, body, _) = get("/ready").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ready");
}

#[tokio::test]
async fn ready_reports_unavailable_when_database_is_down() {
    let database = Database::connect_path(":memory:")
        .await
        .expect("banco em memória");
    let state = AppState::new(false, database.clone());
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

#[tokio::test]
async fn propagates_request_id() {
    let (_, _, request_id) = get("/health").await;

    assert!(request_id.is_some_and(|value| !value.is_empty()));
}

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

#[tokio::test]
async fn session_page_is_absent_when_disabled() {
    let response = api::router(test_state(false).await)
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

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
