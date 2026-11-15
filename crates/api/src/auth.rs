//! Extractor de autenticação: valida o `Authorization: Bearer` e entrega as claims.
//!
//! Centraliza a leitura do cabeçalho para que nenhum handler precise repetir a
//! validação. O extractor consulta a **denylist** por `jti` — é isso que faz o
//! logout invalidar o access token imediatamente — mas não consulta o banco:
//! a checagem de estado do usuário fica com o caso de uso que precisar dela.
//!
//! Quando a denylist está indisponível, a requisição é **recusada** (falha
//! fechada). Aceitar sem conseguir verificar a revogação reabriria exatamente o
//! buraco que a denylist fecha.

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
    /// Extrai o bearer token, verifica assinatura e revogação e retorna as claims autenticadas; em caso de falha, devolve uma resposta HTTP de rejeição.
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

        // Falha fechada: sem conseguir consultar a denylist, não há como saber
        // se o token foi revogado por logout ou rebaixamento.
        let revoked = state
            .denylist
            .is_revoked(&claims.token_id)
            .await
            .map_err(|error| {
                tracing::error!(%error, "denylist indisponível; recusando requisição");
                unauthorized()
            })?;

        if revoked {
            return Err(unauthorized());
        }

        Ok(Self(claims))
    }
}
