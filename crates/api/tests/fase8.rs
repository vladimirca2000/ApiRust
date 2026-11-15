//! Testes HTTP da Fase 8: histórico de estoque/preço, trilha de auditoria,
//! revogação de access token no logout e endpoint `/metrics`.
//!
//! Cobrem o critério de aceite: um saldo que muda deixa rastro, uma alteração
//! de papel é atribuível, o logout invalida o access token e `/metrics` não
//! existe sem token.

mod common;

use common::isolated_database;
use std::net::SocketAddr;

use api::AppState;
use application::{PasswordHasher, UserRepository};
use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use chrono::Utc;
use domain::{Email, Role, User};
use http_body_util::BodyExt;
use infrastructure::{Argon2PasswordHasher, SqlxUserRepository};
use serde_json::{Value, json};
use tower::ServiceExt;

/// Segredo dos tokens de teste.
const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";
/// Senha dos usuários de teste.
const PASSWORD: &str = "Senha-Forte-2026!";
/// Token exigido por `/metrics` nos testes.
const METRICS_TOKEN: &str = "token-de-metricas-de-teste";

/// Ambiente com Admin e User autenticados.
struct Env {
    /// Estado da API.
    state: AppState,
    /// Access token do Admin.
    admin_token: String,
    /// Access token do usuário comum.
    user_token: String,
}

/// Monta o ambiente com métricas habilitadas e dois usuários autenticados.
async fn env() -> Env {
    let database = isolated_database("teste").await;

    let repository = SqlxUserRepository::new(database.pool().clone());
    let hasher = Argon2PasswordHasher::new();

    for (name, email, role) in [
        ("Admin", "admin@example.com", Role::Admin),
        ("User", "user@example.com", Role::User),
    ] {
        let hash = hasher.hash(PASSWORD).await.expect("hash");
        let user = User::new(
            name,
            Email::parse(email).expect("e-mail"),
            hash,
            role,
            Utc::now(),
        )
        .expect("usuário");
        repository.insert(&user, None).await.expect("inserir");
    }

    let state = AppState::for_tests(false, database)
        .with_jwt(SECRET, 900)
        .expect("jwt")
        .with_metrics(Some(METRICS_TOKEN.to_owned()))
        .expect("métricas");

    let admin_token = do_login(&state, "admin@example.com").await;
    let user_token = do_login(&state, "user@example.com").await;

    Env {
        state,
        admin_token,
        user_token,
    }
}

/// Faz login e devolve o access token.
async fn do_login(state: &AppState, email: &str) -> String {
    let (status, body) = call(
        state,
        "POST",
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({"email": email, "password": PASSWORD})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login de {email}");
    body["access_token"]
        .as_str()
        .expect("access_token")
        .to_owned()
}

/// Envia uma requisição, retornando status, corpo JSON e headers.
async fn call(
    state: &AppState,
    method: &str,
    path: &str,
    token: Option<&str>,
    extra_headers: Option<Vec<(&str, &str)>>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);

    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    for (name, value) in extra_headers.unwrap_or_default() {
        builder = builder.header(name, value);
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
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 7000))));

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
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);

    (status, json)
}

/// Cria uma categoria de teste e devolve o id.
async fn create_category(state: &AppState, token: &str, name: &str) -> String {
    let (status, body) = call(
        state,
        "POST",
        "/api/v1/categories",
        Some(token),
        None,
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "criar categoria");

    body["id"].as_str().expect("id").to_owned()
}

