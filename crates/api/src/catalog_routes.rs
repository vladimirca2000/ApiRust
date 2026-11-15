//! Rotas do catálogo: categorias e produtos.
//!
//! Leitura é pública (exige apenas autenticação); escrita exige privilégio de
//! catálogo (Admin ou Manager), decidido em `ManageCategories`/`ManageProducts`.

// Handlers devolvem `Response` como erro para formatar RFC 7807 diretamente.
#![allow(clippy::result_large_err)]

use axum::Json;
use axum::Router;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use chrono::Utc;
use domain::{Category, Product, UserId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::AuthenticatedUser;
use crate::problem::{ProblemDetails, resolve_actor, to_app_error};
use application::{
    ManageCategories, ManageProducts, NewCategoryInput, NewProductInput, ProductFilter,
    ProductSort, UpdateCategoryInput, UpdateProductInput,
};

/// Documentação de `GET /api/v1/categories`.
#[utoipa::path(
    get,
    path = "/api/v1/categories",
    tag = "Catálogo",
    security(("bearer_auth" = [])),
    params(
        ("offset" = Option<i64>, Query, description = "Deslocamento a partir do início"),
        ("limit" = Option<i64>, Query, description = "Tamanho da página (máx. 100)")
    ),
    responses(
        (status = 200, description = "Página de categorias", body = CategoryListResponse),
        (status = 401, description = "Não autenticado", body = ProblemDetails)
    )
)]
/// Declara no OpenAPI o contrato da operação `list categories`; não executa a rota e retorna `()`.
#[allow(dead_code)]
pub fn list_categories_doc() {}

/// Documentação de `POST /api/v1/categories`.
#[utoipa::path(
    post,
    path = "/api/v1/categories",
    tag = "Catálogo",
    security(("bearer_auth" = [])),
    request_body = CreateCategoryRequest,
    responses(
        (status = 201, description = "Categoria criada", body = CategoryResponse),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 403, description = "Requer Admin ou Manager", body = ProblemDetails),
        (status = 409, description = "Nome já em uso", body = ProblemDetails),
        (status = 422, description = "Dados inválidos", body = ProblemDetails)
    )
)]
/// Declara no OpenAPI o contrato da operação `create category`; não executa a rota e retorna `()`.
#[allow(dead_code)]
pub fn create_category_doc() {}

/// Documentação de `GET /api/v1/categories/{id}`.
#[utoipa::path(
    get,
    path = "/api/v1/categories/{id}",
    tag = "Catálogo",
    security(("bearer_auth" = [])),
    params(("id" = String, Path, description = "UUID da categoria")),
    responses(
        (status = 200, description = "Categoria", body = CategoryResponse),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 404, description = "Não encontrada", body = ProblemDetails)
    )
)]
/// Declara no OpenAPI o contrato da operação `get category`; não executa a rota e retorna `()`.
#[allow(dead_code)]
pub fn get_category_doc() {}

/// Documentação de `PATCH /api/v1/categories/{id}`.
#[utoipa::path(
    patch,
    path = "/api/v1/categories/{id}",
    tag = "Catálogo",
    security(("bearer_auth" = [])),
    params(("id" = String, Path, description = "UUID da categoria")),
    request_body = UpdateCategoryRequest,
    responses(
        (status = 200, description = "Categoria atualizada", body = CategoryResponse),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 403, description = "Requer Admin ou Manager", body = ProblemDetails),
        (status = 404, description = "Não encontrada", body = ProblemDetails),
        (status = 409, description = "Nome já em uso", body = ProblemDetails)
    )
)]
/// Declara no OpenAPI o contrato da operação `update category`; não executa a rota e retorna `()`.
#[allow(dead_code)]
pub fn update_category_doc() {}

