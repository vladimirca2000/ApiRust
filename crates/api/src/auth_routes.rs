//! Rotas de autenticação: login, refresh, logout e perfil.
//!
//! Todas as respostas de erro seguem `application/problem+json` (RFC 7807) e
//! nunca incluem tokens, senhas ou detalhes internos.

// Handlers devolvem `Response` como erro para poderem formatar RFC 7807
// diretamente. O lint sugere `Box`, mas cada resposta é construída uma vez por
// requisição, então o boxing só adicionaria uma alocação sem ganho.
#![allow(clippy::result_large_err)]

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use domain::Email;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::AuthenticatedUser;
use crate::problem::{ProblemDetails, resolve_actor, to_app_error, to_problem};
use crate::rate_limit::RateLimitPolicy;

/// Corpo de `POST /auth/login`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    /// E-mail informado pelo cliente.
    pub email: String,
    /// Senha em texto puro, transportada apenas sob TLS.
    pub password: String,
}

/// Corpo de `POST /auth/refresh`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RefreshRequest {
    /// Refresh token opaco devolvido pelo login anterior.
    pub refresh_token: String,
}

/// Corpo de `POST /auth/logout`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct LogoutRequest {
    /// Refresh token a revogar.
    pub refresh_token: String,
}

/// Resposta de login e refresh.
#[derive(Debug, Serialize, ToSchema)]
pub struct TokenResponse {
    /// JWT de acesso.
    pub access_token: String,
    /// Token opaco de renovação.
    pub refresh_token: String,
    /// Tipo do token, sempre `Bearer`.
    pub token_type: &'static str,
    /// Validade do access token em segundos.
    pub expires_in: u64,
}

/// Perfil do usuário autenticado.
#[derive(Debug, Serialize, ToSchema)]
pub struct MeResponse {
    /// Identificador do usuário.
    pub id: String,
    /// Nome de exibição.
    pub name: String,
    /// E-mail normalizado.
    pub email: String,
    /// Papel de acesso.
    pub role: String,
    /// Estado do registro.
    pub status: String,
}

/// Monta as rotas de autenticação.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/refresh", post(refresh))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/users/me", get(me))
}

/// Documentação de `POST /api/v1/auth/login`.
#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "Autenticação",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Par de tokens emitido", body = TokenResponse),
        (status = 401, description = "Credenciais inválidas", body = ProblemDetails),
        (status = 429, description = "Rate limit excedido", body = ProblemDetails)
    )
)]
#[allow(dead_code)]
pub fn login_doc() {}

/// Documentação de `POST /api/v1/auth/refresh`.
#[utoipa::path(
    post,
    path = "/api/v1/auth/refresh",
    tag = "Autenticação",
    request_body = RefreshRequest,
    responses(
        (status = 200, description = "Par rotacionado", body = TokenResponse),
        (status = 401, description = "Refresh inválido, expirado ou reutilizado", body = ProblemDetails),
        (status = 429, description = "Rate limit excedido", body = ProblemDetails)
    )
)]
#[allow(dead_code)]
pub fn refresh_doc() {}

/// Documentação de `POST /api/v1/auth/logout`.
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "Autenticação",
    request_body = LogoutRequest,
    responses(
        (status = 204, description = "Refresh token revogado"),
        (status = 500, description = "Falha interna", body = ProblemDetails)
    )
)]
#[allow(dead_code)]
pub fn logout_doc() {}

/// Documentação de `GET /api/v1/users/me`.
#[utoipa::path(
    get,
    path = "/api/v1/users/me",
    tag = "Autenticação",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Perfil do usuário", body = MeResponse),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 403, description = "Usuário inativo", body = ProblemDetails)
    )
)]
#[allow(dead_code)]
pub fn me_doc() {}

/// Resposta `429` com o tempo restante até a janela reabrir.
fn too_many_requests(retry_after: std::time::Duration) -> Response {
    to_problem(
        StatusCode::TOO_MANY_REQUESTS,
        ProblemDetails::new(
            "about:blank",
            "too_many_requests",
            StatusCode::TOO_MANY_REQUESTS.as_u16(),
            format!(
                "tente novamente em {} segundos",
                retry_after.as_secs().max(1)
            ),
        ),
    )
}

/// Autentica e devolve um par de tokens.
///
/// Aplica rate limit por IP antes de tocar no banco, de modo que tentativas
/// repetidas nem cheguem ao Argon2 (que é caro de propósito).
async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<TokenResponse>, Response> {
    let policy = RateLimitPolicy::login();
    let now = Instant::now();
    let key = format!("login:{}", peer.ip());

    if let Err(retry_after) = state.rate_limiter.check(&key, policy, now) {
        return Err(too_many_requests(retry_after));
    }
    state.rate_limiter.prune(policy, now);

    let email = Email::parse(&body.email)
        .map_err(|_| to_app_error(&application::AppError::Authentication))?;

    let pair = state
        .authenticate()
        .login(&email, &body.password, Utc::now())
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(Json(TokenResponse {
        access_token: pair.access_token,
        refresh_token: pair.refresh_token.expose().to_owned(),
        token_type: "Bearer",
        expires_in: pair.expires_in_secs,
    }))
}

/// Rotaciona o refresh token e devolve um par novo.
async fn refresh(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<RefreshRequest>,
) -> Result<Json<TokenResponse>, Response> {
    let policy = RateLimitPolicy::refresh();
    let now = Instant::now();
    let key = format!("refresh:{}", peer.ip());

    if let Err(retry_after) = state.rate_limiter.check(&key, policy, now) {
        return Err(too_many_requests(retry_after));
    }

    let secret = domain::RefreshTokenSecret::new(body.refresh_token);
    let hash = application::hash_refresh(secret.expose());

    let outcome = state
        .authenticate()
        .refresh(&secret, &hash, Utc::now())
        .await
        .map_err(|error| to_app_error(&error))?;

    match outcome {
        domain::RotationOutcome::Rotated { pair, .. } => Ok(Json(TokenResponse {
            access_token: pair.access_token,
            refresh_token: pair.refresh_token.expose().to_owned(),
            token_type: "Bearer",
            expires_in: pair.expires_in_secs,
        })),
        // Reuso e token inválido têm a mesma resposta pública.
        domain::RotationOutcome::Reused | domain::RotationOutcome::Invalid => {
            Err(to_app_error(&application::AppError::Authentication))
        }
    }
}

/// Revoga o refresh token apresentado.
async fn logout(
    State(state): State<AppState>,
    Json(body): Json<LogoutRequest>,
) -> Result<StatusCode, Response> {
    let secret = domain::RefreshTokenSecret::new(body.refresh_token);
    let hash = application::hash_refresh(secret.expose());

    state
        .authenticate()
        .logout(&hash, Utc::now())
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(StatusCode::NO_CONTENT)
}

/// Devolve o perfil do usuário autenticado.
///
/// O estado é relido do banco a cada chamada: um token válido de um usuário
/// desativado desde a emissão não deve continuar acessando o perfil.
async fn me(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
) -> Result<Json<MeResponse>, Response> {
    let actor = resolve_actor(&state, &claims.subject).await?;

    let user = state
        .users
        .find_by_id(actor.id)
        .await
        .map_err(|error| to_app_error(&error))?
        .ok_or_else(|| to_app_error(&application::AppError::Authentication))?;

    Ok(Json(MeResponse {
        id: user.id().to_string(),
        name: user.name().to_owned(),
        email: user.email().as_str().to_owned(),
        role: user.role().as_str().to_owned(),
        status: user.status().as_str().to_owned(),
    }))
}
