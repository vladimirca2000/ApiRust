//! Stock Keeping Unit (SKU): identificador comercial único do produto.
//!
//! O SKU é normalizado em maiúsculas e restrito a um alfabeto conservador, para
//! que ele seja seguro em URL, planilha, etiqueta e leitor de código de barras
//! sem precisar de escaping em cada destino.

use std::fmt;

use crate::error::DomainError;

/// Comprimento mínimo aceito para um SKU.
pub const MIN_SKU_LEN: usize = 3;
/// Comprimento máximo aceito para um SKU.
pub const MAX_SKU_LEN: usize = 64;

/// SKU normalizado e validado.
///
/// A invariante é o formato: letras, dígitos, `-`, `_` e `.`, sem espaço nem
/// caractere de controle. Maiúsculas porque `sku-1` e `SKU-1` são o mesmo
/// produto comercial e a unicidade no banco é sensível a caixa.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Sku(String);

impl Sku {
    /// Normaliza (trim + maiúsculas) e valida o SKU.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::InvalidSku`] quando o valor é vazio, curto demais,
    /// longo demais ou contém caractere fora do alfabeto permitido.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, DomainError> {
        let normalized = value.as_ref().trim().to_uppercase();

        let length = normalized.chars().count();
        if length < MIN_SKU_LEN {
            return Err(invalid(format!(
                "deve ter pelo menos {MIN_SKU_LEN} caracteres"
            )));
        }
        if length > MAX_SKU_LEN {
            return Err(invalid(format!(
                "deve ter no máximo {MAX_SKU_LEN} caracteres"
            )));
        }

        let valid = normalized.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        });

        if !valid {
            return Err(invalid(
                "use apenas letras, dígitos e os separadores `-`, `_` e `.`",
            ));
        }

        Ok(Self(normalized))
    }

    /// Devolve o SKU normalizado para persistência e comparação.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Sku {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Atalho para construir a variante de SKU inválido.
fn invalid(reason: impl Into<String>) -> DomainError {
    DomainError::InvalidSku(reason.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_and_whitespace() {
        let sku = Sku::parse("  abc-123  ").expect("sku válido");

        assert_eq!(sku.as_str(), "ABC-123");
    }

    #[test]
    fn accepts_common_separators() {
        for value in ["ABC-123", "SKU_1", "PROD.001", "X1Y2Z3"] {
            assert!(Sku::parse(value).is_ok(), "deveria aceitar {value}");
        }
    }

    #[test]
    fn rejects_too_short() {
        assert!(matches!(Sku::parse("AB"), Err(DomainError::InvalidSku(_))));
    }

    #[test]
    fn rejects_too_long() {
        let long = "A".repeat(MAX_SKU_LEN + 1);
        assert!(matches!(Sku::parse(long), Err(DomainError::InvalidSku(_))));
    }

    #[test]
    fn rejects_invalid_characters() {
        for value in ["ABC 123", "ABC/123", "ABC#123", "AÇA-1"] {
            assert!(Sku::parse(value).is_err(), "deveria rejeitar {value:?}");
        }
    }

    #[test]
    fn case_insensitive_equality_after_normalization() {
        let lower = Sku::parse("sku-1").expect("válido");
        let upper = Sku::parse("SKU-1").expect("válido");

        assert_eq!(lower, upper, "a normalização deve tornar os dois iguais");
    }
}
