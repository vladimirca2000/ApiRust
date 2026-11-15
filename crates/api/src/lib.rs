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
/// Resolução do IP do cliente, com suporte opcional a proxy confiável.
pub mod client_ip;
/// Configuração tipada, lida e validada no boot.
pub mod config;
/// Denylist de access token em memória, para desenvolvimento e testes.
pub mod denylist;
/// Camadas de endurecimento: CORS, limite de corpo, timeout, concorrência e headers.
pub mod hardening;
/// Rotas de histórico do catálogo e da trilha de auditoria.
pub mod history_routes;
/// Coleta e exposição de métricas Prometheus.
pub mod metrics;
/// Shutdown gracioso, drain e constantes de operação.
pub mod operation;
/// Tradução de erros para RFC 7807 e resolução do ator autenticado.
pub mod problem;
/// Rate limit em memória para login e refresh.
pub mod rate_limit;
/// Rate limit das rotas autenticadas de leitura.
pub mod read_limit;
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

/// Carga o banco, aplica migrations e provisiona o Admin inicial (se configurado).
use application::{
    AccessTokenDenylist, AuditRepository, AuthenticateUser, CatalogHistoryRepository,
    CategoryRepository, PasswordHasher, ProductRepository, RateLimitStore, RefreshTokenStore,
    TokenService, UserRepository,
};
use axum::Router;
use infrastructure::{
    Argon2PasswordHasher, Database, JwtTokenService, SqlxAuditRepository,
    SqlxCatalogHistoryRepository, SqlxCategoryRepository, SqlxProductRepository,
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
/// Segredo inerte usado quando o estado é construído fora de teste.
///
/// Não é um segredo real nem o de teste: o bootstrap sempre injeta o segredo
/// verdadeiro com `with_jwt` antes de servir. Serve apenas para o estado ser
/// construível sem `unwrap` e sem valor inseguro compartilhado.
const INERT_SECRET: &str = "estado-ainda-sem-segredo-real-nao-serve-para-assinar";

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
    /// Porta de gravação atômica do histórico do catálogo (estoque e preço).
    pub history: Arc<dyn CatalogHistoryRepository>,
    /// Porta de persistência da trilha de auditoria.
    pub audit: Arc<dyn AuditRepository>,
    /// Limitador de tentativas de login, refresh e leitura.
    pub rate_limiter: Arc<dyn RateLimitStore>,
    /// Denylist de access tokens revogados por `jti`.
    pub denylist: Arc<dyn AccessTokenDenylist>,
    /// Se `true`, monta `/swagger-ui` e `/api-docs/openapi.json`.
    ///
    /// Sempre `false` em produção: o bootstrap rejeita a combinação
    /// `APP_ENV=production` com documentação habilitada, e
    /// [`AppState::from_parts`] parte de `false`, deixando o valor final para
    /// `.with_api_docs`, que o deriva do ambiente.
    pub expose_api_docs: bool,
    /// Origens permitidas por CORS; vazio nega qualquer origem cruzada.
    pub allowed_origins: Vec<String>,
    /// Máximo de requisições HTTP simultâneas; o excesso responde `503`.
    pub max_concurrent_requests: usize,
    /// Métricas Prometheus, quando a coleta está habilitada.
    pub metrics: Option<crate::metrics::Metrics>,
    /// Token exigido por `/metrics`; `None` mantém a rota desmontada.
    pub metrics_token: Option<String>,
    /// IPs de proxy confiáveis; vazio significa "exposta direto".
    ///
    /// Só quando o peer do socket está nesta lista o `X-Forwarded-For` é
    /// considerado. Fora dela o cabeçalho é ignorado, porque o cliente pode
    /// forjá-lo e escapar do rate limit trocando de IP a cada requisição.
    pub trusted_proxies: Vec<std::net::IpAddr>,
}

impl AppState {
    /// Monta o estado **de produção**, sem nenhum valor de teste.
    ///
    /// Recebe as portas já resolvidas pelo bootstrap: segredo JWT real (de
    /// `with_jwt`), política de documentação derivada do ambiente e proxies
    /// confiáveis da configuração. Nenhum default inseguro existe aqui — é a
    /// diferença em relação a [`AppState::for_tests`].
    ///
    /// O `main` usa este construtor; um teste garante que ele não regride para
    /// o de teste.
    #[must_use]
    pub fn from_parts(shutdown_on_tab_close: bool, database: Database) -> Self {
        Self::build(shutdown_on_tab_close, database, false)
    }

    /// Monta o estado **somente para testes**, com JWT descartável e Swagger ligado.
    ///
    /// O nome grita o contrato de propósito: este construtor usa um segredo
    /// descartável e liga `expose_api_docs`, combinação que **nunca**
    /// pode chegar a produção. O bootstrap (`main`) monta o estado com
    /// `.with_jwt(...)` e `.with_api_docs(config.expose_api_docs)`, que derivam o
    /// valor do ambiente.
    ///
    /// Renomeado de `new` porque o nome neutro não indicava o risco: um
    /// `AppState::new` no `main` passaria pela revisão sem levantar suspeita.
    #[must_use]
    pub fn for_tests(shutdown_on_tab_close: bool, database: Database) -> Self {
        Self::build(shutdown_on_tab_close, database, true)
    }

