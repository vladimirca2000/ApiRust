//! Rotas de histórico do catálogo e da trilha de auditoria.
//!
//! São rotas de **leitura** e exigem autenticação:
//!
//! - `GET /api/v1/products/{id}/stock-movements` — Admin/Manager, porque revela
//!   perdas e correções de inventário.
//! - `GET /api/v1/products/{id}/price-changes` — Admin/Manager, pelo mesmo motivo.
//! - `GET /api/v1/audit` — apenas Admin. A trilha mostra quem alterou o quê,
//!   incluindo mudanças de papel; expor a um Manager permitiria vigiar o Admin.

#![allow(clippy::result_large_err)]

use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use domain::UserId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::AuthenticatedUser;
use crate::problem::{ProblemDetails, resolve_actor, to_app_error};
use application::{AuditEntity, AuditFilter, DEFAULT_PAGE_SIZE};

/// Documentação de `GET /api/v1/products/{id}/stock-movements`.
#[utoipa::path(
    get,
    path = "/api/v1/products/{id}/stock-movements",
    tag = "Histórico",
    security(("bearer_auth" = [])),
    params(
        ("id" = String, Path, description = "UUID do produto"),
        ("offset" = Option<i64>, Query, description = "Deslocamento a partir do início"),
        ("limit" = Option<i64>, Query, description = "Tamanho da página (máx. 100)")
    ),
    responses(
        (status = 200, description = "Movimentações de estoque", body = StockMovementListResponse),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 403, description = "Requer Admin ou Manager", body = ProblemDetails),
        (status = 404, description = "Produto não encontrado", body = ProblemDetails)
    )
)]
/// Declara no OpenAPI o contrato da operação `stock movements`; não executa a rota e retorna `()`.
#[allow(dead_code)]
pub fn stock_movements_doc() {}

/// Documentação de `GET /api/v1/products/{id}/price-changes`.
#[utoipa::path(
    get,
    path = "/api/v1/products/{id}/price-changes",
    tag = "Histórico",
    security(("bearer_auth" = [])),
    params(
        ("id" = String, Path, description = "UUID do produto"),
        ("offset" = Option<i64>, Query, description = "Deslocamento a partir do início"),
        ("limit" = Option<i64>, Query, description = "Tamanho da página (máx. 100)")
    ),
    responses(
        (status = 200, description = "Alterações de preço", body = PriceChangeListResponse),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 403, description = "Requer Admin ou Manager", body = ProblemDetails),
        (status = 404, description = "Produto não encontrado", body = ProblemDetails)
    )
)]
/// Declara no OpenAPI o contrato da operação `price changes`; não executa a rota e retorna `()`.
#[allow(dead_code)]
pub fn price_changes_doc() {}

/// Documentação de `GET /api/v1/audit`.
#[utoipa::path(
    get,
    path = "/api/v1/audit",
    tag = "Auditoria",
    security(("bearer_auth" = [])),
    params(
        ("entity" = Option<String>, Query, description = "user, category ou product"),
        ("entity_id" = Option<String>, Query, description = "Identificador da entidade"),
        ("actor_id" = Option<String>, Query, description = "UUID do autor"),
        ("offset" = Option<i64>, Query, description = "Deslocamento a partir do início"),
        ("limit" = Option<i64>, Query, description = "Tamanho da página (máx. 100)")
    ),
    responses(
        (status = 200, description = "Entradas da trilha", body = AuditListResponse),
        (status = 400, description = "Filtro inválido", body = ProblemDetails),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 403, description = "Requer Admin", body = ProblemDetails)
    )
)]
/// Declara no OpenAPI o contrato da operação `audit`; não executa a rota e retorna `()`.
#[allow(dead_code)]
pub fn audit_doc() {}

/// Monta as rotas de histórico e auditoria.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/products/{id}/stock-movements",
            get(stock_movements),
        )
        .route("/api/v1/products/{id}/price-changes", get(price_changes))
        .route("/api/v1/audit", get(list_audit))
}

/// Movimentação de estoque devolvida pela API.
#[derive(Debug, Serialize, ToSchema)]
pub struct StockMovementResponse {
    /// Variação aplicada ao saldo.
    pub delta: i64,
    /// Motivo (`initial`, `correction`, `sale`, `loss` ou `restock`).
    pub reason: String,
    /// Autor da movimentação, quando registrado.
    pub actor_id: Option<String>,
    /// Instante da movimentação (RFC 3339).
    pub at: String,
}

/// Envelope de listagem de movimentações.
#[derive(Debug, Serialize, ToSchema)]
pub struct StockMovementListResponse {
    /// Movimentações da página atual, da mais recente para a mais antiga.
    pub items: Vec<StockMovementResponse>,
    /// Total de movimentações do produto.
    pub total: i64,
    /// Offset aplicado.
    pub offset: i64,
    /// Limite aplicado.
    pub limit: i64,
}

