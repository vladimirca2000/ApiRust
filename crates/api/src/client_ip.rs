//! Resolução do IP do cliente, com suporte opcional a proxy confiável.
//!
//! **O problema.** A chave do rate limit é o IP. Lendo apenas o `peer_addr` do
//! socket, uma API atrás de reverse proxy vê **um único IP** — o do proxy — e
//! todos os clientes compartilham a mesma cota. Um cliente pode esgotar o limite
//! de login (5/min) e derrubar a autenticação de todo mundo.
//!
//! **A armadilha da correção óbvia.** Ler `X-Forwarded-For` sempre é pior: o
//! cabeçalho é enviado pelo cliente e qualquer um pode forjar um IP novo a cada
//! requisição, escapando do limite para sempre.
//!
//! **A regra adotada.** O cabeçalho é usado **apenas** quando o peer direto é um
//! proxy declarado em `TRUSTED_PROXIES`. Nesse caso, o IP do cliente é o
//! **último** endereço da lista que não seja ele próprio um proxy confiável —
//! os anteriores foram escritos pelo caminho e podem ter sido forjados pelo
//! cliente original. Sem proxies configurados, o comportamento é o de antes:
//! só o `peer_addr`.

use std::net::IpAddr;

/// Cabeçalho padrão de proxy com a cadeia de IPs de origem.
pub const FORWARDED_FOR_HEADER: &str = "x-forwarded-for";

/// Resolve o IP do cliente a partir do peer e da cadeia de proxies.
///
/// `trusted` é a lista de IPs de proxy declarados na configuração; `forwarded`
/// é o valor bruto de `X-Forwarded-For`, quando presente.
///
/// # Comportamento
///
/// - Peer não confiável: devolve o peer. O cabeçalho é ignorado, porque qualquer
///   cliente pode forjá-lo.
/// - Peer confiável: percorre a cadeia da direita para a esquerda e devolve o
///   primeiro endereço que **não** seja um proxy confiável. Esse é o primeiro
///   salto que o proxy confiável não escreveu.
/// - Cadeia vazia ou ilegível com peer confiável: devolve o peer. Melhor limitar
///   pelo IP do proxy do que aceitar um valor não interpretável.
#[must_use]
pub fn resolve_client_ip(peer: IpAddr, trusted: &[IpAddr], forwarded: Option<&str>) -> IpAddr {
    if !trusted.contains(&peer) {
        return peer;
    }

    let Some(header) = forwarded else {
        return peer;
    };

    // Da direita para a esquerda: o último elemento foi escrito pelo proxy mais
    // próximo, e é o único em que se pode confiar.
    for candidate in header.rsplit(',').map(str::trim) {
        let Ok(ip) = candidate.parse::<IpAddr>() else {
            // Valor ilegível: não há como confiar no resto da cadeia.
            return peer;
        };

        if !trusted.contains(&ip) {
            return ip;
        }
    }

    peer
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IP de cliente usado nos testes.
    fn client() -> IpAddr {
        "203.0.113.7".parse().expect("IP válido")
    }

    /// IP de proxy usado nos testes.
    fn proxy() -> IpAddr {
        "10.0.0.2".parse().expect("IP válido")
    }

    /// Verifica o contrato identificado por `ignores_header_from_untrusted_peer` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn ignores_header_from_untrusted_peer() {
        // Cliente forja o cabeçalho: continua contado pelo próprio IP.
        let resolved = resolve_client_ip(client(), &[proxy()], Some("1.2.3.4"));

        assert_eq!(
            resolved,
            client(),
            "cabeçalho de peer não confiável não pode ser aceito"
        );
    }

    /// Verifica o contrato identificado por `uses_header_from_trusted_proxy` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn uses_header_from_trusted_proxy() {
        let resolved = resolve_client_ip(proxy(), &[proxy()], Some("203.0.113.7"));

        assert_eq!(resolved, client());
    }

    /// Verifica o contrato identificado por `takes_the_last_untrusted_hop` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn takes_the_last_untrusted_hop() {
        // Cadeia: cliente forjou "9.9.9.9", o proxy confiável anexou o IP real.
        // O correto é o último salto não confiável, não o forjado.
        let resolved =
            resolve_client_ip(proxy(), &[proxy()], Some("9.9.9.9, 203.0.113.7, 10.0.0.2"));

        assert_eq!(
            resolved,
            client(),
            "o IP forjado à esquerda não pode ser usado"
        );
    }

    /// Verifica o contrato identificado por `skips_multiple_trusted_proxies` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn skips_multiple_trusted_proxies() {
        let second = "10.0.0.3".parse::<IpAddr>().expect("IP válido");
        let resolved =
            resolve_client_ip(proxy(), &[proxy(), second], Some("203.0.113.7, 10.0.0.3"));

        assert_eq!(resolved, client());
    }

    /// Verifica o contrato identificado por `falls_back_to_peer_without_header` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn falls_back_to_peer_without_header() {
        assert_eq!(resolve_client_ip(proxy(), &[proxy()], None), proxy());
    }

    /// Verifica o contrato identificado por `falls_back_to_peer_on_garbage_header` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn falls_back_to_peer_on_garbage_header() {
        let resolved = resolve_client_ip(proxy(), &[proxy()], Some("nao-e-um-ip"));

        assert_eq!(
            resolved,
            proxy(),
            "valor ilegível não pode virar um balde de limite compartilhado"
        );
    }

    /// Verifica o contrato identificado por `without_trusted_proxies_behaves_as_before` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn without_trusted_proxies_behaves_as_before() {
        let resolved = resolve_client_ip(client(), &[], Some("1.2.3.4"));

        assert_eq!(resolved, client());
    }
}
