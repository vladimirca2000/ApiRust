//! Página de desenvolvimento servida na raiz.
//!
//! A lista de endpoints é **gerada** a partir de [`crate::catalog`], nunca
//! escrita à mão: assim ela não volta a ficar desatualizada quando uma rota
//! nova entra. O HTML mantém uma conexão SSE aberta; quando o navegador a
//! encerra, o guard da sessão dispara o shutdown após o período de tolerância.

use axum::extract::State;
use axum::response::Html;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use std::convert::Infallible;
use std::fmt::Write as _;
use std::time::Duration;

use crate::AppState;
use crate::catalog::{Access, Endpoint, GROUPS};

/// Estado do processo exibido no cabeçalho da página.
#[derive(Debug, Clone)]
pub struct HealthSnapshot {
    /// `true` quando o banco respondeu ao `SELECT 1`.
    pub database_ready: bool,
    /// Nome do ambiente de execução (`development`/`production`).
    pub environment: &'static str,
}

/// Página inicial usada apenas em desenvolvimento.
///
/// Consulta o banco para mostrar o estado real de readiness e renderiza o
/// catálogo de endpoints.
pub async fn page(State(state): State<AppState>) -> Html<String> {
    let database_ready = state.database.ping().await.is_ok();
    let snapshot = HealthSnapshot {
        database_ready,
        environment: "development",
    };

    Html(render(&snapshot))
}