/// Documentação de `DELETE /api/v1/categories/{id}`.
#[utoipa::path(
    delete,
    path = "/api/v1/categories/{id}",
    tag = "Catálogo",
    security(("bearer_auth" = [])),
    params(("id" = String, Path, description = "UUID da categoria")),
    responses(
        (status = 204, description = "Categoria removida"),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 403, description = "Requer Admin ou Manager", body = ProblemDetails),
        (status = 404, description = "Não encontrada", body = ProblemDetails),
        (status = 409, description = "Categoria possui produtos vinculados", body = ProblemDetails)
    )
)]
/// Declara no OpenAPI o contrato da operação `delete category`; não executa a rota e retorna `()`.
#[allow(dead_code)]
pub fn delete_category_doc() {}

/// Documentação de `GET /api/v1/products`.
#[utoipa::path(
    get,
    path = "/api/v1/products",
    tag = "Catálogo",
    security(("bearer_auth" = [])),
    params(
        ("offset" = Option<i64>, Query, description = "Deslocamento a partir do início"),
        ("limit" = Option<i64>, Query, description = "Tamanho da página (máx. 100)"),
        ("sort" = Option<String>, Query, description = "newest | oldest | name | cheapest | most_expensive"),
        ("category_id" = Option<String>, Query, description = "Filtra por categoria"),
        ("name" = Option<String>, Query, description = "Busca por trecho do nome"),
        ("only_active" = Option<bool>, Query, description = "Somente ativos")
    ),
    responses(
        (status = 200, description = "Página de produtos", body = ProductListResponse),
        (status = 400, description = "Ordenação fora da allowlist", body = ProblemDetails),
        (status = 401, description = "Não autenticado", body = ProblemDetails)
    )
)]
/// Declara no OpenAPI o contrato da operação `list products`; não executa a rota e retorna `()`.
#[allow(dead_code)]
pub fn list_products_doc() {}

/// Documentação de `POST /api/v1/products`.
#[utoipa::path(
    post,
    path = "/api/v1/products",
    tag = "Catálogo",
    security(("bearer_auth" = [])),
    request_body = CreateProductRequest,
    responses(
        (status = 201, description = "Produto criado", body = ProductResponse),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 403, description = "Requer Admin ou Manager", body = ProblemDetails),
        (status = 404, description = "Categoria inexistente", body = ProblemDetails),
        (status = 409, description = "SKU já em uso", body = ProblemDetails),
        (status = 422, description = "Preço, estoque ou SKU inválido", body = ProblemDetails)
    )
)]
/// Declara no OpenAPI o contrato da operação `create product`; não executa a rota e retorna `()`.
#[allow(dead_code)]
pub fn create_product_doc() {}

/// Documentação de `GET /api/v1/products/{id}`.
#[utoipa::path(
    get,
    path = "/api/v1/products/{id}",
    tag = "Catálogo",
    security(("bearer_auth" = [])),
    params(("id" = String, Path, description = "UUID do produto")),
    responses(
        (status = 200, description = "Produto", body = ProductResponse),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 404, description = "Não encontrado", body = ProblemDetails)
    )
)]
/// Declara no OpenAPI o contrato da operação `get product`; não executa a rota e retorna `()`.
#[allow(dead_code)]
pub fn get_product_doc() {}

/// Documentação de `PATCH /api/v1/products/{id}`.
#[utoipa::path(
    patch,
    path = "/api/v1/products/{id}",
    tag = "Catálogo",
    security(("bearer_auth" = [])),
    params(("id" = String, Path, description = "UUID do produto")),
    request_body = UpdateProductRequest,
    responses(
        (status = 200, description = "Produto atualizado", body = ProductResponse),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 403, description = "Requer Admin ou Manager", body = ProblemDetails),
        (status = 404, description = "Produto ou categoria inexistente", body = ProblemDetails),
        (status = 409, description = "SKU já em uso", body = ProblemDetails),
        (status = 422, description = "Dados inválidos", body = ProblemDetails)
    )
)]
/// Declara no OpenAPI o contrato da operação `update product`; não executa a rota e retorna `()`.
#[allow(dead_code)]
pub fn update_product_doc() {}

