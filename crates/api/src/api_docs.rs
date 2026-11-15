//! Documentação interativa da API (OpenAPI + Swagger UI).
//!
//! **Só existe em desenvolvimento.** Em produção o módulo não é montado no
//! roteador (`AppConfig::expose_api_docs` é `false`), então as rotas respondem
//! `404` como qualquer caminho inexistente. Expor a spec entregaria um mapa
//! completo de rotas, esquemas e códigos de erro — e o Swagger UI permite
//! disparar requisições, inclusive com credenciais do visitante.

use axum::Router;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::AppState;

/// Documento OpenAPI gerado a partir das anotações `#[utoipa::path]`.
///
/// `paths(...)` precisa listar toda rota documentada; há um teste que compara
/// essa lista com o catálogo de endpoints para impedir divergência silenciosa.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "ApiRust",
        version = "0.1.0",
        description = "API REST de usuários, categorias e produtos.\n\n\
                       Documentação disponível apenas em desenvolvimento. \
                       Em produção estas rotas não existem.",
    ),
    paths(
        crate::routes::health_doc,
        crate::routes::ready_doc,
        crate::auth_routes::login_doc,
        crate::auth_routes::refresh_doc,
        crate::auth_routes::logout_doc,
        crate::auth_routes::me_doc,
        crate::user_routes::list_users_doc,
        crate::user_routes::create_user_doc,
        crate::user_routes::get_user_doc,
        crate::user_routes::update_user_doc,
        crate::user_routes::change_role_doc,
        crate::user_routes::delete_user_doc,
        crate::catalog_routes::list_categories_doc,
        crate::catalog_routes::create_category_doc,
        crate::catalog_routes::get_category_doc,
        crate::catalog_routes::update_category_doc,
        crate::catalog_routes::delete_category_doc,
        crate::catalog_routes::list_products_doc,
        crate::catalog_routes::create_product_doc,
        crate::catalog_routes::get_product_doc,
        crate::catalog_routes::update_product_doc,
        crate::catalog_routes::delete_product_doc,
        crate::history_routes::stock_movements_doc,
        crate::history_routes::price_changes_doc,
        crate::history_routes::audit_doc,
    ),
    components(schemas(
        crate::problem::ProblemDetails,
        crate::routes::StatusResponse,
        crate::auth_routes::LoginRequest,
        crate::auth_routes::RefreshRequest,
        crate::auth_routes::LogoutRequest,
        crate::auth_routes::TokenResponse,
        crate::auth_routes::MeResponse,
        crate::user_routes::UserResponse,
        crate::user_routes::ListResponse,
        crate::user_routes::CreateUserRequest,
        crate::user_routes::UpdateUserRequest,
        crate::user_routes::ChangeRoleRequest,
        crate::catalog_routes::CategoryResponse,
        crate::catalog_routes::CategoryListResponse,
        crate::catalog_routes::CreateCategoryRequest,
        crate::catalog_routes::UpdateCategoryRequest,
        crate::catalog_routes::ProductResponse,
        crate::catalog_routes::ProductListResponse,
        crate::catalog_routes::CreateProductRequest,
        crate::catalog_routes::UpdateProductRequest,
        crate::history_routes::StockMovementResponse,
        crate::history_routes::StockMovementListResponse,
        crate::history_routes::PriceChangeResponse,
        crate::history_routes::PriceChangeListResponse,
        crate::history_routes::AuditEntryResponse,
        crate::history_routes::AuditListResponse,
        crate::history_routes::PageQuery,
        crate::history_routes::AuditQuery,
    )),
    tags(
        (name = "Operação", description = "Health e readiness"),
        (name = "Autenticação", description = "Login, refresh, logout e perfil"),
        (name = "Usuários", description = "CRUD de usuários e papéis"),
        (name = "Catálogo", description = "Categorias e produtos"),
        (name = "Histórico", description = "Movimentação de estoque e mudanças de preço"),
        (name = "Auditoria", description = "Trilha de quem alterou o quê"),
    ),
    servers(
        (url = "/", description = "Este servidor")
    ),
    modifiers(&BearerSecurityAddon)
)]
/// Especificação OpenAPI completa da API, incluindo rotas, esquemas e segurança Bearer.
pub struct ApiDoc;

/// Adiciona o esquema Bearer usado pelo JWT à especificação OpenAPI.
///
/// O esquema é registrado nos componentes reutilizáveis para que a interface
/// Swagger exiba o botão `Authorize` e envie o token informado no cabeçalho
/// `Authorization` das operações protegidas.
struct BearerSecurityAddon;

impl Modify for BearerSecurityAddon {
    /// Registra a autenticação Bearer com formato JWT na especificação.
    ///
    /// A alteração é aplicada depois da geração automática dos schemas e
    /// preserva os demais componentes já produzidos. Não retorna valor.
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi
            .components
            .get_or_insert_with(utoipa::openapi::Components::new);
        components.add_security_scheme(
            "bearer_auth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .build(),
            ),
        );
    }
}

/// Caminho do JSON OpenAPI.
pub const SPEC_PATH: &str = "/api-docs/openapi.json";

/// Caminho onde o Swagger UI é servido.
pub const SWAGGER_PATH: &str = "/swagger-ui";

/// Rotas de documentação, montadas **apenas** em desenvolvimento.
///
/// `SwaggerUi` já serve o JSON da spec na URL informada em `.url(...)`, então
/// não é preciso registrar o endpoint da spec separadamente.
pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(utoipa_swagger_ui::SwaggerUi::new(SWAGGER_PATH).url(SPEC_PATH, ApiDoc::openapi()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifica o contrato identificado por `spec_lists_every_catalogued_path` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn spec_lists_every_catalogued_path() {
        let spec = ApiDoc::openapi();
        let doc = spec.paths.paths;

        // Toda rota do catálogo precisa estar documentada, exceto as auxiliares
        // de desenvolvimento (página HTML e SSE), que não são contrato de API.
        for endpoint in crate::catalog::all_endpoints() {
            if endpoint.path == "/" || endpoint.path.starts_with("/api/dev/") {
                continue;
            }
            let documented = doc.contains_key(endpoint.path);
            assert!(
                documented,
                "rota {} {} não está documentada no OpenAPI",
                endpoint.method, endpoint.path
            );
        }
    }

    /// Verifica o contrato identificado por `spec_declares_the_problem_details_schema` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn spec_declares_the_problem_details_schema() {
        let spec = ApiDoc::openapi();
        let components = spec.components.expect("components");

        assert!(
            components.schemas.contains_key("ProblemDetails"),
            "o schema de erro RFC 7807 deve existir na spec"
        );
    }

    /// Verifica o contrato identificado por `spec_has_the_expected_title` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn spec_has_the_expected_title() {
        let spec = ApiDoc::openapi();

        assert_eq!(spec.info.title, "ApiRust");
    }

    /// Verifica o contrato identificado por `spec_declares_bearer_jwt_authorization` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn spec_declares_bearer_jwt_authorization() {
        let spec = ApiDoc::openapi();
        let components = spec.components.expect("components");
        let scheme = components
            .security_schemes
            .get("bearer_auth")
            .expect("esquema bearer_auth");

        assert_eq!(
            serde_json::to_value(scheme).expect("serialização do esquema"),
            serde_json::json!({
                "type": "http",
                "scheme": "bearer",
                "bearerFormat": "JWT"
            })
        );
    }
}