/// Renderiza o HTML completo da página para o snapshot informado.
///
/// Separado do handler para permitir testar o conteúdo sem subir servidor.
#[must_use]
pub fn render(snapshot: &HealthSnapshot) -> String {
    let mut html = String::with_capacity(8 * 1024);

    html.push_str(
        r#"<!doctype html>
<html lang="pt-BR">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>ApiRust · endpoints</title>
<style>
  :root { color-scheme: dark; --bg:#111316; --panel:#171a1f; --line:#2a2f36;
          --text:#e6e6e6; --muted:#9aa4b2; --green:#7ee787; --amber:#e3b341;
          --red:#f85149; --blue:#79c0ff; }
  * { box-sizing: border-box; }
  body { font-family: ui-monospace, SFMono-Regular, Consolas, monospace; background:var(--bg);
         color:var(--text); margin:0; padding:32px 16px; line-height:1.5; }
  main { width:min(920px, 100%); margin:0 auto; }
  header { border:1px solid var(--line); border-radius:12px; padding:22px 26px;
           background:var(--panel); margin-bottom:18px; }
  h1 { margin:0 0 6px; font-size:21px; }
  h2 { font-size:14px; text-transform:uppercase; letter-spacing:.08em;
       color:var(--muted); margin:26px 0 10px; font-weight:600; }
  p { color:var(--muted); margin:6px 0; font-size:13px; }
  .badges { display:flex; gap:8px; flex-wrap:wrap; margin-top:14px; }
  .badge { display:inline-flex; align-items:center; gap:6px; padding:4px 10px;
           border-radius:999px; font-size:12px; border:1px solid transparent; }
  .badge.ok { background:#11351f; color:var(--green); border-color:#1c4a2b; }
  .badge.warn { background:#3a2a10; color:var(--amber); border-color:#56401a; }
  .badge.err { background:#3d1518; color:var(--red); border-color:#5d2126; }
  table { width:100%; border-collapse:collapse; background:var(--panel);
          border:1px solid var(--line); border-radius:10px; overflow:hidden; }
  th, td { text-align:left; padding:9px 12px; font-size:13px; border-bottom:1px solid var(--line); }
  th { color:var(--muted); font-weight:500; font-size:11px; text-transform:uppercase;
       letter-spacing:.06em; background:#1b1f25; }
  tr:last-child td { border-bottom:none; }
  td.method { width:74px; font-weight:700; }
  .GET { color:var(--green); } .POST { color:var(--blue); }
  .PUT, .PATCH { color:var(--amber); } .DELETE { color:var(--red); }
  td.path { color:#e6e6e6; white-space:nowrap; }
  .access { font-size:11px; padding:2px 7px; border-radius:6px; border:1px solid var(--line);
            color:var(--muted); }
  .access.bearer { color:var(--amber); }
  td.desc { color:var(--muted); }
  footer { color:#6e7681; font-size:12px; margin-top:22px; }
  a { color:var(--blue); }
</style>
</head>
<body>
<main>
<header>
  <h1>ApiRust</h1>
  <p>Servidor de desenvolvimento. Os endpoints abaixo são gerados a partir do
     catálogo de rotas da própria aplicação.</p>
  <div class="badges">
    <span id="session" class="badge warn">conectando…</span>"#,
    );

    let (db_class, db_text) = if snapshot.database_ready {
        ("ok", "banco: pronto")
    } else {
        ("err", "banco: indisponível")
    };
    let _ = write!(html, r#"<span class="badge {db_class}">{db_text}</span>"#);
    let _ = write!(
        html,
        r#"<span class="badge">{}</span>"#,
        escape(snapshot.environment)
    );

    html.push_str(
        r#"
  </div>
</header>
"#,
    );

    for group in GROUPS {
        let _ = write!(html, "<h2>{}</h2>", escape(group.title));
        html.push_str(
            "<table><thead><tr><th>Método</th><th>Rota</th><th>Acesso</th>\
             <th>Descrição</th></tr></thead><tbody>",
        );

        for endpoint in group.endpoints {
            push_row(&mut html, endpoint);
        }

        html.push_str("</tbody></table>");
    }

    html.push_str(
        r#"<footer>
  Esta página mantém uma conexão SSE (<code>/api/dev/session</code>).
  Fechar a aba encerra o servidor após 10 segundos — um refresh não derruba,
  porque a tolerância é rearmada.
</footer>
</main>
<script>
  const badge = document.getElementById('session');
  const setBadge = (text, kind) => {
    badge.textContent = text;
    badge.className = 'badge ' + kind;
  };

  const source = new EventSource('/api/dev/session');
  source.addEventListener('ready', () => setBadge('sessão ativa', 'ok'));
  source.onopen = () => setBadge('sessão ativa', 'ok');
  source.onerror = () => setBadge('conexão perdida — reconectando…', 'warn');
</script>
</body>
</html>
"#,
    );

    html
}

/// Adiciona a linha de tabela de um endpoint.
fn push_row(html: &mut String, endpoint: &Endpoint) {
    let (access_class, access_text) = match endpoint.access {
        Access::Public => ("", endpoint.access.label().to_owned()),
        Access::Bearer => ("bearer", endpoint.access.label().to_owned()),
        Access::DevOnly => ("bearer", endpoint.access.label().to_owned()),
    };

    let _ = write!(
        html,
        r#"<tr><td class="method {method}">{method}</td><td class="path">{path}</td>"#,
        method = escape(endpoint.method),
        path = escape(endpoint.path),
    );
    let _ = write!(
        html,
        r#"<td><span class="access {access_class}">{access_text}</span></td>"#,
    );
    let _ = write!(
        html,
        r#"<td class="desc">{}</td></tr>"#,
        escape(endpoint.description)
    );
}

/// Escapa os caracteres que teriam significado em HTML.
///
/// O catálogo é constante e controlado, mas o escaping evita que uma descrição
/// futura com `<`, `>` ou `&` quebre a página.
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Canal SSE que mantém a sessão viva enquanto a aba estiver aberta.
pub async fn session(State(state): State<AppState>) -> impl IntoResponse {
    let connection = state.sessions.connection();

    let events = async_stream::stream! {
        // Mantém o guard vivo enquanto o stream existir; ao fechar a aba,
        // o stream é descartado e o guard dispara a contagem de shutdown.
        let _connection = connection;

        yield Ok::<Event, Infallible>(Event::default().event("ready").data("conectado"));

        let mut ticker = tokio::time::interval(Duration::from_secs(15));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;

        loop {
            ticker.tick().await;
            yield Ok(Event::default().comment("keep-alive"));
        }
    };

    let mut response = Sse::new(events)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response();

    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Snapshot usado nos testes de renderização.
    fn snapshot() -> HealthSnapshot {
        HealthSnapshot {
            database_ready: true,
            environment: "development",
        }
    }

    /// Verifica o contrato identificado por `page_lists_every_endpoint_from_the_catalog` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn page_lists_every_endpoint_from_the_catalog() {
        let html = render(&snapshot());

        for endpoint in crate::catalog::all_endpoints() {
            assert!(
                html.contains(endpoint.path),
                "página não lista a rota {}",
                endpoint.path
            );
            assert!(
                html.contains(endpoint.description),
                "página não lista a descrição de {}",
                endpoint.path
            );
        }
    }

    /// Verifica o contrato identificado por `page_does_not_hardcode_stale_phase_text` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn page_does_not_hardcode_stale_phase_text() {
        let html = render(&snapshot());

        assert!(
            !html.contains("Fase 0"),
            "a página não pode anunciar 'Fase 0': o servidor já passou da Fase 3"
        );
        assert!(
            !html.contains("a partir da Fase"),
            "a página não pode prometer rotas 'a partir da fase'"
        );
    }

    /// Verifica o contrato identificado por `page_reports_database_state` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn page_reports_database_state() {
        let ready = render(&HealthSnapshot {
            database_ready: true,
            environment: "development",
        });
        assert!(ready.contains("banco: pronto"));

        let down = render(&HealthSnapshot {
            database_ready: false,
            environment: "development",
        });
        assert!(down.contains("banco: indisponível"));
    }

    /// Verifica o contrato identificado por `page_escapes_special_characters` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn page_escapes_special_characters() {
        assert_eq!(escape("<script>"), "&lt;script&gt;");
        assert_eq!(escape("a & b"), "a &amp; b");
    }
}
