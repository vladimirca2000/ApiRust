//! Métricas Prometheus: contadores, histograma de latência e endpoint `/metrics`.
//!
//! As métricas são registradas por um middleware na borda HTTP, não nos
//! handlers: assim toda rota entra na contagem, inclusive as de erro e as de
//! documentação, sem ninguém precisar lembrar de instrumentar cada uma.
//!
//! **Proteção do endpoint**: `/metrics` revela nomes de rotas, volume de
//! requisições e taxa de erro — material útil para reconhecimento. O acesso é
//! restrito por token (`METRICS_TOKEN`); sem token configurado, a rota responde
//! `404` (não `403`, que confirmaria a existência).

use std::time::Instant;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, IntGaugeVec, Opts, Registry, TextEncoder,
};

use crate::AppState;

/// Caminho do endpoint de métricas.
pub const METRICS_PATH: &str = "/metrics";

/// Nome do cabeçalho que carrega o token de acesso às métricas.
pub const METRICS_TOKEN_HEADER: &str = "x-metrics-token";

/// Buckets do histograma de latência, em segundos.
///
/// Cobrem de 5 ms (consulta rápida) a 10 s (quase o timeout de 30 s), com
/// resolução suficiente para distinguir p50 de p99 em uma API de CRUD.
/// São valores fixos de instrumentação, não parâmetros de negócio.
const LATENCY_BUCKET_MS_5: f64 = 0.005;

/// Bucket de 10 ms.
const LATENCY_BUCKET_MS_10: f64 = 0.01;
/// Bucket de 25 ms.
const LATENCY_BUCKET_MS_25: f64 = 0.025;
/// Bucket de 50 ms.
const LATENCY_BUCKET_MS_50: f64 = 0.05;
/// Bucket de 100 ms.
const LATENCY_BUCKET_MS_100: f64 = 0.1;
/// Bucket de 250 ms.
const LATENCY_BUCKET_MS_250: f64 = 0.25;
/// Bucket de 500 ms.
const LATENCY_BUCKET_MS_500: f64 = 0.5;
/// Bucket de 1 s.
const LATENCY_BUCKET_S_1: f64 = 1.0;
/// Bucket de 2,5 s.
const LATENCY_BUCKET_S_2_5: f64 = 2.5;
/// Bucket de 5 s.
const LATENCY_BUCKET_S_5: f64 = 5.0;
/// Bucket de 10 s.
const LATENCY_BUCKET_S_10: f64 = 10.0;

/// Coleção de buckets usada pelo histograma de latência.
const LATENCY_BUCKETS: [f64; 11] = [
    LATENCY_BUCKET_MS_5,
    LATENCY_BUCKET_MS_10,
    LATENCY_BUCKET_MS_25,
    LATENCY_BUCKET_MS_50,
    LATENCY_BUCKET_MS_100,
    LATENCY_BUCKET_MS_250,
    LATENCY_BUCKET_MS_500,
    LATENCY_BUCKET_S_1,
    LATENCY_BUCKET_S_2_5,
    LATENCY_BUCKET_S_5,
    LATENCY_BUCKET_S_10,
];

/// Conjunto de métricas da aplicação.
#[derive(Clone)]
pub struct Metrics {
    /// Registro que agrupa todas as métricas desta instância.
    registry: Registry,
    /// Total de requisições, por método, rota e status.
    requests: IntCounterVec,
    /// Latência das requisições em segundos, por método e rota.
    latency: HistogramVec,
    /// Requisições em voo neste instante.
    in_flight: IntGaugeVec,
    /// Contador de rate limit negado, por rota.
    rate_limited: IntCounterVec,
}

impl Metrics {
    /// Cria o conjunto de métricas e o registra.
    ///
    /// # Errors
    ///
    /// Retorna erro se algum nome/coletor for inválido; acontece apenas por
    /// defeito de código, não por configuração.
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();

        let requests = IntCounterVec::new(
            Opts::new("api_http_requests_total", "Total de requisições HTTP"),
            &["method", "route", "status"],
        )?;
        let latency = HistogramVec::new(
            HistogramOpts::new(
                "api_http_request_duration_seconds",
                "Latência das requisições",
            )
            .buckets(LATENCY_BUCKETS.to_vec()),
            &["method", "route"],
        )?;
        let in_flight = IntGaugeVec::new(
            Opts::new("api_http_in_flight", "Requisições em processamento"),
            &["method"],
        )?;
        let rate_limited = IntCounterVec::new(
            Opts::new(
                "api_rate_limited_total",
                "Requisições recusadas por rate limit",
            ),
            &["route"],
        )?;

        registry.register(Box::new(requests.clone()))?;
        registry.register(Box::new(latency.clone()))?;
        registry.register(Box::new(in_flight.clone()))?;
        registry.register(Box::new(rate_limited.clone()))?;

        Ok(Self {
            registry,
            requests,
            latency,
            in_flight,
            rate_limited,
        })
    }

    /// Serializa todas as métricas no formato de texto do Prometheus.
    ///
    /// # Errors
    ///
    /// Retorna erro se a codificação falhar (praticamente impossível com o
    /// `TextEncoder`, mas a assinatura exige).
    pub fn encode(&self) -> Result<String, prometheus::Error> {
        // Bytes de saída do encoder; capacidade inicial só evita realocação.
        let mut buffer = Vec::new();
        TextEncoder::new().encode(&self.registry.gather(), &mut buffer)?;
        String::from_utf8(buffer).map_err(|error| {
            prometheus::Error::Msg(format!("saída de métricas não é texto válido: {error}"))
        })
    }

    /// Registra uma requisição concluída.
    fn observe(&self, method: &str, route: &str, status: u16, elapsed_secs: f64) {
        let status = status.to_string();
        self.requests
            .with_label_values(&[method, route, &status])
            .inc();
        self.latency
            .with_label_values(&[method, route])
            .observe(elapsed_secs);
    }

    /// Registra uma recusa por rate limit.
    pub fn record_rate_limited(&self, route: &str) {
        self.rate_limited.with_label_values(&[route]).inc();
    }

    /// Incrementa o medidor de requisições em voo.
    fn enter(&self, method: &str) {
        self.in_flight.with_label_values(&[method]).inc();
    }

    /// Decrementa o medidor de requisições em voo.
    fn leave(&self, method: &str) {
        self.in_flight.with_label_values(&[method]).dec();
    }
}