/// Cria um produto de teste e devolve o corpo completo.
async fn create_product(
    state: &AppState,
    token: &str,
    sku: &str,
    price_cents: i64,
    stock: i64,
    category_id: &str,
) -> Value {
    let (status, body) = call(
        state,
        "POST",
        "/api/v1/products",
        Some(token),
        None,
        Some(json!({
            "name": "Produto",
            "sku": sku,
            "price_cents": price_cents,
            "stock": stock,
            "category_id": category_id,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "criar produto");

    body
}

/// Verifica o contrato identificado por `creating_a_product_with_stock_records_an_initial_movement` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn creating_a_product_with_stock_records_an_initial_movement() {
    let env = env().await;
    let category = create_category(&env.state, &env.admin_token, "Eletrônicos").await;
    let product = create_product(&env.state, &env.admin_token, "HIST-1", 1500, 7, &category).await;
    let id = product["id"].as_str().expect("id");

    let (status, _body) = call(
        &env.state,
        "GET",
        &format!("/api/v1/products/{id}/stock-movements"),
        Some(&env.user_token),
        None,
        None,
    )
    .await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "usuário comum não pode ver histórico de estoque"
    );

    let (status, body) = call(
        &env.state,
        "GET",
        &format!("/api/v1/products/{id}/stock-movements"),
        Some(&env.admin_token),
        None,
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["delta"], 7);
    assert_eq!(body["items"][0]["reason"], "initial");
}

/// Verifica o contrato identificado por `updating_stock_records_the_delta_and_the_price_change` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn updating_stock_records_the_delta_and_the_price_change() {
    let env = env().await;
    let category = create_category(&env.state, &env.admin_token, "Livros").await;
    let product = create_product(&env.state, &env.admin_token, "HIST-2", 2000, 10, &category).await;
    let id = product["id"].as_str().expect("id");

    let (status, _) = call(
        &env.state,
        "PATCH",
        &format!("/api/v1/products/{id}"),
        Some(&env.admin_token),
        None,
        Some(json!({ "stock": 4, "price_cents": 2500 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call(
        &env.state,
        "GET",
        &format!("/api/v1/products/{id}/stock-movements"),
        Some(&env.admin_token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 2, "inicial + ajuste");
    assert_eq!(
        body["items"][0]["delta"], -6,
        "o mais recente deve ser o delta -6 (10 → 4)"
    );
    assert_eq!(body["items"][0]["reason"], "correction");

    let (status, body) = call(
        &env.state,
        "GET",
        &format!("/api/v1/products/{id}/price-changes"),
        Some(&env.admin_token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["old_cents"], 2000);
    assert_eq!(body["items"][0]["new_cents"], 2500);
}

/// Verifica o contrato identificado por `updating_without_change_does_not_write_history` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn updating_without_change_does_not_write_history() {
    let env = env().await;
    let category = create_category(&env.state, &env.admin_token, "Móveis").await;
    let product = create_product(&env.state, &env.admin_token, "HIST-3", 3000, 5, &category).await;
    let id = product["id"].as_str().expect("id");

    // Mesmo preço e mesmo estoque: nada mudou, nada entra no histórico.
    let (status, _) = call(
        &env.state,
        "PATCH",
        &format!("/api/v1/products/{id}"),
        Some(&env.admin_token),
        None,
        Some(json!({ "stock": 5, "price_cents": 3000, "name": "Renomeado" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, stock) = call(
        &env.state,
        "GET",
        &format!("/api/v1/products/{id}/stock-movements"),
        Some(&env.admin_token),
        None,
        None,
    )
    .await;
    let (_, price) = call(
        &env.state,
        "GET",
        &format!("/api/v1/products/{id}/price-changes"),
        Some(&env.admin_token),
        None,
        None,
    )
    .await;

    assert_eq!(stock["total"], 1, "só a movimentação inicial");
    assert_eq!(price["total"], 0, "nenhuma alteração de preço");
}

/// Verifica o contrato identificado por `audit_records_who_changed_a_user_role` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn audit_records_who_changed_a_user_role() {
    let env = env().await;

    // Cria um segundo usuário para promover — Admin não altera o próprio papel.
    let (status, created) = call(
        &env.state,
        "POST",
        "/api/v1/users",
        Some(&env.admin_token),
        None,
        Some(json!({
            "name": "Novo",
            "email": "novo@example.com",
            "password": "Senha-Forte-2026!",
            "role": "user",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = created["id"].as_str().expect("id");

    let (status, _) = call(
        &env.state,
        "PATCH",
        &format!("/api/v1/users/{id}/role"),
        Some(&env.admin_token),
        None,
        Some(json!({ "role": "manager" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call(
        &env.state,
        "GET",
        "/api/v1/audit?entity=user",
        Some(&env.admin_token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let change_role = body["items"]
        .as_array()
        .expect("itens")
        .iter()
        .find(|entry| entry["action"] == "change_role")
        .expect("entrada de mudança de papel");

    assert_eq!(change_role["entity"], "user");
    assert_eq!(change_role["entity_id"], id);
    assert!(change_role["actor_id"].is_string(), "autor registrado");
    assert_eq!(change_role["before"]["role"], "user");
    assert_eq!(change_role["after"]["role"], "manager");
}

/// Verifica o contrato identificado por `audit_never_stores_the_password_hash` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn audit_never_stores_the_password_hash() {
    let env = env().await;

    let (status, _) = call(
        &env.state,
        "POST",
        "/api/v1/users",
        Some(&env.admin_token),
        None,
        Some(json!({
            "name": "Segredo",
            "email": "segredo@example.com",
            "password": "Senha-Forte-2026!",
            "role": "user",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (_, body) = call(
        &env.state,
        "GET",
        "/api/v1/audit?entity=user",
        Some(&env.admin_token),
        None,
        None,
    )
    .await;

    let serialized = body.to_string();
    assert!(
        !serialized.contains("password_hash") && !serialized.contains("$argon2"),
        "a trilha não pode conter hash de senha: {serialized}"
    );
}

/// Verifica o contrato identificado por `only_admin_can_read_the_audit_trail` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn only_admin_can_read_the_audit_trail() {
    let env = env().await;

    let (status, _) = call(
        &env.state,
        "GET",
        "/api/v1/audit",
        Some(&env.user_token),
        None,
        None,
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Verifica o contrato identificado por `invalid_audit_filter_returns_400` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn invalid_audit_filter_returns_400() {
    let env = env().await;

    let (status, body) = call(
        &env.state,
        "GET",
        "/api/v1/audit?entity=banco",
        Some(&env.admin_token),
        None,
        None,
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["title"], "invalid_request");
}

/// Verifica o contrato identificado por `logout_revokes_the_access_token_immediately` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn logout_revokes_the_access_token_immediately() {
    let env = env().await;

    // Token válido antes do logout.
    let (status, _) = call(
        &env.state,
        "GET",
        "/api/v1/users/me",
        Some(&env.user_token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Recupera o refresh token correspondente para completar o logout.
    let (_, login_body) = call(
        &env.state,
        "POST",
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({"email": "user@example.com", "password": PASSWORD})),
    )
    .await;
    let access = login_body["access_token"].as_str().expect("access");
    let refresh = login_body["refresh_token"].as_str().expect("refresh");

    let (status, _) = call(
        &env.state,
        "POST",
        "/api/v1/auth/logout",
        Some(access),
        None,
        Some(json!({ "refresh_token": refresh })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // O mesmo token agora é recusado: logout revoga o `jti`, não só o refresh.
    let (status, _) = call(
        &env.state,
        "GET",
        "/api/v1/users/me",
        Some(access),
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "access token revogado não pode continuar válido até expirar"
    );
}

/// Verifica o contrato identificado por `metrics_requires_the_configured_token` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn metrics_requires_the_configured_token() {
    let env = env().await;

    let (status, _) = call(&env.state, "GET", "/metrics", None, None, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "sem token, /metrics responde 404 como rota inexistente"
    );

    let (status, _) = call(
        &env.state,
        "GET",
        "/metrics",
        None,
        Some(vec![("x-metrics-token", "errado")]),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "token errado não confirma a rota"
    );

    let (status, _) = call(
        &env.state,
        "GET",
        "/metrics",
        None,
        Some(vec![("x-metrics-token", METRICS_TOKEN)]),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// Verifica o contrato identificado por `metrics_are_absent_when_no_token_is_configured` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn metrics_are_absent_when_no_token_is_configured() {
    let database = isolated_database("teste").await;

    let state = AppState::for_tests(false, database)
        .with_jwt(SECRET, 900)
        .expect("jwt")
        .with_metrics(None)
        .expect("métricas");

    let (status, _) = call(
        &state,
        "GET",
        "/metrics",
        None,
        Some(vec![("x-metrics-token", "qualquer")]),
        None,
    )
    .await;

    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "sem METRICS_TOKEN a rota não é montada"
    );
}

/// Verifica o contrato identificado por `metrics_text_contains_request_counters` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn metrics_text_contains_request_counters() {
    let env = env().await;

    // Gera tráfego para haver o que contar.
    call(&env.state, "GET", "/health", None, None, None).await;

    let mut request = Request::builder()
        .uri("/metrics")
        .header("x-metrics-token", METRICS_TOKEN)
        .body(Body::empty())
        .expect("requisição");
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 7001))));

    let response = api::router(env.state.clone())
        .oneshot(request)
        .await
        .expect("resposta");

    assert_eq!(response.status(), StatusCode::OK);

    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("corpo")
        .to_bytes();
    let text = String::from_utf8(bytes.to_vec()).expect("utf-8");

    assert!(text.contains("api_http_requests_total"), "{text}");
    assert!(text.contains("api_http_request_duration_seconds"), "{text}");
}
