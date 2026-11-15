//! Garante que o catálogo de endpoints e o roteador real não divergem.
//!
//! A página `GET /` é gerada a partir de `catalog`, então um endpoint montado no
//! `router()` mas ausente do catálogo ficaria invisível para o usuário. Estes
//! testes falham nos dois sentidos: rota no roteador sem catálogo e rota no
//! catálogo que o roteador não atende.

mod common;

use common::isolated_database;
use std::net::SocketAddr;

use api::AppState;
use api::catalog::{self, Access};
use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

/// Estado com banco em memória migrado e JWT configurado.
async fn state(sessions_enabled: bool) -> AppState {
    let database = isolated_database("teste").await;

    AppState::for_tests(sessions_enabled, database)
        .with_jwt("0123456789abcdef0123456789abcdef", 900)
        .expect("JWT válido")
}

/// Envia uma requisição informando o IP de origem (exigido pelo rate limit).
async fn call(state: &AppState, method: &str, path: &str) -> StatusCode {
    let builder = Request::builder().method(method).uri(path);
    let mut request = builder.body(Body::empty()).expect("requisição válida");
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4000))));

    api::router(state.clone())
        .oneshot(request)
        .await
        .expect("resposta")
        .status()
}

/// Verifica o contrato identificado por `every_catalogued_endpoint_is_reachable_in_the_router` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn every_catalogued_endpoint_is_reachable_in_the_router() {
    let app_state = state(true).await;

    for endpoint in catalog::all_endpoints() {
        let status = call(&app_state, endpoint.method, endpoint.path).await;

        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "rota do catálogo não existe no roteador: {} {}",
            endpoint.method,
            endpoint.path
        );
    }
}

/// Verifica o contrato identificado por `catalogue_covers_the_real_route_set` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn catalogue_covers_the_real_route_set() {
    // Lista completa dos caminhos declarados em `routes.rs` e `auth_routes.rs`.
    //
    // O axum não expõe introspecção das rotas registradas no `Router`, então a
    // única forma de detectar "rota no roteador sem catálogo" é comparar com a
    // fonte. `declared_route_paths()` lê os `.route("...")` do código-fonte.
    let declared = declared_route_paths();
    assert!(
        !declared.is_empty(),
        "não foi possível ler as rotas declaradas no código-fonte"
    );

    for path in &declared {
        // "/" só existe em modo dev; os demais são sempre montados.
        let is_dev_only = path == "/" || path.starts_with("/api/dev/");
        if is_dev_only {
            continue;
        }

        let in_catalogue = catalog::all_endpoints().any(|endpoint| endpoint.path == *path);
        assert!(
            in_catalogue,
            "rota `{path}` está declarada no código e não no catálogo — \
             ela ficaria invisível na página `GET /`"
        );
    }
}

/// Verifica o contrato identificado por `catalogue_has_no_endpoint_missing_from_the_code` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn catalogue_has_no_endpoint_missing_from_the_code() {
    let declared = declared_route_paths();

    for endpoint in catalog::all_endpoints() {
        assert!(
            declared.iter().any(|path| path == endpoint.path),
            "catálogo declara `{}` mas o código não registra essa rota",
            endpoint.path
        );
    }
}

/// Extrai os caminhos passados a `.route("...")` nos módulos de rotas.
///
/// Lê os arquivos-fonte em tempo de teste; se o projeto ganhar um módulo de
/// rotas novo, ele precisa ser adicionado aqui — do contrário esta verificação
/// passaria a ignorá-lo em silêncio.
fn declared_route_paths() -> Vec<String> {
    let sources = [
        include_str!("../src/routes.rs"),
        include_str!("../src/auth_routes.rs"),
        include_str!("../src/user_routes.rs"),
        include_str!("../src/catalog_routes.rs"),
        include_str!("../src/history_routes.rs"),
    ];

    let mut paths = Vec::new();
    for source in sources {
        for segment in source.split(".route(").skip(1) {
            let Some(rest) = segment.trim_start().strip_prefix('"') else {
                continue;
            };
            let Some(end) = rest.find('"') else { continue };
            let path = &rest[..end];
            if path.starts_with('/') {
                paths.push(path.to_owned());
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

/// Verifica o contrato identificado por `dev_only_routes_are_absent_when_sessions_are_disabled` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn dev_only_routes_are_absent_when_sessions_are_disabled() {
    let app_state = state(false).await;

    for endpoint in catalog::all_endpoints() {
        if endpoint.access != Access::DevOnly {
            continue;
        }

        let status = call(&app_state, endpoint.method, endpoint.path).await;

        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "rota de desenvolvimento acessível com sessões desabilitadas: {} {}",
            endpoint.method,
            endpoint.path
        );
    }
}

/// Verifica o contrato identificado por `bearer_endpoints_reject_requests_without_token` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn bearer_endpoints_reject_requests_without_token() {
    let app_state = state(true).await;

    for endpoint in catalog::all_endpoints() {
        if endpoint.access != Access::Bearer {
            continue;
        }

        let status = call(&app_state, endpoint.method, endpoint.path).await;

        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "endpoint Bearer deveria exigir token: {} {}",
            endpoint.method,
            endpoint.path
        );
    }
}

/// Verifica o contrato identificado por `the_root_page_lists_the_catalogue` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn the_root_page_lists_the_catalogue() {
    let app_state = state(true).await;

    let response = api::router(app_state)
        .oneshot(
            Request::builder()
                .uri("/")
                .body(Body::empty())
                .expect("requisição"),
        )
        .await
        .expect("resposta");

    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), 512 * 1024)
        .await
        .expect("corpo");
    let html = String::from_utf8(bytes.to_vec()).expect("utf-8");

    for endpoint in catalog::all_endpoints() {
        assert!(
            html.contains(endpoint.path),
            "página raiz não lista {}",
            endpoint.path
        );
    }
}