/// Documentação de `DELETE /api/v1/products/{id}`.
#[utoipa::path(
    delete,
    path = "/api/v1/products/{id}",
    tag = "Catálogo",
    security(("bearer_auth" = [])),
    params(("id" = String, Path, description = "UUID do produto")),
    responses(
        (status = 204, description = "Produto removido (soft delete)"),
        (status = 401, description = "Não autenticado", body = ProblemDetails),
        (status = 403, description = "Requer Admin ou Manager", body = ProblemDetails),
        (status = 404, description = "Não encontrado", body = ProblemDetails)
    )
)]
/// Declara no OpenAPI o contrato da operação `delete product`; não executa a rota e retorna `()`.
#[allow(dead_code)]
pub fn delete_product_doc() {}

/// Monta as rotas de categorias e produtos.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/categories",
            get(list_categories).post(create_category),
        )
        .route(
            "/api/v1/categories/{id}",
            get(get_category)
                .patch(update_category)
                .delete(delete_category),
        )
        .route("/api/v1/products", get(list_products).post(create_product))
        .route(
            "/api/v1/products/{id}",
            get(get_product)
                .patch(update_product)
                .delete(delete_product),
        )
}

/// Resposta de categoria.
#[derive(Debug, Serialize, ToSchema)]
pub struct CategoryResponse {
    /// Identificador.
    pub id: String,
    /// Nome único.
    pub name: String,
    /// Descrição opcional.
    pub description: Option<String>,
    /// Estado do registro.
    pub status: String,
    /// Criação em RFC 3339.
    pub created_at: String,
    /// Última atualização em RFC 3339.
    pub updated_at: String,
}

impl From<Category> for CategoryResponse {
    /// Converte uma entidade de domínio em DTO HTTP, preservando os campos públicos e os formatos da API; retorna a resposta serializável.
    fn from(category: Category) -> Self {
        Self {
            id: category.id().to_string(),
            name: category.name().to_owned(),
            description: category.description().map(str::to_owned),
            status: category.status().as_str().to_owned(),
            created_at: category.created_at().to_rfc3339(),
            updated_at: category.updated_at().to_rfc3339(),
        }
    }
}

/// Resposta de produto.
///
/// O preço aparece em **centavos** (`price_cents`), o mesmo inteiro que o banco
/// guarda: converter para decimal na API reintroduziria imprecisão.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProductResponse {
    /// Identificador.
    pub id: String,
    /// Nome comercial.
    pub name: String,
    /// SKU normalizado.
    pub sku: String,
    /// Descrição opcional.
    pub description: Option<String>,
    /// Preço em centavos.
    pub price_cents: i64,
    /// Estoque atual.
    pub stock: i64,
    /// Categoria vinculada.
    pub category_id: String,
    /// Estado do registro.
    pub status: String,
    /// Criação em RFC 3339.
    pub created_at: String,
    /// Última atualização em RFC 3339.
    pub updated_at: String,
}

impl From<Product> for ProductResponse {
    /// Converte uma entidade de domínio em DTO HTTP, preservando os campos públicos e os formatos da API; retorna a resposta serializável.
    fn from(product: Product) -> Self {
        Self {
            id: product.id().to_string(),
            name: product.name().to_owned(),
            sku: product.sku().as_str().to_owned(),
            description: product.description().map(str::to_owned),
            price_cents: product.price().cents(),
            stock: product.stock(),
            category_id: product.category_id().to_string(),
            status: product.status().as_str().to_owned(),
            created_at: product.created_at().to_rfc3339(),
            updated_at: product.updated_at().to_rfc3339(),
        }
    }
}

/// Envelope de listagem de categorias.
#[derive(Debug, Serialize, ToSchema)]
pub struct CategoryListResponse {
    /// Itens da página.
    pub items: Vec<CategoryResponse>,
    /// Total de categorias vivas.
    pub total: i64,
    /// Offset aplicado.
    pub offset: i64,
    /// Limite aplicado.
    pub limit: i64,
}

