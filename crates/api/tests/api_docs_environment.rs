//! Garante que a documentação interativa **nunca** aparece em produção.
//!
//! Esta é a regra que o projeto passou a exigir: Swagger UI, JSON OpenAPI e
//! qualquer console equivalente existem apenas em desenvolvimento. Os testes
//! cobrem os dois lados — presentes em dev, ausentes (`404`) em produção — e o
//! boot que rejeita a combinação insegura.

mod common;

use common::isolated_database;
use std::net::SocketAddr;

use api::AppState;
use api::config::AppConfig;
use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

/// Segredo de teste válido para o JWT.
const SECRET: &str = "0123456789abcdef0123456789abcdef";

/// Caminhos que expõem contrato ou console interativo.
const DOC_PATHS: [&str; 2] = ["/swagger-ui", "/api-docs/openapi.json"];

/// Estado com banco em memória, controlando a exposição da documentação.
async fn state(expose_api_docs: bool) -> AppState {
    let database = isolated_database("teste").await;

    AppState::for_tests(false, database)
        .with_jwt(SECRET, 900)
        .expect("jwt")
        .with_api_docs(expose_api_docs)
}

/// Executa um GET e devolve o status.
async fn get(state: &AppState, path: &str) -> StatusCode {
    let mut request = Request::builder()
        .uri(path)
        .body(Body::empty())
        .expect("requisição");
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 7100))));

    api::router(state.clone())
        .oneshot(request)
        .await
        .expect("resposta")
        .status()
}

/// Verifica o contrato identificado por `documentation_is_served_in_development` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn documentation_is_served_in_development() {
    let app = state(true).await;

    // A UI redireciona para o index; qualquer 2xx/3xx prova que a rota existe.
    let ui = get(&app, "/swagger-ui").await;
    assert!(
        !matches!(ui, StatusCode::NOT_FOUND),
        "em desenvolvimento o Swagger UI deve existir, recebido {ui}"
    );

    let spec = get(&app, "/api-docs/openapi.json").await;
    assert_eq!(
        spec,
        StatusCode::OK,
        "o JSON OpenAPI deve ser servido em desenvolvimento"
    );
}

/// Verifica o contrato identificado por `documentation_returns_404_in_production` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn documentation_returns_404_in_production() {
    let app = state(false).await;

    for path in DOC_PATHS {
        let status = get(&app, path).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "`{path}` não pode existir quando a documentação está desabilitada"
        );
    }
}

/// Verifica o contrato identificado por `documentation_returns_404_not_403` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn documentation_returns_404_not_403() {
    let app = state(false).await;

    // `403` confirmaria a existência do endpoint; `404` não revela nada.
    for path in DOC_PATHS {
        assert_ne!(
            get(&app, path).await,
            StatusCode::FORBIDDEN,
            "rota de documentação desabilitada deve responder 404, não 403 ({path})"
        );
    }
}

/// Verifica o contrato identificado por `other_alternative_consoles_do_not_exist` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn other_alternative_consoles_do_not_exist() {
    let app = state(true).await;

    // Redoc, GraphQL Playground, Scalar e afins não são montados em nenhum
    // ambiente: a regra cobre "outra similar", não só Swagger.
    for path in ["/redoc", "/graphql", "/playground", "/scalar", "/docs"] {
        let status = get(&app, path).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "`{path}` não deve existir, nem em desenvolvimento"
        );
    }
}

/// Garante que a documentação não interfere nas rotas reais.
#[tokio::test]
async fn api_routes_work_regardless_of_documentation() {
    for expose in [true, false] {
        let app = state(expose).await;

        assert_eq!(
            get(&app, "/health").await,
            StatusCode::OK,
            "/health deve responder com expose_api_docs={expose}"
        );
    }
}

/// Confere que o JSON publicado cobre as rotas do catálogo.
#[tokio::test]
async fn published_spec_covers_every_catalogued_path() {
    let app = state(true).await;

    let mut request = Request::builder()
        .uri("/api-docs/openapi.json")
        .body(Body::empty())
        .expect("requisição");
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 7101))));

    let response = api::router(app).oneshot(request).await.expect("resposta");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .expect("corpo");
    let spec: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON válido");
    let paths = spec["paths"].as_object().expect("objeto paths");

    for endpoint in api::catalog::all_endpoints() {
        if endpoint.path == "/" || endpoint.path.starts_with("/api/dev/") {
            continue;
        }
        assert!(
            paths.contains_key(endpoint.path),
            "spec publicada não contém {} {}",
            endpoint.method,
            endpoint.path
        );
    }
}

/// O boot precisa rejeitar documentação habilitada em produção.
#[test]
fn production_rejects_enabling_api_docs() {
    let base = |env: &str, expose: Option<&str>| -> Result<AppConfig, String> {
        let mut pairs = vec![
            ("APP_ENV", env),
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", SECRET),
        ];
        if let Some(value) = expose {
            pairs.push(("EXPOSE_API_DOCS", value));
        }
        AppConfig::from_map(&pairs).map_err(|error| error.to_string())
    };

    // Produção + habilitar explicitamente: deve abortar o boot.
    let rejected = base("production", Some("true"));
    assert!(
        rejected.is_err(),
        "produção + EXPOSE_API_DOCS=true deve abortar o boot"
    );
    assert!(
        rejected
            .expect_err("erro esperado")
            .contains("EXPOSE_API_DOCS"),
        "a mensagem deve indicar a variável culpada"
    );

    // Produção sem a variável: documentação desabilitada.
    let production = base("production", None).expect("config válida");
    assert!(
        !production.expose_api_docs,
        "produção nunca deve expor a documentação"
    );

    // Produção pedindo explicitamente para desabilitar: também desabilitada.
    let production_off = base("production", Some("false")).expect("config válida");
    assert!(!production_off.expose_api_docs);

    // Desenvolvimento sem a variável: padrão é habilitado.
    let development = base("development", None).expect("config válida");
    assert!(
        development.expose_api_docs,
        "em desenvolvimento o padrão é expor"
    );

    // Desenvolvimento pedindo desabilitar: respeitado.
    let development_off = base("development", Some("false")).expect("config válida");
    assert!(
        !development_off.expose_api_docs,
        "EXPOSE_API_DOCS=false deve desabilitar até em desenvolvimento"
    );
}
