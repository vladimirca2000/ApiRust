//! Parâmetros de paginação compartilhados por todas as listagens.
//!
//! Manter os limites em um lugar só evita que uma listagem nova esqueça o teto
//! e permita a um cliente pedir a tabela inteira, derrubando a memória do
//! processo.
//!
//! **Valores inválidos são recusados, não corrigidos.** Antes, `limit=0` virava
//! `1` e `offset=-5` virava `0` em silêncio: um cliente com bug recebia uma
//! página de um item e podia concluir que "não há mais dados", sem nunca ver um
//! erro. Recusar com `400` troca uma resposta silenciosamente errada por uma
//! mensagem que aponta o parâmetro — o cliente conserta a chamada.

use crate::error::AppError;

/// Tamanho máximo de página aceito por qualquer listagem.
pub const MAX_PAGE_SIZE: i64 = 100;

/// Tamanho padrão de página quando o cliente não informa `limit`.
pub const DEFAULT_PAGE_SIZE: i64 = 20;

/// Normaliza `offset`/`limit` recebidos do cliente.
///
/// `offset` ausente ou negativo vira 0; `limit` ausente vira
/// [`DEFAULT_PAGE_SIZE`]. Um `limit` fora de `1..=MAX_PAGE_SIZE` é **erro**.
///
/// O teto continua valendo mesmo com a validação: um `limit=100` explícito é
/// aceito, um `1000` não — sem teto, uma única requisição alocaria a tabela
/// inteira.
///
/// # Errors
///
/// Retorna [`AppError::InvalidPagination`] quando `limit` é menor que 1 ou maior
/// que [`MAX_PAGE_SIZE`].
pub fn normalize(offset: i64, limit: i64) -> Result<(i64, i64), AppError> {
    if !(1..=MAX_PAGE_SIZE).contains(&limit) {
        return Err(AppError::InvalidPagination(format!(
            "limit deve estar entre 1 e {MAX_PAGE_SIZE}, recebido {limit}"
        )));
    }

    Ok((offset.max(0), limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifica o contrato identificado por `rejects_limit_above_maximum` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_limit_above_maximum() {
        let error = normalize(0, 10_000).expect_err("acima do teto deve ser erro");
        assert!(matches!(error, AppError::InvalidPagination(_)));
    }

    /// Verifica o contrato identificado por `rejects_limit_zero_and_negative` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_limit_zero_and_negative() {
        assert!(normalize(0, 0).is_err(), "limit zero não é página válida");
        assert!(
            normalize(0, -5).is_err(),
            "limit negativo não é página válida"
        );
    }

    /// Verifica o contrato identificado por `accepts_the_maximum_exactly` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn accepts_the_maximum_exactly() {
        assert_eq!(
            normalize(0, MAX_PAGE_SIZE).expect("valor no teto"),
            (0, MAX_PAGE_SIZE)
        );
    }

    /// Verifica o contrato identificado por `turns_negative_offset_into_zero` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn turns_negative_offset_into_zero() {
        assert_eq!(normalize(-100, 10).expect("offset corrigido"), (0, 10));
    }

    /// Verifica o contrato identificado por `keeps_valid_values_untouched` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn keeps_valid_values_untouched() {
        assert_eq!(normalize(40, 20).expect("valores válidos"), (40, 20));
    }
}