/// Envelope de listagem de produtos.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProductListResponse {
    /// Itens da página.
    pub items: Vec<ProductResponse>,
    /// Total que atende ao filtro.
    pub total: i64,
    /// Offset aplicado.
    pub offset: i64,
    /// Limite aplicado.
    pub limit: i64,
    /// Ordenação aplicada, para o cliente confirmar o que pediu.
    pub sort: String,
}

/// Query de listagem de categorias.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PageQuery {
    /// Deslocamento a partir do início.
    pub offset: Option<i64>,
    /// Tamanho da página.
    pub limit: Option<i64>,
}

/// Query de listagem de produtos.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ProductQuery {
    /// Deslocamento a partir do início.
    pub offset: Option<i64>,
    /// Tamanho da página.
    pub limit: Option<i64>,
    /// Ordenação (allowlist: newest, oldest, name, cheapest, most_expensive).
    pub sort: Option<String>,
    /// Filtra por categoria.
    pub category_id: Option<String>,
    /// Busca por trecho do nome.
    pub name: Option<String>,
    /// Filtra apenas ativos (`true`).
    pub only_active: Option<bool>,
}

/// Corpo de criação de categoria.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateCategoryRequest {
    /// Nome único.
    pub name: String,
    /// Descrição opcional.
    pub description: Option<String>,
}

/// Corpo de atualização de categoria.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateCategoryRequest {
    /// Novo nome.
    pub name: Option<String>,
    /// Nova descrição (`null` limpa).
    #[serde(default, deserialize_with = "double_option")]
    pub description: Option<Option<String>>,
}

/// Corpo de criação de produto.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateProductRequest {
    /// Nome comercial.
    pub name: String,
    /// SKU.
    pub sku: String,
    /// Descrição opcional.
    pub description: Option<String>,
    /// Preço em centavos (inteiro positivo).
    pub price_cents: i64,
    /// Estoque inicial (não negativo).
    pub stock: i64,
    /// Categoria vinculada.
    pub category_id: String,
}

/// Corpo de atualização de produto.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateProductRequest {
    /// Novo nome.
    pub name: Option<String>,
    /// Nova descrição (`null` limpa).
    #[serde(default, deserialize_with = "double_option")]
    pub description: Option<Option<String>>,
    /// Novo preço em centavos.
    pub price_cents: Option<i64>,
    /// Novo estoque.
    pub stock: Option<i64>,
    /// Nova categoria.
    pub category_id: Option<String>,
}

/// Distingue "campo ausente" de "campo enviado como `null`".
///
/// Sem isso, `description: null` seria indistinguível de não enviar o campo, e
/// não haveria como limpar a descrição via `PATCH`.
fn double_option<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

