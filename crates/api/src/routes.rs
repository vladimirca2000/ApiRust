use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use serde::Serialize;
use utoipa::ToSchema;

use crate::AppState;
use crate::operation::READY_PING_TIMEOUT;
use crate::page;

/// Corpo JSON mínimo devolvido por `/health` e `/ready`.
///
/// Apenas o estado do processo/dependências; nunca detalhes internos.
#[derive(Debug, Serialize, ToSchema)]
pub struct StatusResponse {
    /// Estado legível: `ok`, `ready` ou `unavailable`.
    pub status: &'static str,
}

/// Documentação de `GET /health`.
///
/// Existe apenas para carregar a anotação `#[utoipa::path]`; a rota real é
/// registrada em [`public_routes`].
#[utoipa::path(
    get,
    path = "/health",
    tag = "Operação",
    responses(
        (status = 200, description = "Processo ativo", body = StatusResponse)
    )
)]
#[allow(dead_code)]
pub fn health_doc() {}

/// Documentação de `GET /ready`.
#[utoipa::path(
    get,
    path = "/ready",
    tag = "Operação",
    responses(
        (status = 200, description = "Dependências prontas", body = StatusResponse),
        (status = 503, description = "Banco indisponível", body = StatusResponse)
    )
)]
#[allow(dead_code)]
pub fn ready_doc() {}

/// Rotas públicas de operação, sempre montadas.
///
/// São usadas por orquestradores e load balancers, então não exigem
/// autenticação e não expõem dados de negócio.
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
}

/// Rotas auxiliares de desenvolvimento, montadas apenas quando habilitadas.
///
/// Expõem a página com SSE usada para o shutdown por aba; nunca devem ser
/// montadas em produção (o boot já rejeita `SHUTDOWN_ON_TAB_CLOSE=true` lá).
pub fn session_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(page::page))
        .route("/api/dev/session", get(page::session))
}

/// Informa que o processo está vivo, sem consultar dependências.
///
/// Sempre retorna `200` enquanto o servidor conseguir responder.
async fn health() -> Json<StatusResponse> {
    Json(StatusResponse { status: "ok" })
}

/// Informa se as dependências necessárias estão prontas.
///
/// Executa `SELECT 1` no SQLite com timeout curto e traduz o resultado em
/// `200 ready` ou `503 unavailable`; o erro real vai para o log, nunca para o
/// corpo — um orquestrador não deve receber stack trace de SQLite.
async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let ping = tokio::time::timeout(READY_PING_TIMEOUT, state.database.ping()).await;
    match ping {
        Ok(Ok(())) => (StatusCode::OK, Json(StatusResponse { status: "ready" })),
        Ok(Err(error)) => {
            tracing::error!(%error, "readiness falhou");
            unavailable()
        }
        Err(_) => {
            tracing::error!("readiness expirou aguardando o banco");
            unavailable()
        }
    }
}

/// Corpo mínimo de `/ready` quando o banco não responde.
///
/// Isolado para os dois caminhos de falha (erro e timeout) devolverem exatamente
/// o mesmo JSON, sem detalhe interno.
fn unavailable() -> (StatusCode, Json<StatusResponse>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(StatusResponse {
            status: "unavailable",
        }),
    )
}
