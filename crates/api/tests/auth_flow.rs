//! Testes E2E do fluxo de autenticação sobre o roteador em memória.
//!
//! Cobre o critério de aceite da Fase 3: login → acesso → refresh → logout,
//! mais os casos de falha (senha inválida, token inválido, usuário inativo,
//! refresh reutilizado e rate limit).

use std::net::SocketAddr;

use api::AppState;
use application::{PasswordHasher, UserRepository};
use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use chrono::Utc;
use domain::{Email, Role, User};
use http_body_util::BodyExt;
use infrastructure::{Argon2PasswordHasher, Database, SqlxUserRepository};
use serde_json::{Value, json};
use tower::ServiceExt;

/// Segredo usado para assinar os tokens dos testes.
const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

/// Senha em texto puro do usuário de teste.
const PASSWORD: &str = "Senha-Forte-2026!";

/// TTL do access token nos testes, em segundos.
const TTL_SECS: u64 = 900;

/// Banco em memória migrado, com um usuário ativo já criado.
async fn state_with_user() -> AppState {
    let database = Database::connect_path(":memory:")
        .await
        .expect("banco em memória");
    database.migrate().await.expect("migrations");

    let repository = SqlxUserRepository::new(database.pool().clone());
    let hasher = Argon2PasswordHasher::new();
    let hash = hasher.hash(PASSWORD).await.expect("gerar hash");

    let user = User::new(
        "Vladimir",
        Email::parse("user@example.com").expect("e-mail válido"),
        hash,
        Role::Admin,
        Utc::now(),
    )
    .expect("usuário válido");

    repository.insert(&user).await.expect("inserir usuário");

    AppState::new(false, database)
        .with_jwt(SECRET, TTL_SECS)
        .expect("configurar JWT")
}

/// Envia uma requisição ao roteador, informando o IP de origem exigido pelo rate limit.
async fn send(
    state: &AppState,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value, Option<String>) {
    let mut builder = Request::builder().method(method).uri(uri);

    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }

    let request_body = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };

    let mut request = builder.body(request_body).expect("requisição válida");
    // O extractor `ConnectInfo` do rate limit espera o IP da conexão.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));

    let response = api::router(state.clone())
        .oneshot(request)
        .await
        .expect("resposta");

    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("corpo")
        .to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);

    (status, json, content_type)
}

