//! Camada HTTP e composição da aplicação.
//!
//! Handlers são finos: validam entrada, chamam casos de uso e traduzem
//! resultados em respostas HTTP. Configuração e telemetria ficam aqui.

/// Documentação OpenAPI + Swagger UI (somente em desenvolvimento).
pub mod api_docs;
/// Extractor que valida o `Authorization: Bearer` e expõe as claims.
pub mod auth;
/// Rotas de autenticação: login, refresh, logout e perfil.
pub mod auth_routes;
/// Catálogo único dos endpoints, usado pela página e pelos testes.
pub mod catalog;
/// Rotas do catálogo: categorias e produtos.
pub mod catalog_routes;
/// Configuração tipada, lida e validada no boot.
pub mod config;
/// Camadas de endurecimento: CORS, limite de corpo, timeout, concorrência e headers.
pub mod hardening;
/// Shutdown gracioso, drain e constantes de operação.
pub mod operation;
/// Tradução de erros para RFC 7807 e resolução do ator autenticado.
pub mod problem;
/// Rate limit em memória para login e refresh.
pub mod rate_limit;
/// Propagação do request ID até o corpo de erro (`instance`).
pub mod request_id;
/// Inicialização do `tracing`, em JSON (produção) ou pretty (dev).
pub mod telemetry;
/// Rotas de gerenciamento de usuários.
pub mod user_routes;

/// Página de desenvolvimento com sessão SSE vinculada à aba.
mod page;
/// Definição das rotas públicas e auxiliares.
mod routes;
/// Rastreamento de abas conectadas para o shutdown por aba.
mod session;

use std::sync::Arc;

use application::{
    AuthenticateUser, CategoryRepository, PasswordHasher, ProductRepository, RefreshTokenStore,
    TokenService, UserRepository,
};
use axum::Router;
use infrastructure::{
    Argon2PasswordHasher, Database, JwtTokenService, SqlxCategoryRepository, SqlxProductRepository,
    SqlxRefreshTokenStore, SqlxUserRepository,
};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

use crate::rate_limit::RateLimiter;

pub use session::SessionTracker;

/// Segredo de preenchimento usado por [`AppState::new`] quando nenhum JWT real
/// é injetado (testes de infraestrutura que não exercitam autenticação).
const DEFAULT_TEST_SECRET: &str = "00000000000000000000000000000000";
/// TTL de preenchimento que acompanha [`DEFAULT_TEST_SECRET`].
const DEFAULT_TEST_TTL_SECS: u64 = 900;

/// Estado compartilhado pelos handlers.
///
/// Guarda as **portas** (trait objects em `Arc`), não o caso de uso concreto:
/// `AuthenticateUser` carrega referências com lifetime, então é montado por
/// requisição a partir destes handles. Isso custa apenas clonar `Arc`s e
/// permite que os testes substituam qualquer porta isoladamente.
#[derive(Clone)]
pub struct AppState {
    /// Rastreia abas conectadas para o modo "fechar a aba encerra o servidor".
    pub sessions: SessionTracker,
    /// Acesso ao banco usado por `/ready`.
    pub database: Database,
    /// Porta de persistência de usuários.
    pub users: Arc<dyn UserRepository>,
    /// Porta de hashing de senhas.
    pub hasher: Arc<dyn PasswordHasher>,
    /// Porta de emissão e verificação de access tokens.
    pub tokens: Arc<dyn TokenService>,
    /// Porta de persistência de refresh tokens.
    pub refresh: Arc<dyn RefreshTokenStore>,
    /// Porta de persistência de categorias.
    pub categories: Arc<dyn CategoryRepository>,
    /// Porta de persistência de produtos.
    pub products: Arc<dyn ProductRepository>,
    /// Limitador de tentativas de login e refresh.
    pub rate_limiter: RateLimiter,
    /// Se `true`, monta `/swagger-ui` e `/api-docs/openapi.json`.
    ///
    /// Sempre `false` em produção: o bootstrap rejeita a combinação
    /// `APP_ENV=production` com documentação habilitada, e [`AppState::new`]
    /// deriva o valor do ambiente.
    pub expose_api_docs: bool,
    /// Origens permitidas por CORS; vazio nega qualquer origem cruzada.
    pub allowed_origins: Vec<String>,
    /// Máximo de requisições HTTP simultâneas; o excesso responde `503`.
    pub max_concurrent_requests: usize,
}

