//! Rotas de gerenciamento de usuários (`/api/v1/users`).
//!
//! Todos os handlers seguem o mesmo fluxo: extraem as claims, resolvem o
//! `Actor` a partir do estado atual no banco e delegam para `ManageUsers`, que
//! aplica a política central. Nenhum handler decide permissão por conta própria
//! — é isso que mantém IDOR e escalonamento de privilégio sob controle.

// Handlers devolvem `Response` como erro para formatar RFC 7807 diretamente;
// `Box` só adicionaria alocação por requisição.
#![allow(clippy::result_large_err)]

use axum::Json;
use axum::Router;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use chrono::Utc;
use domain::{Role, UserId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::AuthenticatedUser;
use crate::problem::{ProblemDetails, resolve_actor, to_app_error};
use application::{DEFAULT_PAGE_SIZE, ManageUsers, NewUserInput, UpdateProfileInput};

/// Documentação de `GET /api/v1/users`.
#[utoipa::path(
    get,
    path = "/api/v1/users",
    tag = "Usuários",
    security(("bearer_auth" = [])),
    params(
        ("offset" = Option<i64>, Query, description = "Deslocamento a partir do início"),
        ("limit" = Option<i64>, Query, description = "Tamanho da página (máx. 100)")
    ),
    responses(
        (status = 200, description = "Página de usuários", body = ListResponse),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 403, description = "Requer Admin", body = ProblemDetails)
    )
)]
#[allow(dead_code)]
pub fn list_users_doc() {}

/// Documentação de `POST /api/v1/users`.
#[utoipa::path(
    post,
    path = "/api/v1/users",
    tag = "Usuários",
    security(("bearer_auth" = [])),
    request_body = CreateUserRequest,
    responses(
        (status = 201, description = "Usuário criado", body = UserResponse),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 403, description = "Requer Admin", body = ProblemDetails),
        (status = 409, description = "E-mail já em uso", body = ProblemDetails),
        (status = 422, description = "Dados inválidos", body = ProblemDetails)
    )
)]
#[allow(dead_code)]
pub fn create_user_doc() {}

/// Documentação de `GET /api/v1/users/{id}`.
#[utoipa::path(
    get,
    path = "/api/v1/users/{id}",
    tag = "Usuários",
    security(("bearer_auth" = [])),
    params(("id" = String, Path, description = "UUID do usuário")),
    responses(
        (status = 200, description = "Usuário", body = UserResponse),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 403, description = "Nem Admin nem o próprio", body = ProblemDetails),
        (status = 404, description = "Não encontrado", body = ProblemDetails)
    )
)]
#[allow(dead_code)]
pub fn get_user_doc() {}

/// Documentação de `PATCH /api/v1/users/{id}`.
#[utoipa::path(
    patch,
    path = "/api/v1/users/{id}",
    tag = "Usuários",
    security(("bearer_auth" = [])),
    params(("id" = String, Path, description = "UUID do usuário")),
    request_body = UpdateUserRequest,
    responses(
        (status = 200, description = "Usuário atualizado", body = UserResponse),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 403, description = "Nem Admin nem o próprio", body = ProblemDetails),
        (status = 404, description = "Não encontrado", body = ProblemDetails),
        (status = 422, description = "Dados inválidos", body = ProblemDetails)
    )
)]
#[allow(dead_code)]
pub fn update_user_doc() {}

/// Documentação de `PATCH /api/v1/users/{id}/role`.
#[utoipa::path(
    patch,
    path = "/api/v1/users/{id}/role",
    tag = "Usuários",
    security(("bearer_auth" = [])),
    params(("id" = String, Path, description = "UUID do usuário")),
    request_body = ChangeRoleRequest,
    responses(
        (status = 200, description = "Papel alterado", body = UserResponse),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 403, description = "Requer Admin e não a própria conta", body = ProblemDetails),
        (status = 404, description = "Não encontrado", body = ProblemDetails),
        (status = 422, description = "Papel inválido", body = ProblemDetails)
    )
)]
#[allow(dead_code)]
pub fn change_role_doc() {}