/// Lista categorias.
async fn list_categories(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<CategoryListResponse>, Response> {
    resolve_actor(&state, &claims.subject).await?;
    let Query(query) = query.map_err(|_| invalid_query())?;

    let use_case =
        ManageCategories::new(state.categories.as_ref()).with_audit(state.audit.as_ref());
    let page = use_case
        .list(
            query.offset.unwrap_or(0),
            query.limit.unwrap_or(application::DEFAULT_PAGE_SIZE),
        )
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(Json(CategoryListResponse {
        items: page.items.into_iter().map(CategoryResponse::from).collect(),
        total: page.total,
        offset: page.offset,
        limit: page.limit,
    }))
}

/// Lê uma categoria.
async fn get_category(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    path: Result<Path<String>, PathRejection>,
) -> Result<Json<CategoryResponse>, Response> {
    resolve_actor(&state, &claims.subject).await?;
    let Path(id) = path.map_err(|_| invalid_path())?;
    let id = parse_id(&id, true)?;

    let use_case =
        ManageCategories::new(state.categories.as_ref()).with_audit(state.audit.as_ref());
    let category = use_case
        .view(id)
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(Json(CategoryResponse::from(category)))
}

/// Cria uma categoria.
async fn create_category(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    body: Result<Json<CreateCategoryRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<CategoryResponse>), Response> {
    let actor = resolve_actor(&state, &claims.subject).await?;
    let Json(body) = body.map_err(|_| invalid_body())?;

    let use_case =
        ManageCategories::new(state.categories.as_ref()).with_audit(state.audit.as_ref());
    let category = use_case
        .create(
            actor,
            NewCategoryInput {
                name: body.name,
                description: body.description,
            },
            Utc::now(),
        )
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok((StatusCode::CREATED, Json(CategoryResponse::from(category))))
}

/// Atualiza uma categoria.
async fn update_category(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    path: Result<Path<String>, PathRejection>,
    body: Result<Json<UpdateCategoryRequest>, JsonRejection>,
) -> Result<Json<CategoryResponse>, Response> {
    let actor = resolve_actor(&state, &claims.subject).await?;
    let Path(id) = path.map_err(|_| invalid_path())?;
    let Json(body) = body.map_err(|_| invalid_body())?;
    let id = parse_id(&id, true)?;

    let use_case =
        ManageCategories::new(state.categories.as_ref()).with_audit(state.audit.as_ref());
    let category = use_case
        .update(
            actor,
            id,
            UpdateCategoryInput {
                name: body.name,
                description: body.description,
            },
            Utc::now(),
        )
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(Json(CategoryResponse::from(category)))
}

/// Exclui uma categoria; conflita quando há produtos vinculados.
async fn delete_category(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    path: Result<Path<String>, PathRejection>,
) -> Result<StatusCode, Response> {
    let actor = resolve_actor(&state, &claims.subject).await?;
    let Path(id) = path.map_err(|_| invalid_path())?;
    let id = parse_id(&id, true)?;

    let use_case =
        ManageCategories::new(state.categories.as_ref()).with_audit(state.audit.as_ref());
    use_case
        .delete(actor, id, Utc::now())
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(StatusCode::NO_CONTENT)
}

/// Lista produtos com filtros, ordenação e paginação.
async fn list_products(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    query: Result<Query<ProductQuery>, QueryRejection>,
) -> Result<Json<ProductListResponse>, Response> {
    resolve_actor(&state, &claims.subject).await?;
    let Query(query) = query.map_err(|_| invalid_query())?;

    // Ordenação fora da allowlist vira 400, conforme o critério de aceite.
    let sort = ProductSort::parse(query.sort.as_deref()).map_err(|error| to_app_error(&error))?;

    let category_id = match query.category_id.as_deref() {
        Some(value) => Some(parse_id(value, true)?),
        None => None,
    };

    let filter = ProductFilter {
        category_id,
        name_contains: query.name.clone(),
        only_active: query.only_active,
    };

    let use_case = ManageProducts::new(
        state.products.as_ref(),
        state.categories.as_ref(),
        state.history.as_ref(),
    )
    .with_audit(state.audit.as_ref());
    let page = use_case
        .list(
            filter,
            sort,
            query.offset.unwrap_or(0),
            query.limit.unwrap_or(application::DEFAULT_PAGE_SIZE),
        )
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(Json(ProductListResponse {
        items: page.items.into_iter().map(ProductResponse::from).collect(),
        total: page.total,
        offset: page.offset,
        limit: page.limit,
        sort: sort_name(sort).to_owned(),
    }))
}

/// Lê um produto.
async fn get_product(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    path: Result<Path<String>, PathRejection>,
) -> Result<Json<ProductResponse>, Response> {
    resolve_actor(&state, &claims.subject).await?;
    let Path(id) = path.map_err(|_| invalid_path())?;
    let id = parse_id(&id, false)?;

    let use_case = ManageProducts::new(
        state.products.as_ref(),
        state.categories.as_ref(),
        state.history.as_ref(),
    )
    .with_audit(state.audit.as_ref());
    let product = use_case
        .view(id)
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(Json(ProductResponse::from(product)))
}

/// Cria um produto.
async fn create_product(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    body: Result<Json<CreateProductRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ProductResponse>), Response> {
    let actor = resolve_actor(&state, &claims.subject).await?;
    let Json(body) = body.map_err(|_| invalid_body())?;
    let category_id = parse_id(&body.category_id, true)?;

    let use_case = ManageProducts::new(
        state.products.as_ref(),
        state.categories.as_ref(),
        state.history.as_ref(),
    )
    .with_audit(state.audit.as_ref());
    let product = use_case
        .create(
            actor,
            NewProductInput {
                name: body.name,
                sku: body.sku,
                description: body.description,
                price_cents: body.price_cents,
                stock: body.stock,
                category_id,
            },
            Utc::now(),
        )
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok((StatusCode::CREATED, Json(ProductResponse::from(product))))
}

/// Atualiza um produto.
async fn update_product(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    path: Result<Path<String>, PathRejection>,
    body: Result<Json<UpdateProductRequest>, JsonRejection>,
) -> Result<Json<ProductResponse>, Response> {
    let actor = resolve_actor(&state, &claims.subject).await?;
    let Path(id) = path.map_err(|_| invalid_path())?;
    let Json(body) = body.map_err(|_| invalid_body())?;
    let id = parse_id(&id, false)?;

    let category_id = match body.category_id.as_deref() {
        Some(value) => Some(parse_id(value, true)?),
        None => None,
    };

    let use_case = ManageProducts::new(
        state.products.as_ref(),
        state.categories.as_ref(),
        state.history.as_ref(),
    )
    .with_audit(state.audit.as_ref());
    let product = use_case
        .update(
            actor,
            id,
            UpdateProductInput {
                name: body.name,
                description: body.description,
                price_cents: body.price_cents,
                stock: body.stock,
                category_id,
            },
            Utc::now(),
        )
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(Json(ProductResponse::from(product)))
}

/// Exclui um produto (soft delete).
async fn delete_product(
    State(state): State<AppState>,
    AuthenticatedUser(claims): AuthenticatedUser,
    path: Result<Path<String>, PathRejection>,
) -> Result<StatusCode, Response> {
    let actor = resolve_actor(&state, &claims.subject).await?;
    let Path(id) = path.map_err(|_| invalid_path())?;
    let id = parse_id(&id, false)?;

    let use_case = ManageProducts::new(
        state.products.as_ref(),
        state.categories.as_ref(),
        state.history.as_ref(),
    )
    .with_audit(state.audit.as_ref());
    use_case
        .delete(actor, id, Utc::now())
        .await
        .map_err(|error| to_app_error(&error))?;

    Ok(StatusCode::NO_CONTENT)
}

/// Nome estável da ordenação, devolvido na resposta.
fn sort_name(sort: ProductSort) -> &'static str {
    match sort {
        ProductSort::Newest => "newest",
        ProductSort::Oldest => "oldest",
        ProductSort::Name => "name",
        ProductSort::Cheapest => "cheapest",
        ProductSort::MostExpensive => "most_expensive",
    }
}

/// Interpreta um identificador, escolhendo o erro conforme o recurso.
///
/// Categoria ausente responde `CategoryNotFound` (404) e produto `NotFound`
/// (404); a distinção existe para a mensagem ficar precisa.
fn parse_id(value: &str, is_category: bool) -> Result<UserId, Response> {
    UserId::parse(value).map_err(|_| {
        let error = if is_category {
            application::AppError::CategoryNotFound
        } else {
            application::AppError::NotFound
        };
        to_app_error(&error)
    })
}

/// Erro padrão para JSON inválido.
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

/// Erro padrão para query inválida.
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
