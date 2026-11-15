//! Valor monetário em unidades mínimas.
//!
//! Dinheiro **nunca** é representado por `f32`/`f64`: ponto flutuante binário
//! não representa `0.10` exatamente e acumula erro em somas. Toda a aplicação
//! usa inteiros em centavos, e a formatação para exibição acontece só na borda.

use std::fmt;

use crate::error::DomainError;

/// Valor monetário positivo, armazenado em centavos.
///
/// A invariante é `centavos > 0`: preço zero ou negativo não representa um
/// produto vendável e é rejeitado na construção. Isso espelha o
/// `CHECK (price_cents > 0)` do schema, de modo que domínio e banco concordam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Money(i64);

impl Money {
    /// Cria um valor a partir de centavos.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::InvalidMoney`] quando o valor é zero ou negativo.
    pub fn from_cents(cents: i64) -> Result<Self, DomainError> {
        if cents <= 0 {
            return Err(DomainError::InvalidMoney(
                "o valor deve ser maior que zero".to_owned(),
            ));
        }
        Ok(Self(cents))
    }

    /// Devolve o valor em centavos, pronto para persistência.
    #[must_use]
    pub const fn cents(self) -> i64 {
        self.0
    }
}

/// Quantidade de centavos em uma unidade monetária.
const CENTS_PER_UNIT: i64 = 100;

/// Largura mínima dos centavos na formatação (`5` vira `05`).
const CENTS_WIDTH: usize = 2;

impl fmt::Display for Money {
    /// Formata no padrão brasileiro, com vírgula decimal e sem símbolo.
    ///
    /// Usado apenas para exibição; o transporte na API é sempre em centavos.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let units = self.0 / CENTS_PER_UNIT;
        let cents = self.0 % CENTS_PER_UNIT;
        write!(formatter, "{units},{cents:0width$}", width = CENTS_WIDTH)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifica o contrato identificado por `accepts_positive_values` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn accepts_positive_values() {
        assert_eq!(Money::from_cents(1).expect("válido").cents(), 1);
        assert_eq!(Money::from_cents(9_999).expect("válido").cents(), 9_999);
    }

    /// Verifica o contrato identificado por `rejects_zero_and_negative` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_zero_and_negative() {
        assert!(matches!(
            Money::from_cents(0),
            Err(DomainError::InvalidMoney(_))
        ));
        assert!(matches!(
            Money::from_cents(-1),
            Err(DomainError::InvalidMoney(_))
        ));
    }

    /// Verifica o contrato identificado por `formats_with_brazilian_decimal_separator` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn formats_with_brazilian_decimal_separator() {
        assert_eq!(
            Money::from_cents(1_250).expect("válido").to_string(),
            "12,50"
        );
        assert_eq!(Money::from_cents(5).expect("válido").to_string(), "0,05");
        assert_eq!(Money::from_cents(100).expect("válido").to_string(), "1,00");
    }

    /// Verifica o contrato identificado por `orders_by_numeric_value` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn orders_by_numeric_value() {
        let cheap = Money::from_cents(100).expect("válido");
        let expensive = Money::from_cents(10_000).expect("válido");

        assert!(cheap < expensive);
    }
}