    /// Constrói o estado, aplicando defaults de teste apenas quando pedido.
    ///
    /// Centraliza a fiação das portas para que produção e teste não divirjam em
    /// nada além dos dois defaults controlados por `test_defaults`.
    fn build(shutdown_on_tab_close: bool, database: Database, test_defaults: bool) -> Self {
        let pool = database.pool().clone();
        let users = Arc::new(SqlxUserRepository::new(pool.clone()));
        let refresh = Arc::new(SqlxRefreshTokenStore::new(pool.clone()));
        let categories = Arc::new(SqlxCategoryRepository::new(pool.clone()));
        let products = Arc::new(SqlxProductRepository::new(pool.clone()));
        let history = Arc::new(SqlxCatalogHistoryRepository::new(pool.clone()));
        let audit = Arc::new(SqlxAuditRepository::new(pool));
        // Produção recebe o segredo real por `with_jwt` antes de servir; o valor
        // abaixo existe apenas para o estado ser construível em teste. Um JWT
        // emitido com ele seria rejeitado assim que `with_jwt` rodasse.
        let (secret, ttl) = if test_defaults {
            (DEFAULT_TEST_SECRET, DEFAULT_TEST_TTL_SECS)
        } else {
            // Segredo aleatório: nunca coincide com o real e não é adivinhável.
            (INERT_SECRET, DEFAULT_TEST_TTL_SECS)
        };
        let tokens =
            Arc::new(JwtTokenService::new(secret, ttl).expect("segredo de preenchimento válido"));

        Self {
            sessions: SessionTracker::new(shutdown_on_tab_close),
            database,
            users,
            hasher: Arc::new(Argon2PasswordHasher::new()),
            tokens,
            refresh,
            categories,
            products,
            history,
            audit,
            rate_limiter: Arc::new(RateLimiter::new()),
            denylist: Arc::new(crate::denylist::MemoryDenylist::new()),
            // Só o construtor de teste liga a documentação por padrão; produção
            // usa `with_api_docs` com o valor derivado de `AppConfig`.
            expose_api_docs: test_defaults,
            // Sem origens configuradas: nenhuma origem cruzada é aceita.
            allowed_origins: Vec::new(),
            max_concurrent_requests: crate::operation::DEFAULT_MAX_CONCURRENT_REQUESTS,
            metrics: None,
            metrics_token: None,
            trusted_proxies: Vec::new(),
        }
    }

    /// Habilita a coleta de métricas e a rota `/metrics` com o token informado.
    ///
    /// Sem token, a rota não é montada: métricas expostas sem autenticação
    /// entregam o mapa de rotas e a taxa de erro para qualquer visitante.
    ///
    /// # Errors
    ///
    /// Retorna erro se a criação dos coletores Prometheus falhar.
    pub fn with_metrics(mut self, token: Option<String>) -> Result<Self, prometheus::Error> {
        self.metrics = Some(crate::metrics::Metrics::new()?);
        self.metrics_token = token;
        Ok(self)
    }

    /// Define os IPs de proxy confiáveis para resolução do IP do cliente.
    ///
    /// Used pelo bootstrap com `TRUSTED_PROXIES` e pelos testes que simulam
    /// requisição vinda de proxy.
    #[must_use]
    pub fn with_trusted_proxies(mut self, proxies: Vec<std::net::IpAddr>) -> Self {
        self.trusted_proxies = proxies;
        self
    }

    /// Indica se `/metrics` está efetivamente exposto.
    ///
    /// Exige token **e** coletores: um sem o outro deixaria a rota aberta ou
    /// sem dados.
    #[must_use]
    pub fn session_metrics_enabled(&self) -> bool {
        self.metrics.is_some() && self.metrics_token.is_some()
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
    // Rotas de leitura/escrita autenticada ficam agrupadas para receber o rate
    // limit de tráfego: sem ele, listar produtos ou usuários em laço é gratuito
    // para quem faz scraping.
    let authenticated = Router::new()
        .merge(user_routes::routes())
        .merge(catalog_routes::routes())
        .merge(history_routes::routes())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            read_limit::middleware,
        ));

    let mut router = routes::public_routes()
        .merge(auth_routes::routes())
        .merge(authenticated);

    if state.sessions.is_enabled() {
        router = router.merge(routes::session_routes());
    }

    // Métricas: só são úteis com token de acesso; sem token, nem monta a rota.
    if state.metrics_token.is_some() {
        router = router.merge(metrics::routes());
    }

    // Documentação interativa: apenas em desenvolvimento. Em produção a rota
    // simplesmente não existe, então responde 404 como qualquer caminho
    // desconhecido — `403` confirmaria a existência do endpoint.
    if state.expose_api_docs {
        router = router.merge(api_docs::routes());
    }

    let allowed_origins = state.allowed_origins.clone();
    let max_concurrent = state.max_concurrent_requests;
    // O middleware de métricas precisa do estado depois que ele é consumido
    // por `with_state`; clonar aqui é só um `Arc` por campo.
    let state_for_metrics = state.clone();

    let router = router
        .with_state(state)
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(axum::middleware::from_fn(request_id::publish_request_id))
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(TraceLayer::new_for_http())
        // A coleta fica por dentro do `Trace` e por fora das rotas, para medir
        // a latência que inclui os extractors e a resposta serializada.
        .layer(axum::middleware::from_fn_with_state(
            state_for_metrics,
            metrics::track,
        ));

    // CORS, limite de corpo, timeout, concorrência e headers de segurança por
    // fora de tudo: cortam a requisição antes que ela alcance o handler.
    hardening::apply(router, &allowed_origins, max_concurrent)
}