/// Middleware que mede cada requisição e alimenta as métricas.
///
/// O rótulo `route` usa o caminho bruto da requisição. Isso inclui IDs de
/// recurso e criaria cardinalidade ilimitada; por isso o caminho é normalizado
/// por [`normalize_route`] antes de virar rótulo.
pub async fn track(state: State<AppState>, request: Request, next: Next) -> Response {
    let Some(metrics) = state.metrics.clone() else {
        return next.run(request).await;
    };

    let method = request.method().as_str().to_owned();
    let route = normalize_route(request.uri().path());

    metrics.enter(&method);
    let started = Instant::now();
    let response = next.run(request).await;
    let elapsed = started.elapsed().as_secs_f64();
    metrics.leave(&method);

    metrics.observe(&method, &route, response.status().as_u16(), elapsed);
    response
}

/// Reduz o caminho a um rótulo estável, substituindo segmentos variáveis.
///
/// `/api/v1/users/8f3a...` vira `/api/v1/users/{id}`. Sem isso, cada usuário
/// criaria uma série temporal nova e o Prometheus acumularia memória até cair.
#[must_use]
pub fn normalize_route(path: &str) -> String {
    let mut normalized: Vec<String> = Vec::new();

    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        // Segmento seguinte a `users`, `categories` ou `products` é um UUID.
        let is_identifier = normalized.last().is_some_and(|previous| {
            matches!(previous.as_str(), "users" | "categories" | "products")
        });

        if is_identifier {
            normalized.push("{id}".to_owned());
        } else {
            normalized.push(segment.to_owned());
        }
    }

    format!("/{}", normalized.join("/"))
}

/// Rotas de métricas, montadas apenas quando existe configuração.
///
/// A rota é servida sob o token configurado em `METRICS_TOKEN`; sem token, a
/// rota nem é montada e responde `404`.
pub fn routes() -> Router<AppState> {
    Router::new().route(METRICS_PATH, get(metrics_endpoint))
}

/// Serve o texto de métricas quando o token confere.
///
/// # Errors
///
/// Responde `404` quando o token está ausente/errado — o mesmo status de uma
/// rota inexistente, para não confirmar que `/metrics` existe.
async fn metrics_endpoint(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(expected) = state.metrics_token.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let provided = headers
        .get(METRICS_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok());

    if !constant_time_eq(provided, Some(expected)) {
        return StatusCode::NOT_FOUND.into_response();
    }

    let Some(metrics) = state.metrics.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match metrics.encode() {
        Ok(body) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
            body,
        )
            .into_response(),
        Err(error) => {
            tracing::error!(%error, "falha ao serializar métricas");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Compara dois tokens em tempo constante em relação ao conteúdo.
///
/// Comparar com `==` encerra no primeiro byte diferente e vaza, por latência,
/// quantos caracteres do token estavam corretos.
#[must_use]
pub fn constant_time_eq(provided: Option<&str>, expected: Option<&str>) -> bool {
    let (Some(provided), Some(expected)) = (provided, expected) else {
        return false;
    };

    let provided = provided.as_bytes();
    let expected = expected.as_bytes();
    if provided.len() != expected.len() {
        return false;
    }

    provided
        .iter()
        .zip(expected)
        .fold(0u8, |accumulator, (left, right)| {
            accumulator | (left ^ right)
        })
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifica o contrato identificado por `normalizes_identifier_segments` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn normalizes_identifier_segments() {
        assert_eq!(
            normalize_route("/api/v1/users/8f3a1c2e-1111-2222-3333-444455556666"),
            "/api/v1/users/{id}"
        );
        assert_eq!(normalize_route("/api/v1/products"), "/api/v1/products");
        assert_eq!(normalize_route("/health"), "/health");
    }

    /// Verifica o contrato identificado por `normalizes_role_path` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn normalizes_role_path() {
        assert_eq!(
            normalize_route("/api/v1/users/abc/role"),
            "/api/v1/users/{id}/role"
        );
    }

    /// Verifica o contrato identificado por `constant_time_comparison_rejects_different_values` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn constant_time_comparison_rejects_different_values() {
        assert!(constant_time_eq(Some("segredo"), Some("segredo")));
        assert!(!constant_time_eq(Some("segredo"), Some("segred0")));
        assert!(!constant_time_eq(Some("curto"), Some("mais-longo")));
        assert!(!constant_time_eq(None, Some("segredo")));
        assert!(!constant_time_eq(Some("segredo"), None));
    }

    /// Verifica o contrato identificado por `metrics_encode_exposes_counters` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn metrics_encode_exposes_counters() {
        let metrics = Metrics::new().expect("registro válido");
        metrics.observe("GET", "/health", 200, 0.01);

        let text = metrics.encode().expect("codificar métricas");

        assert!(text.contains("api_http_requests_total"));
        assert!(text.contains("route=\"/health\""));
    }
}