/// Alteração de preço devolvida pela API.
#[derive(Debug, Serialize, ToSchema)]
pub struct PriceChangeResponse {
    /// Preço anterior em centavos.
    pub old_cents: i64,
    /// Preço novo em centavos.
    pub new_cents: i64,
    /// Autor da alteração, quando registrado.
    pub actor_id: Option<String>,
    /// Instante da alteração (RFC 3339).
    pub at: String,
}

/// Envelope de listagem de alterações de preço.
#[derive(Debug, Serialize, ToSchema)]
pub struct PriceChangeListResponse {
    /// Alterações da página atual, da mais recente para a mais antiga.
    pub items: Vec<PriceChangeResponse>,
    /// Total de alterações do produto.
    pub total: i64,
    /// Offset aplicado.
    pub offset: i64,
    /// Limite aplicado.
    pub limit: i64,
}

/// Entrada da trilha devolvida pela API.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuditEntryResponse {
    /// Autor da ação, quando houve.
    pub actor_id: Option<String>,
    /// Ação (`create`, `update`, `change_role` ou `delete`).
    pub action: String,
    /// Entidade afetada (`user`, `category` ou `product`).
    pub entity: String,
    /// Identificador da entidade.
    pub entity_id: String,
    /// Snapshot anterior, quando aplicável.
    pub before: Option<serde_json::Value>,
    /// Snapshot posterior, quando aplicável.
    pub after: Option<serde_json::Value>,
    /// Instante da ação (RFC 3339).
    pub at: String,
}

/// Envelope de listagem da trilha.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuditListResponse {
    /// Entradas da página atual, da mais recente para a mais antiga.
    pub items: Vec<AuditEntryResponse>,
    /// Total de entradas que atendem ao filtro.
    pub total: i64,
    /// Offset aplicado.
    pub offset: i64,
    /// Limite aplicado.
    pub limit: i64,
}

/// Query de paginação usada pelas rotas de histórico.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PageQuery {
    /// Deslocamento a partir do início (padrão 0).
    pub offset: Option<i64>,
    /// Tamanho da página (padrão 20, máximo 100).
    pub limit: Option<i64>,
}

/// Query da trilha de auditoria.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AuditQuery {
    /// Filtro por entidade.
    pub entity: Option<String>,
    /// Filtro por identificador da entidade.
    pub entity_id: Option<String>,
    /// Filtro por autor.
    pub actor_id: Option<String>,
    /// Deslocamento a partir do início.
    pub offset: Option<i64>,
    /// Tamanho da página.
    pub limit: Option<i64>,
}

