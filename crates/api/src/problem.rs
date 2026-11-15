//! Utilidades HTTP compartilhadas pelas rotas.
//!
//! Concentra a tradução de `AppError` para `application/problem+json`
//! (RFC 7807) e a resolução do ator autenticado a partir das claims, para que
//! todas as rotas respondam erros de forma idêntica.

// Handlers devolvem `Response` como erro para formatar RFC 7807 diretamente;
// `Box` só adicionaria uma alocação por requisição sem ganho real.
#![allow(clippy::result_large_err)]

use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use application::{Actor, AppError, UserRepository};

use crate::AppState;

/// Corpo de erro no formato RFC 7807.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProblemDetails {
    /// URI que identifica o tipo do problema.
    #[serde(rename = "type")]
    #[schema(rename = "type")]
    pub kind: &'static str,
    /// Resumo curto e estável do problema.
    pub title: &'static str,
    /// Código HTTP repetido no corpo, conforme a RFC.
    pub status: u16,
    /// Explicação específica desta ocorrência, sem dados sensíveis.
    pub detail: String,
    /// URI da ocorrência concreta: aqui, o request ID correlacionado.
    ///
    /// Preenchido automaticamente a partir de [`crate::request_id`] quando o
    /// corpo é serializado, permitindo casar a resposta com a linha do log.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}

impl ProblemDetails {
    /// Monta o corpo de problema já com o `instance` do request atual.
    #[must_use]
    pub fn new(kind: &'static str, title: &'static str, status: u16, detail: String) -> Self {
        Self {
            kind,
            title,
            status,
            detail,
            instance: crate::request_id::current_request_id(),
        }
    }

    /// Monta o corpo sem consultar o request atual, para testes e uso estático.
    #[must_use]
    pub fn without_instance(
        kind: &'static str,
        title: &'static str,
        status: u16,
        detail: String,
    ) -> Self {
        Self {
            kind,
            title,
            status,
            detail,
            instance: None,
        }
    }
}

/// Traduz um [`AppError`] para o status HTTP e o corpo RFC 7807 correspondente.
///
/// Erros de infraestrutura viram `500` genérico: detalhes ficam no log, nunca
/// na resposta ao cliente.
pub fn to_app_error(error: &AppError) -> Response {
    let (status, title, detail) = match error {
        AppError::Authentication => (
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "credenciais inválidas".to_owned(),
        ),
        AppError::InactiveUser => (
            StatusCode::FORBIDDEN,
            "forbidden",
            "usuário inativo".to_owned(),
        ),
        AppError::Forbidden => (
            StatusCode::FORBIDDEN,
            "forbidden",
            "acesso negado".to_owned(),
        ),
        AppError::SelfModificationForbidden => (
            StatusCode::FORBIDDEN,
            "forbidden",
            "operação não permitida sobre a própria conta".to_owned(),
        ),
        AppError::Domain(domain_error) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_error",
            domain_error.to_string(),
        ),
        AppError::EmailAlreadyInUse => (
            StatusCode::CONFLICT,
            "conflict",
            "e-mail já está em uso".to_owned(),
        ),
        AppError::UserNotFound => (
            StatusCode::NOT_FOUND,
            "not_found",
            "recurso não encontrado".to_owned(),
        ),
        AppError::NotFound => (
            StatusCode::NOT_FOUND,
            "not_found",
            "recurso não encontrado".to_owned(),
        ),
        AppError::CategoryNotFound => (
            StatusCode::NOT_FOUND,
            "not_found",
            "categoria não encontrada".to_owned(),
        ),
        AppError::AlreadyInUse(what) => (
            StatusCode::CONFLICT,
            "conflict",
            format!("{what} já está em uso"),
        ),
        // Ordenação fora da allowlist é erro do cliente na query: 400.
        AppError::InvalidSort(value) => (
            StatusCode::BAD_REQUEST,
            "invalid_request",
            format!(
                "ordenação inválida: `{value}`; use newest, oldest, name, cheapest ou most_expensive"
            ),
        ),
        // Regra de negócio que impede a operação (categoria com produtos): 409.
        AppError::Conflict(message) => (StatusCode::CONFLICT, "conflict", message.clone()),
        // Paginação fora do intervalo aceito: 400, apontando o parâmetro.
        AppError::InvalidPagination(message) => {
            (StatusCode::BAD_REQUEST, "invalid_request", message.clone())
        }
        AppError::Infrastructure(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "erro interno".to_owned(),
        ),
    };

    if status == StatusCode::INTERNAL_SERVER_ERROR {
        tracing::error!(%error, "falha ao processar requisição");
    }

    to_problem(
        status,
        ProblemDetails::new("about:blank", title, status.as_u16(), detail),
    )
}

/// Serializa o corpo de problema com o content-type exigido pela RFC 7807.
pub fn to_problem(status: StatusCode, body: ProblemDetails) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/problem+json")],
        Json(body),
    )
        .into_response()
}

/// Resolve o ator autenticado a partir do `sub` das claims.
///
/// Relê o usuário no banco de propósito: o papel e o estado válidos são os do
/// registro atual, não os do momento em que o token foi emitido. Sem isso, um
/// usuário rebaixado de Admin para User continuaria administrando até o token
/// expirar.
///
/// # Errors
///
/// Retorna `401` quando o `sub` não é um UUID, `403` quando a conta está
/// inativa e `404`/`500` conforme o caso.
pub async fn resolve_actor(state: &AppState, subject: &str) -> Result<Actor, Response> {
    let user_id =
        domain::UserId::parse(subject).map_err(|_| to_app_error(&AppError::Authentication))?;

    let user = state
        .users
        .find_by_id(user_id)
        .await
        .map_err(|error| to_app_error(&error))?
        .ok_or_else(|| to_app_error(&AppError::Authentication))?;

    if !user.is_active() {
        return Err(to_app_error(&AppError::InactiveUser));
    }

    Ok(Actor::new(user.id(), user.role()))
}

/// Referência ao repositório de usuários do estado, para leitura conveniente.
pub fn users_of(state: &AppState) -> &dyn UserRepository {
    state.users.as_ref()
}