/// Documentação de `DELETE /api/v1/users/{id}`.
#[utoipa::path(
    delete,
    path = "/api/v1/users/{id}",
    tag = "Usuários",
    security(("bearer_auth" = [])),
    params(("id" = String, Path, description = "UUID do usuário")),
    responses(
        (status = 204, description = "Usuário removido (soft delete)"),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 403, description = "Requer Admin e não a própria conta", body = ProblemDetails),
        (status = 404, description = "Não encontrado", body = ProblemDetails)
    )
)]
#[allow(dead_code)]
pub fn delete_user_doc() {}

/// Monta as rotas de gerenciamento de usuários.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/users", get(list_users).post(create_user))
        .route(
            "/api/v1/users/{id}",
            get(get_user).patch(update_user).delete(delete_user),
        )
        .route(
            "/api/v1/users/{id}/role",
            axum::routing::patch(change_user_role),
        )
}

/// Item de usuário devolvido pela API.
///
/// Nunca inclui `password_hash`: o hash não sai do domínio.
#[derive(Debug, Serialize, ToSchema)]
pub struct UserResponse {
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
    /// Momento de criação (RFC 3339).
    pub created_at: String,
    /// Momento da última atualização (RFC 3339).
    pub updated_at: String,
}

impl From<domain::User> for UserResponse {
    fn from(user: domain::User) -> Self {
        Self {
            id: user.id().to_string(),
            name: user.name().to_owned(),
            email: user.email().as_str().to_owned(),
            role: user.role().as_str().to_owned(),
            status: user.status().as_str().to_owned(),
            created_at: user.created_at().to_rfc3339(),
            updated_at: user.updated_at().to_rfc3339(),
        }
    }
}

/// Envelope de listagem paginada.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListResponse {
    /// Usuários da página atual.
    pub items: Vec<UserResponse>,
    /// Total de usuários vivos.
    pub total: i64,
    /// Offset aplicado.
    pub offset: i64,
    /// Limite aplicado.
    pub limit: i64,
}

/// Parâmetros de `GET /api/v1/users`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListQuery {
    /// Deslocamento a partir do início (padrão 0).
    pub offset: Option<i64>,
    /// Tamanho da página (padrão 20, máximo 100).
    pub limit: Option<i64>,
}

/// Corpo de `POST /api/v1/users`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateUserRequest {
    /// Nome de exibição.
    pub name: String,
    /// E-mail do novo usuário.
    pub email: String,
    /// Senha em texto puro, sujeita à política de força.
    pub password: String,
    /// Papel inicial (`admin`, `manager` ou `user`).
    pub role: String,
}

/// Corpo de `PATCH /api/v1/users/{id}`.
///
/// Todos os campos são opcionais: apenas os informados são alterados.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateUserRequest {
    /// Novo nome.
    pub name: Option<String>,
    /// Novo e-mail.
    pub email: Option<String>,
    /// Nova senha em texto puro.
    pub password: Option<String>,
}

/// Corpo de `PATCH /api/v1/users/{id}/role`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ChangeRoleRequest {
    /// Novo papel (`admin`, `manager` ou `user`).
    pub role: String,
}