/// Lista as movimentações de estoque de um produto.
async fn stock_movements(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<StockMovementListResponse>, Response> {
    require_manager(&state, &claims.subject).await?;
    let Path(id) = path.map_err(|_| invalid_path())?;
    let product_id = parse_product_id(&id)?;
    let Query(query) = query.map_err(|_| invalid_query())?;
    let (offset, limit) = normalize(query.offset, query.limit)?;

    let items = state
        .history
        .stock_movements(product_id, offset, limit)
        .await
        .map_err(|error| to_app_error(&error))?;
    let total = state
        .history
        .count_stock_movements(product_id)
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(Json(StockMovementListResponse {
        items: items
            .into_iter()
            .map(|movement| StockMovementResponse {
                delta: movement.delta,
                reason: movement.reason.as_str().to_owned(),
                actor_id: movement.actor_id.map(|id| id.to_string()),
                at: movement.at.to_rfc3339(),
            })
            .collect(),
        total,
        offset,
        limit,
    }))
}

/// Lista as alterações de preço de um produto.
async fn price_changes(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<PriceChangeListResponse>, Response> {
    require_manager(&state, &claims.subject).await?;
    let Path(id) = path.map_err(|_| invalid_path())?;
    let product_id = parse_product_id(&id)?;
    let Query(query) = query.map_err(|_| invalid_query())?;
    let (offset, limit) = normalize(query.offset, query.limit)?;

    let items = state
        .history
        .price_changes(product_id, offset, limit)
        .await
        .map_err(|error| to_app_error(&error))?;
    let total = state
        .history
        .count_price_changes(product_id)
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(Json(PriceChangeListResponse {
        items: items
            .into_iter()
            .map(|change| PriceChangeResponse {
                old_cents: change.old_cents,
                new_cents: change.new_cents,
                actor_id: change.actor_id.map(|id| id.to_string()),
                at: change.at.to_rfc3339(),
            })
            .collect(),
        total,
        offset,
        limit,
    }))
}

/// Lista a trilha de auditoria, com filtros.
async fn list_audit(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    query: Result<Query<AuditQuery>, QueryRejection>,
) -> Result<Json<AuditListResponse>, Response> {
    let actor = resolve_actor(&state, &claims.subject).await?;
    if !actor.is_admin() {
        return Err(to_app_error(&application::AppError::Forbidden));
    }

    let Query(query) = query.map_err(|_| invalid_query())?;
    let (offset, limit) = normalize(query.offset, query.limit)?;

    let entity = query
        .entity
        .as_deref()
        .map(|value| {
            AuditEntity::parse(value).ok_or_else(|| {
                crate::problem::to_problem(
                    StatusCode::BAD_REQUEST,
                    ProblemDetails::new(
                        "about:blank",
                        "invalid_request",
                        StatusCode::BAD_REQUEST.as_u16(),
                        "entity deve ser user, category ou product".to_owned(),
                    ),
                )
            })
        })
        .transpose()?;

    let actor_id = query
        .actor_id
        .as_deref()
        .map(|value| parse_uuid(value, "actor_id"))
        .transpose()?;

    let filter = AuditFilter {
        entity,
        entity_id: query.entity_id.clone(),
        actor_id,
    };

    let page = state
        .audit
        .list(&filter, offset, limit)
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(Json(AuditListResponse {
        items: page
            .items
            .into_iter()
            .map(|entry| AuditEntryResponse {
                actor_id: entry.actor_id.map(|id| id.to_string()),
                action: entry.action.as_str().to_owned(),
                entity: entry.entity.as_str().to_owned(),
                entity_id: entry.entity_id,
                before: entry.before,
                after: entry.after,
                at: entry.at.to_rfc3339(),
            })
            .collect(),
        total: page.total,
        offset: page.offset,
        limit: page.limit,
    }))
}

/// Exige que o usuário autenticado seja Admin ou Manager.
///
/// # Errors
///
/// Retorna `403` para qualquer outro papel e o erro de resolução do ator
/// quando o usuário não existe ou está inativo.
async fn require_manager(state: &AppState, subject: &str) -> Result<(), Response> {
    let actor = resolve_actor(state, subject).await?;
    if actor.role.can_manage_catalog() {
        Ok(())
    } else {
        Err(to_app_error(&application::AppError::Forbidden))
    }
}

/// Normaliza a paginação da mesma forma que as listagens do catálogo.
///
/// Delega para a regra central: `limit` fora de `1..=100` é **erro**, não um
/// valor corrigido em silêncio.
///
/// # Errors
///
/// Retorna `400` quando `limit` está fora do intervalo aceito.
fn normalize(offset: Option<i64>, limit: Option<i64>) -> Result<(i64, i64), Response> {
    application::normalize_page(offset.unwrap_or(0), limit.unwrap_or(DEFAULT_PAGE_SIZE))
        .map_err(|error| to_app_error(&error))
}

/// Interpreta o UUID do produto, respondendo `404` quando inválido.
///
/// # Errors
///
/// Retorna `404`: um identificador malformado descreve um recurso que não
/// existe, e `422` sugeriria que o formato é problema do cliente.
fn parse_product_id(value: &str) -> Result<UserId, Response> {
    UserId::parse(value).map_err(|_| to_app_error(&application::AppError::NotFound))
}

/// Interpreta um UUID de filtro, respondendo `400` quando inválido.
///
/// # Errors
///
/// Retorna `400` porque aqui o valor é um filtro opcional, não a identidade do
/// recurso pedido.
fn parse_uuid(value: &str, field: &str) -> Result<UserId, Response> {
    UserId::parse(value).map_err(|_| {
        crate::problem::to_problem(
            StatusCode::BAD_REQUEST,
            ProblemDetails::new(
                "about:blank",
                "invalid_request",
                StatusCode::BAD_REQUEST.as_u16(),
                format!("{field} deve ser um UUID válido"),
            ),
        )
    })
}

/// Erro padrão para parâmetro de rota inválido.
fn invalid_path() -> Response {
    crate::problem::to_problem(
        StatusCode::BAD_REQUEST,
        ProblemDetails::new(
            "about:blank",
            "invalid_request",
            StatusCode::BAD_REQUEST.as_u16(),
            "parâmetro de rota inválido".to_owned(),
        ),
    )
}

/// Erro padrão para query inválida.
fn invalid_query() -> Response {
    crate::problem::to_problem(
        StatusCode::BAD_REQUEST,
        ProblemDetails::new(
            "about:blank",
            "invalid_request",
            StatusCode::BAD_REQUEST.as_u16(),
            "parâmetros de consulta inválidos".to_owned(),
        ),
    )
}
