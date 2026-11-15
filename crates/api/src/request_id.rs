//! Propagação do request ID até a criação da resposta de erro.
//!
//! Handlers devolvem `Response` já formatada e, nesse ponto, não têm mais acesso
//! à requisição — então não conseguem ler o `x-request-id` para preencher
//! `instance` no RFC 7807. A solução é guardar o valor num `task_local` durante
//! o processamento: cada requisição roda numa task própria, então os valores não
//! se misturam.
//!
//! Isso permite correlacionar o corpo do erro ao log correspondente: o cliente
//! reporta o `instance` e o operador acha a linha exata.

use std::future::Future;

use axum::extract::Request;
use axum::http::HeaderName;
use axum::middleware::Next;
use axum::response::Response;

/// Cabeçalho que carrega o identificador da requisição.
pub const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

tokio::task_local! {
    /// Identificador da requisição em processamento.
    static CURRENT_REQUEST_ID: String;
}

/// Executa o futuro com o request ID disponível para [`current_request_id`].
pub async fn scope<F>(request_id: String, future: F) -> F::Output
where
    F: Future,
{
    CURRENT_REQUEST_ID.scope(request_id, future).await
}

/// Devolve o request ID da tarefa atual, quando houver.
///
/// Fora do middleware (por exemplo em teste unitário) retorna `None`.
#[must_use]
pub fn current_request_id() -> Option<String> {
    CURRENT_REQUEST_ID.try_with(Clone::clone).ok()
}

/// Middleware que publica o request ID da requisição para o resto do pipeline.
///
/// Lê o cabeçalho já definido por `SetRequestIdLayer` e, quando ele não existe,
/// gera um valor para que `instance` nunca fique vazio.
pub async fn publish_request_id(mut request: Request, next: Next) -> Response {
    let incoming = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Guarda também na requisição, para handlers que queiram o valor direto.
    if let Ok(value) = axum::http::HeaderValue::from_str(&incoming) {
        request
            .headers_mut()
            .insert(REQUEST_ID_HEADER.clone(), value);
    }

    let mut response = scope(incoming.clone(), next.run(request)).await;

    // Garante que o cliente consiga correlacionar mesmo em respostas de erro.
    if !response.headers().contains_key(&REQUEST_ID_HEADER) {
        if let Ok(value) = axum::http::HeaderValue::from_str(&incoming) {
            response
                .headers_mut()
                .insert(REQUEST_ID_HEADER.clone(), value);
        }
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifica o contrato identificado por `current_request_id_is_none_outside_the_scope` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn current_request_id_is_none_outside_the_scope() {
        assert_eq!(current_request_id(), None);
    }

    /// Verifica o contrato identificado por `scope_exposes_the_request_id` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[tokio::test]
    async fn scope_exposes_the_request_id() {
        let id = scope("abc-123".to_owned(), async { current_request_id() }).await;

        assert_eq!(id.as_deref(), Some("abc-123"));
    }

    /// Verifica o contrato identificado por `scopes_do_not_leak_between_tasks` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[tokio::test]
    async fn scopes_do_not_leak_between_tasks() {
        let first = tokio::spawn(scope("primeiro".to_owned(), async {
            tokio::task::yield_now().await;
            current_request_id()
        }));
        let second = tokio::spawn(scope("segundo".to_owned(), async {
            tokio::task::yield_now().await;
            current_request_id()
        }));

        assert_eq!(first.await.expect("task").as_deref(), Some("primeiro"));
        assert_eq!(second.await.expect("task").as_deref(), Some("segundo"));
    }
}
