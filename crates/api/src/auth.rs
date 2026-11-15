//! Extractor de autenticação: valida o `Authorization: Bearer` e entrega as claims.
//!
//! Centraliza a leitura do cabeçalho para que nenhum handler precise repetir a
//! validação. O extractor nunca consulta o banco: confia nas claims verificadas
//! e deixa a checagem de estado do usuário para o caso de uso que precisar.

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use domain::AccessClaims;

use crate::AppState;

/// Claims autenticadas, disponíveis como argumento de handler.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser(pub AccessClaims);

impl FromRequestParts<AppState> for AuthenticatedUser {
    /// Rejeição já formatada como `application/problem+json` com status `401`.
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let unauthorized = || {
            let body = serde_json::json!({
                "type": "about:blank",
                "title": "unauthorized",
                "status": StatusCode::UNAUTHORIZED.as_u16(),
                "detail": "credenciais inválidas",
            });
            (
                StatusCode::UNAUTHORIZED,
                [("content-type", "application/problem+json")],
                axum::Json(body),
            )
                .into_response()
        };

        let header = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(unauthorized)?;

        let token = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
            .ok_or_else(unauthorized)?;

        let claims = state
            .tokens
            .verify_access(token.trim())
            .await
            .map_err(|_| unauthorized())?;

        Ok(Self(claims))
    }
}
