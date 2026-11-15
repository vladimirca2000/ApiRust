//! Rate limit das rotas autenticadas de leitura.
//!
//! Login e refresh já tinham limite; as listagens não tinham nenhum. Isso
//! deixava `GET /api/v1/products` como vetor de scraping gratuito: um cliente
//! autenticado podia varrer o catálogo inteiro em laço, quantas vezes quisesse.
//!
//! A chave combina IP e caminho normalizado para que uma listagem pesada não
//! consuma a cota de outra, e o limite é alto o bastante (300/min) para não
//! atrapalhar paginação legítima.
//!
//! O limite é aplicado como **middleware nas rotas de leitura**, e não dentro de
//! cada handler: um handler novo que esquecesse a chamada reabriria o buraco.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::AppState;
use crate::metrics::normalize_route;
use crate::problem::{ProblemDetails, to_problem};
use crate::rate_limit::read_policy;

/// Tempo informado ao cliente quando o contador de leitura está fora do ar.
///
/// Mesmo prazo de uma janela cheia: sem contador, a resposta é "tente mais
/// tarde" em vez de liberar tráfego ilimitado.
const READ_LIMIT_UNAVAILABLE_RETRY_SECS: u64 = 60;

/// Middleware que consome uma unidade da cota de leitura por requisição.
///
/// Falha fechada: se o contador está indisponível, responde `429` em vez de
/// liberar tráfego ilimitado — a mesma escolha feita em login e refresh.
pub async fn middleware(State(state): State<AppState>, request: Request, next: Next) -> Response {
    // Sem `ConnectInfo` (router em memória nos testes), a chave usa um marcador
    // estável para manter o teste determinístico.
    // IP real do cliente: atrás de proxy confiável vem do `X-Forwarded-For`;
    // fora disso, o cabeçalho é ignorado para não permitir troca de identidade.
    let forwarded = request
        .headers()
        .get(crate::client_ip::FORWARDED_FOR_HEADER)
        .and_then(|value| value.to_str().ok());

    let ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(address)| {
            crate::client_ip::resolve_client_ip(address.ip(), &state.trusted_proxies, forwarded)
                .to_string()
        })
        .unwrap_or_else(|| "desconhecido".to_owned());

    let route = normalize_route(request.uri().path());
    let key = format!("read:{route}:{ip}");

    match state.rate_limiter.hit(&key, read_policy()).await {
        Ok(decision) => {
            if let Err(retry_after) = decision.into_result() {
                if let Some(metrics) = state.metrics.as_ref() {
                    metrics.record_rate_limited(&route);
                }
                return too_many_requests(retry_after);
            }
        }
        Err(error) => {
            tracing::error!(%error, "rate limit indisponível; recusando leitura");
            return too_many_requests(std::time::Duration::from_secs(
                READ_LIMIT_UNAVAILABLE_RETRY_SECS,
            ));
        }
    }

    next.run(request).await
}

/// Resposta `429` com o tempo restante até a janela reabrir.
///
/// Inclui `Retry-After`, para que clientes bem comportados esperem em vez de
/// insistir imediatamente.
#[must_use]
pub fn too_many_requests(retry_after: std::time::Duration) -> Response {
    let seconds = retry_after.as_secs().max(1);
    let mut response = to_problem(
        StatusCode::TOO_MANY_REQUESTS,
        ProblemDetails::new(
            "about:blank",
            "too_many_requests",
            StatusCode::TOO_MANY_REQUESTS.as_u16(),
            format!("tente novamente em {seconds} segundos"),
        ),
    )
    .into_response();

    if let Ok(value) = axum::http::HeaderValue::from_str(&seconds.to_string()) {
        response
            .headers_mut()
            .insert(axum::http::header::RETRY_AFTER, value);
    }

    response
}