impl AppState {
    /// Monta o estado sem autenticação configurada, para testes de infraestrutura.
    ///
    /// Todas as portas apontam para adaptadores reais sobre o banco informado.
    #[must_use]
    pub fn new(shutdown_on_tab_close: bool, database: Database) -> Self {
        let pool = database.pool().clone();
        let users = Arc::new(SqlxUserRepository::new(pool.clone()));
        let refresh = Arc::new(SqlxRefreshTokenStore::new(pool.clone()));
        let categories = Arc::new(SqlxCategoryRepository::new(pool.clone()));
        let products = Arc::new(SqlxProductRepository::new(pool));
        // Segredo descartável: só é usado se um teste chamar as rotas de auth
        // sem injetar as portas corretas.
        let tokens = Arc::new(
            JwtTokenService::new(DEFAULT_TEST_SECRET, DEFAULT_TEST_TTL_SECS)
                .expect("segredo de teste válido"),
        );

        Self {
            sessions: SessionTracker::new(shutdown_on_tab_close),
            database,
            users,
            hasher: Arc::new(Argon2PasswordHasher::new()),
            tokens,
            refresh,
            categories,
            products,
            rate_limiter: RateLimiter::new(),
            // O construtor de teste assume desenvolvimento. O bootstrap real usa
            // `with_api_docs` com o valor derivado de `AppConfig`.
            expose_api_docs: true,
            // Sem origens configuradas: nenhuma origem cruzada é aceita.
            allowed_origins: Vec::new(),
            max_concurrent_requests: crate::operation::DEFAULT_MAX_CONCURRENT_REQUESTS,
        }
    }

    /// Define se a documentação interativa deve ser montada.
    ///
    /// Usado pelo bootstrap para refletir `AppConfig::expose_api_docs` e pelos
    /// testes que precisam simular produção.
    #[must_use]
    pub fn with_api_docs(mut self, expose: bool) -> Self {
        self.expose_api_docs = expose;
        self
    }

    /// Define as origens permitidas por CORS.
    #[must_use]
    pub fn with_allowed_origins(mut self, origins: Vec<String>) -> Self {
        self.allowed_origins = origins;
        self
    }

    /// Define o teto de requisições HTTP simultâneas.
    ///
    /// Usado pelo bootstrap com o valor validado de `MAX_CONCURRENT_REQUESTS` e
    /// pelos testes que exercitam o `503` de sobrecarga com um teto de `1`.
    #[must_use]
    pub fn with_max_concurrent_requests(mut self, max_concurrent: usize) -> Self {
        self.max_concurrent_requests = max_concurrent;
        self
    }

    /// Substitui as portas de autenticação pelos adaptadores reais do bootstrap.
    ///
    /// # Errors
    ///
    /// Retorna erro se o segredo ou o TTL do JWT forem inválidos.
    pub fn with_jwt(
        mut self,
        secret: &str,
        access_ttl_secs: u64,
    ) -> Result<Self, application::AppError> {
        self.tokens = Arc::new(JwtTokenService::new(secret, access_ttl_secs)?);
        Ok(self)
    }

    /// Monta o caso de uso de autenticação ligado às portas deste estado.
    #[must_use]
    pub fn authenticate(&self) -> AuthenticateUser<'_> {
        AuthenticateUser::new(
            self.users.as_ref(),
            self.hasher.as_ref(),
            self.tokens.as_ref(),
            self.refresh.as_ref(),
        )
    }
}

/// Monta o roteador HTTP com middlewares transversais.
///
/// Registra as rotas públicas (`/health`, `/ready`), as rotas de autenticação e,
/// quando habilitado, o modo de sessão de desenvolvimento. As camadas de request
/// ID e trace envolvem todas as rotas, inclusive as de erro.
///
/// Ordem das camadas (de fora para dentro): `Trace` → `SetRequestId` →
/// `publish_request_id` → `PropagateRequestId` → rotas. O `publish` precisa vir
/// **depois** do `Set` para enxergar o ID recém-gerado e **antes** do handler
/// para que o corpo de erro possa preencher `instance`.
pub fn router(state: AppState) -> Router {
    let mut router = routes::public_routes()
        .merge(auth_routes::routes())
        .merge(user_routes::routes())
        .merge(catalog_routes::routes());

    if state.sessions.is_enabled() {
        router = router.merge(routes::session_routes());
    }

    // Documentação interativa: apenas em desenvolvimento. Em produção a rota
    // simplesmente não existe, então responde 404 como qualquer caminho
    // desconhecido — `403` confirmaria a existência do endpoint.
    if state.expose_api_docs {
        router = router.merge(api_docs::routes());
    }

    let allowed_origins = state.allowed_origins.clone();
    let max_concurrent = state.max_concurrent_requests;

    let router = router
        .with_state(state)
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(axum::middleware::from_fn(request_id::publish_request_id))
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(TraceLayer::new_for_http());

    // CORS, limite de corpo, timeout, concorrência e headers de segurança por
    // fora de tudo: cortam a requisição antes que ela alcance o handler.
    hardening::apply(router, &allowed_origins, max_concurrent)
}
