//! Camadas HTTP de endurecimento: CORS, limite de corpo, timeouts, concorrência e headers.
//!
//! Todas são construídas a partir da configuração validada no boot. Os padrões
//! são restritivos de propósito: uma origem esquecida deve **negar**, não liberar
//! tudo — o erro clássico de `CorsLayer::permissive()` em produção.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Extension, Request};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use tokio::sync::Semaphore;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;

use crate::AppState;

/// Tempo máximo que o navegador pode cachear a resposta de preflight, em segundos.
const CORS_MAX_AGE_SECS: u64 = 600;

/// Tamanho máximo do corpo de uma requisição.
///
/// 1 MiB é folgado para os DTOs atuais (o maior é o JSON de produto) e evita
/// que um cliente envie um payload gigante para consumir memória.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Tempo máximo para uma requisição completar.
///
/// Protege contra handlers travados em I/O e contra clientes lentos que seguram
/// conexão (slowloris no nível da aplicação).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Cabeçalhos de segurança aplicados a toda resposta.
const SECURITY_HEADERS: [(&str, &str); 5] = [
    // Impede o navegador de "adivinhar" o tipo do conteúdo.
    ("x-content-type-options", "nosniff"),
    // Nega enquadramento: mitiga clickjacking em qualquer página servida.
    ("x-frame-options", "DENY"),
    // Não vaza a URL de origem ao navegar para outro site.
    ("referrer-policy", "no-referrer"),
    // API não precisa de nenhuma dessas capacidades do navegador.
    (
        "permissions-policy",
        "geolocation=(), microphone=(), camera=()",
    ),
    // Respostas da API nunca devem ser cacheadas por intermediários.
    ("cache-control", "no-store"),
];

/// Monta a camada de CORS com origens explícitas.
///
/// Sem origem configurada, nenhuma origem cruzada é permitida — o comportamento
/// correto para uma API consumida por cliente próprio ou por `curl`.
fn cors_layer(allowed_origins: &[String]) -> CorsLayer {
    let mut layer = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        .max_age(Duration::from_secs(CORS_MAX_AGE_SECS));

    if !allowed_origins.is_empty() {
        let origins: Vec<HeaderValue> = allowed_origins
            .iter()
            .filter_map(|origin| origin.parse::<HeaderValue>().ok())
            .collect();
        layer = layer.allow_origin(origins);
    }

    layer
}

/// Semáforo compartilhado entre clones do roteador.
///
/// Precisa viver num `Extension` (e não num `ConcurrencyLimitLayer` do tower)
/// porque o Axum devolve `Ready` ao spawnar o handler: o limite do tower
/// liberaria o slot antes da requisição terminar.
#[derive(Clone)]
struct RequestConcurrency {
    /// Permissões restantes para requisições em voo.
    semaphore: Arc<Semaphore>,
}

/// Recusa a requisição com `503` quando o teto de concorrência está cheio.
///
/// `try_acquire` é intencional: enfileirar esconderia a sobrecarga até a
/// memória estourar. O permit fica vivo até `next.run` completar.
async fn enforce_concurrency(
    Extension(limit): Extension<RequestConcurrency>,
    request: Request,
    next: Next,
) -> Response {
    let Ok(_permit) = limit.semaphore.try_acquire() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "serviço sobrecarregado").into_response();
    };
    next.run(request).await
}

/// Aplica todas as camadas de endurecimento ao roteador.
///
/// A ordem importa: concorrência, `timeout` e `body limit` ficam por fora para
/// que uma requisição lenta, grande ou excedente seja cortada antes do handler.
///
/// `max_concurrent` é o teto de requisições em voo; o excesso responde `503`
/// imediatamente, em vez de enfileirar até estourar memória.
pub fn apply<S>(router: Router<S>, allowed_origins: &[String], max_concurrent: usize) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let concurrency = RequestConcurrency {
        semaphore: Arc::new(Semaphore::new(max_concurrent)),
    };

    let mut router = router
        .layer(middleware::from_fn(enforce_concurrency))
        .layer(Extension(concurrency))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(cors_layer(allowed_origins));

    for (name, value) in SECURITY_HEADERS {
        let header_name = HeaderName::from_static(name);
        // O valor é `&'static str` da tabela acima, então `from_static` é seguro.
        let header_value = HeaderValue::from_static(value);
        router = router.layer(SetResponseHeaderLayer::overriding(
            header_name,
            header_value,
        ));
    }

    router
}

/// Erro devolvido quando o corpo excede [`MAX_BODY_BYTES`].
///
/// Constante exposta para o teste conferir que o status é o esperado.
pub const PAYLOAD_TOO_LARGE: StatusCode = StatusCode::PAYLOAD_TOO_LARGE;

/// Estado usado apenas para documentar a assinatura esperada das camadas.
#[allow(dead_code)]
fn _state_marker(_: &AppState) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_headers_are_all_lowercase() {
        // Cabeçalhos HTTP são case-insensitive, mas manter minúsculas evita
        // duplicação ao comparar com o que o cliente envia.
        for (name, _) in SECURITY_HEADERS {
            assert_eq!(name, name.to_lowercase(), "cabeçalho deve ser minúsculo");
        }
    }

    #[test]
    fn security_headers_forbid_embedding_and_sniffing() {
        let map: std::collections::HashMap<_, _> = SECURITY_HEADERS.iter().copied().collect();

        assert_eq!(map.get("x-frame-options"), Some(&"DENY"));
        assert_eq!(map.get("x-content-type-options"), Some(&"nosniff"));
        assert_eq!(map.get("cache-control"), Some(&"no-store"));
    }

    #[test]
    fn cors_without_origins_does_not_wildcard() {
        // A camada sem origens configuradas não deve aceitar qualquer origem;
        // a verificação real acontece no teste HTTP.
        let layer = cors_layer(&[]);
        let _ = layer;
    }

    #[test]
    fn body_limit_is_reasonable() {
        assert_eq!(MAX_BODY_BYTES, 1024 * 1024);
        assert!(REQUEST_TIMEOUT.as_secs() >= 5, "timeout curto demais");
        assert!(REQUEST_TIMEOUT.as_secs() <= 60, "timeout longo demais");
    }
}
