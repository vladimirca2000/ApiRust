//! Parâmetros de paginação compartilhados por todas as listagens.
//!
//! Manter os limites em um lugar só evita que uma listagem nova esqueça o teto
//! e permita a um cliente pedir a tabela inteira, derrubando a memória do
//! processo.

/// Tamanho máximo de página aceito por qualquer listagem.
pub const MAX_PAGE_SIZE: i64 = 100;

/// Tamanho padrão de página quando o cliente não informa `limit`.
pub const DEFAULT_PAGE_SIZE: i64 = 20;

/// Normaliza `offset`/`limit` recebidos do cliente.
///
/// `offset` negativo vira 0 e `limit` é limitado ao intervalo
/// `1..=MAX_PAGE_SIZE`, de modo que um valor absurdo nunca chegue ao banco.
#[must_use]
pub fn normalize(offset: i64, limit: i64) -> (i64, i64) {
    (offset.max(0), limit.clamp(1, MAX_PAGE_SIZE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_limit_to_maximum() {
        assert_eq!(normalize(0, 10_000), (0, MAX_PAGE_SIZE));
    }

    #[test]
    fn raises_limit_to_at_least_one() {
        assert_eq!(normalize(0, 0), (0, 1));
        assert_eq!(normalize(0, -5), (0, 1));
    }

    #[test]
    fn turns_negative_offset_into_zero() {
        assert_eq!(normalize(-100, 10), (0, 10));
    }

    #[test]
    fn keeps_valid_values_untouched() {
        assert_eq!(normalize(40, 20), (40, 20));
    }
}