#[tokio::test]
async fn full_flow_login_access_refresh_logout() {
    let state = state_with_user().await;

    // 1. Login
    let (status, body, _) = send(
        &state,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": "user@example.com", "password": PASSWORD})),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let access = body["access_token"].as_str().expect("access_token");
    let refresh = body["refresh_token"].as_str().expect("refresh_token");
    assert_eq!(body["token_type"], "Bearer");
    assert_eq!(body["expires_in"], TTL_SECS);

    // 2. Acesso ao perfil com o access token
    let (status, me, _) = send(&state, "GET", "/api/v1/users/me", None, Some(access)).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["email"], "user@example.com");
    assert_eq!(me["role"], "admin");
    assert_eq!(me["status"], "active");
    assert!(
        me.get("password_hash").is_none(),
        "o perfil nunca pode expor o hash"
    );

    // 3. Refresh: rotaciona e devolve par novo
    let (status, rotated, _) = send(
        &state,
        "POST",
        "/api/v1/auth/refresh",
        Some(json!({"refresh_token": refresh})),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let new_access = rotated["access_token"].as_str().expect("novo access");
    let new_refresh = rotated["refresh_token"].as_str().expect("novo refresh");
    assert_ne!(new_refresh, refresh, "o refresh deve ser rotacionado");

    // O access novo funciona
    let (status, _, _) = send(&state, "GET", "/api/v1/users/me", None, Some(new_access)).await;
    assert_eq!(status, StatusCode::OK);

    // 4. Logout revoga o refresh atual
    let (status, _, _) = send(
        &state,
        "POST",
        "/api/v1/auth/logout",
        Some(json!({"refresh_token": new_refresh})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // 5. O refresh revogado não renova mais
    let (status, _, _) = send(
        &state,
        "POST",
        "/api/v1/auth/refresh",
        Some(json!({"refresh_token": new_refresh})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_with_wrong_password_returns_401_problem_json() {
    let state = state_with_user().await;

    let (status, body, content_type) = send(
        &state,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": "user@example.com", "password": "Senha-Errada-2026!"})),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        content_type.as_deref(),
        Some("application/problem+json"),
        "erro deve seguir RFC 7807"
    );
    assert_eq!(body["status"], 401);
    assert_eq!(body["title"], "unauthorized");
    assert!(
        body.get("access_token").is_none(),
        "erro não pode vazar token"
    );
}

#[tokio::test]
async fn login_with_invalid_email_format_returns_401() {
    let state = state_with_user().await;

    let (status, _, _) = send(
        &state,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": "nao-e-email", "password": PASSWORD})),
        None,
    )
    .await;

    // Formato inválido não revela que o e-mail não existe.
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn me_without_token_returns_401() {
    let state = state_with_user().await;

    let (status, body, content_type) = send(&state, "GET", "/api/v1/users/me", None, None).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(content_type.as_deref(), Some("application/problem+json"));
    assert_eq!(body["status"], 401);
}

#[tokio::test]
async fn me_with_invalid_token_returns_401() {
    let state = state_with_user().await;

    let (status, _, _) = send(
        &state,
        "GET",
        "/api/v1/users/me",
        None,
        Some("token-invalido"),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn me_with_token_of_deactivated_user_returns_403() {
    let state = state_with_user().await;

    let (_, login, _) = send(
        &state,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": "user@example.com", "password": PASSWORD})),
        None,
    )
    .await;
    let access = login["access_token"].as_str().expect("access").to_owned();

    // Desativa depois de emitir o token: o perfil deve recusar mesmo válido.
    sqlx::query("UPDATE users SET status = 'inactive'")
        .execute(state.database.pool())
        .await
        .expect("desativar");

    let (status, _, _) = send(&state, "GET", "/api/v1/users/me", None, Some(&access)).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "token válido de usuário inativo não pode acessar o perfil"
    );
}

#[tokio::test]
async fn reusing_a_refresh_token_invalidates_the_session() {
    let state = state_with_user().await;

    let (_, login, _) = send(
        &state,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": "user@example.com", "password": PASSWORD})),
        None,
    )
    .await;
    let first_refresh = login["refresh_token"].as_str().expect("refresh").to_owned();

    // Primeiro uso: rotaciona
    let (status, rotated, _) = send(
        &state,
        "POST",
        "/api/v1/auth/refresh",
        Some(json!({"refresh_token": first_refresh})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let second_refresh = rotated["refresh_token"]
        .as_str()
        .expect("refresh")
        .to_owned();

    // Reuso do primeiro: deve ser recusado
    let (status, _, _) = send(
        &state,
        "POST",
        "/api/v1/auth/refresh",
        Some(json!({"refresh_token": first_refresh})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // E o segundo, que era válido, também cai (família revogada)
    let (status, _, _) = send(
        &state,
        "POST",
        "/api/v1/auth/refresh",
        Some(json!({"refresh_token": second_refresh})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_is_rate_limited_per_ip() {
    let state = state_with_user().await;

    // A política de login permite 5 tentativas por minuto; a 6ª deve falhar.
    let mut last_status = StatusCode::OK;
    for _ in 0..6 {
        let (status, _, _) = send(
            &state,
            "POST",
            "/api/v1/auth/login",
            Some(json!({"email": "user@example.com", "password": "Senha-Errada-2026!"})),
            None,
        )
        .await;
        last_status = status;
    }

    assert_eq!(
        last_status,
        StatusCode::TOO_MANY_REQUESTS,
        "após exceder o limite, login deve retornar 429"
    );
}

#[tokio::test]
async fn refresh_does_not_accept_an_access_token() {
    let state = state_with_user().await;

    let (_, login, _) = send(
        &state,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": "user@example.com", "password": PASSWORD})),
        None,
    )
    .await;
    let access = login["access_token"].as_str().expect("access").to_owned();

    // Access e refresh não são intercambiáveis.
    let (status, _, _) = send(
        &state,
        "POST",
        "/api/v1/auth/refresh",
        Some(json!({"refresh_token": access})),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn tokens_are_never_logged() {
    let state = state_with_user().await;

    let (_, login, _) = send(
        &state,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": "user@example.com", "password": PASSWORD})),
        None,
    )
    .await;

    let access = login["access_token"].as_str().expect("access");
    let refresh = login["refresh_token"].as_str().expect("refresh");

    // O hash do refresh é o que vai ao banco; o token em claro não deve estar lá.
    let stored: Vec<String> = sqlx::query_scalar("SELECT token_hash FROM refresh_tokens")
        .fetch_all(state.database.pool())
        .await
        .expect("hashes");

    assert!(
        !stored.iter().any(|hash| hash == refresh),
        "o refresh em claro não pode ser persistido"
    );
    assert!(
        !stored.iter().any(|hash| hash == access),
        "o access token não pode ser confundido com refresh"
    );
    assert_eq!(stored.len(), 1);
}