/// Lista usuários vivos com paginação.
async fn list_users(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<ListResponse>, Response> {
    let actor = resolve_actor(&state, &claims.subject).await?;
    let Query(query) = query.map_err(|_| invalid_query())?;

    let page = manage(&state)
        .list(
            actor,
            query.offset.unwrap_or(0),
            query.limit.unwrap_or(DEFAULT_PAGE_SIZE),
        )
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(Json(ListResponse {
        items: page.items.into_iter().map(UserResponse::from).collect(),
        total: page.total,
        offset: page.offset,
        limit: page.limit,
    }))
}

/// Cria um usuário novo (apenas Admin).
async fn create_user(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    body: Result<Json<CreateUserRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<UserResponse>), Response> {
    let actor = resolve_actor(&state, &claims.subject).await?;
    let Json(body) = body.map_err(|_| invalid_body())?;

    // Papel vem do corpo, mas só um Admin chega até aqui (autorização abaixo).
    // Valor desconhecido é rejeitado antes de qualquer escrita.
    let role = Role::parse(&body.role)
        .map_err(|error| to_app_error(&application::AppError::Domain(error)))?;

    let user = manage(&state)
        .create(
            actor,
            NewUserInput {
                name: body.name,
                email: body.email,
                password: body.password,
                role,
            },
            Utc::now(),
        )
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok((StatusCode::CREATED, Json(UserResponse::from(user))))
}

/// Lê um usuário por identificador (Admin ou o próprio).
async fn get_user(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    path: Result<Path<String>, PathRejection>,
) -> Result<Json<UserResponse>, Response> {
    let actor = resolve_actor(&state, &claims.subject).await?;
    let Path(id) = path.map_err(|_| invalid_path())?;
    let target = parse_id(&id)?;

    let user = manage(&state)
        .view(actor, target)
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(Json(UserResponse::from(user)))
}

/// Atualiza nome, e-mail e/ou senha (Admin ou o próprio).
async fn update_user(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    path: Result<Path<String>, PathRejection>,
    body: Result<Json<UpdateUserRequest>, JsonRejection>,
) -> Result<Json<UserResponse>, Response> {
    let actor = resolve_actor(&state, &claims.subject).await?;
    let Path(id) = path.map_err(|_| invalid_path())?;
    let Json(body) = body.map_err(|_| invalid_body())?;
    let target = parse_id(&id)?;

    let user = manage(&state)
        .update_profile(
            actor,
            target,
            UpdateProfileInput {
                name: body.name,
                email: body.email,
                password: body.password,
            },
            Utc::now(),
        )
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(Json(UserResponse::from(user)))
}

/// Altera o papel de um usuário (apenas Admin, nunca a própria conta).
async fn change_user_role(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    path: Result<Path<String>, PathRejection>,
    body: Result<Json<ChangeRoleRequest>, JsonRejection>,
) -> Result<Json<UserResponse>, Response> {
    let actor = resolve_actor(&state, &claims.subject).await?;
    let Path(id) = path.map_err(|_| invalid_path())?;
    let Json(body) = body.map_err(|_| invalid_body())?;
    let target = parse_id(&id)?;

    let role = Role::parse(&body.role)
        .map_err(|error| to_app_error(&application::AppError::Domain(error)))?;

    let user = manage(&state)
        .change_role(actor, target, role, Utc::now())
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(Json(UserResponse::from(user)))
}

/// Aplica soft delete em um usuário (apenas Admin, nunca a própria conta).
async fn delete_user(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    path: Result<Path<String>, PathRejection>,
) -> Result<StatusCode, Response> {
    let actor = resolve_actor(&state, &claims.subject).await?;
    let Path(id) = path.map_err(|_| invalid_path())?;
    let target = parse_id(&id)?;

    manage(&state)
        .delete(actor, target, Utc::now())
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(StatusCode::NO_CONTENT)
}

/// Monta o caso de uso ligado às portas deste estado.
fn manage(state: &AppState) -> ManageUsers<'_> {
    ManageUsers::new(state.users.as_ref(), state.hasher.as_ref())
}

/// Interpreta o identificador da URL, devolvendo `404` quando não é UUID.
///
/// Um id malformado não existe no sistema, então `404` é a resposta correta —
/// e não `422`, que sugeriria que o recurso poderia existir.
fn parse_id(value: &str) -> Result<UserId, Response> {
    UserId::parse(value).map_err(|_| to_app_error(&application::AppError::UserNotFound))
}

/// Erro padrão para JSON inválido ou ausente.
fn invalid_body() -> Response {
    crate::problem::to_problem(
        StatusCode::BAD_REQUEST,
        crate::problem::ProblemDetails::new(
            "about:blank",
            "invalid_request",
            StatusCode::BAD_REQUEST.as_u16(),
            "corpo JSON inválido ou ausente".to_owned(),
        ),
    )
}

/// Erro padrão para query string inválida.
fn invalid_query() -> Response {
    crate::problem::to_problem(
        StatusCode::BAD_REQUEST,
        crate::problem::ProblemDetails::new(
            "about:blank",
            "invalid_request",
            StatusCode::BAD_REQUEST.as_u16(),
            "parâmetros de consulta inválidos".to_owned(),
        ),
    )
}

/// Erro padrão para parâmetro de rota inválido.
fn invalid_path() -> Response {
    crate::problem::to_problem(
        StatusCode::BAD_REQUEST,
        crate::problem::ProblemDetails::new(
            "about:blank",
            "invalid_request",
            StatusCode::BAD_REQUEST.as_u16(),
            "parâmetro de rota inválido".to_owned(),
        ),
    )
}
