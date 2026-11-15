//! Value object de e-mail com normalização e validação de formato.
//!
//! Mantém caixa e espaços normalizados antes que o endereço participe das
//! regras de unicidade ou autenticação.

use std::fmt;
use std::str::FromStr;

use crate::error::DomainError;

/// Comprimento máximo de um endereço de e-mail, conforme a RFC 5321.
pub const MAX_EMAIL_LEN: usize = 254;

/// Comprimento máximo da parte local, conforme a RFC 5321.
pub const MAX_LOCAL_PART_LEN: usize = 64;

/// Comprimento máximo de um rótulo do domínio, conforme a RFC 1035.
const MAX_DOMAIN_LABEL_LEN: usize = 63;

/// Endereço de e-mail normalizado (trim + minúsculas) e validado.
///
/// O construtor é a única forma de criar um `Email`, então qualquer instância
/// carrega a garantia de formato e normalização.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Email(String);

impl Email {
    /// Valida e normaliza um endereço de e-mail.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::InvalidEmail`] quando o endereço é vazio, longo
    /// demais ou não segue o formato `local@dominio`.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, DomainError> {
        let normalized = value.as_ref().trim().to_lowercase();
        validate(&normalized)?;
        Ok(Self(normalized))
    }

    /// Devolve o endereço normalizado para persistência, log e comparação.
    ///
    /// A garantia de que o valor é um e-mail válido vem do construtor.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Email {
    /// Formata o e-mail normalizado como texto e retorna o resultado da escrita no formatter.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for Email {
    /// Expõe o e-mail normalizado como `&str` sem copiar a string.
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for Email {
    type Err = DomainError;
    /// Valida e normaliza o texto recebido como e-mail; retorna o value object ou `DomainError` se o formato for inválido.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Atalho para construir a variante de e-mail inválido.
fn invalid(reason: impl Into<String>) -> DomainError {
    DomainError::InvalidEmail(reason.into())
}

/// Valida um e-mail já normalizado (trim + minúsculas).
///
/// Aplica os limites da RFC 5321 e exige exatamente um `@` separando parte
/// local e domínio.
///
/// # Errors
///
/// Retorna [`DomainError::InvalidEmail`] com a razão específica da falha.
fn validate(value: &str) -> Result<(), DomainError> {
    if value.is_empty() {
        return Err(invalid("não pode ser vazio"));
    }

    if value.chars().count() > MAX_EMAIL_LEN {
        return Err(invalid(format!(
            "deve ter no máximo {MAX_EMAIL_LEN} caracteres"
        )));
    }

    let Some((local, domain)) = value.split_once('@') else {
        return Err(invalid("deve conter um '@'"));
    };

    if domain.contains('@') {
        return Err(invalid("deve conter exatamente um '@'"));
    }

    validate_local_part(local)?;
    validate_domain(domain)
}

/// Valida a parte local (antes do `@`): tamanho, pontos e caracteres permitidos.
///
/// # Errors
///
/// Retorna [`DomainError::InvalidEmail`] quando a parte local é vazia, longa
/// demais, começa/termina com ponto, tem pontos consecutivos ou contém
/// caractere fora de `a-z 0-9 . _ % + -`.
fn validate_local_part(local: &str) -> Result<(), DomainError> {
    if local.is_empty() {
        return Err(invalid("a parte local não pode ser vazia"));
    }

    if local.len() > MAX_LOCAL_PART_LEN {
        return Err(invalid(format!(
            "a parte local deve ter no máximo {MAX_LOCAL_PART_LEN} caracteres"
        )));
    }

    if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return Err(invalid(
            "a parte local não pode começar, terminar ou ter pontos consecutivos",
        ));
    }

    let valid_characters = local.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '%' | '+' | '-')
    });

    if !valid_characters {
        return Err(invalid("a parte local contém caracteres não permitidos"));
    }

    Ok(())
}

/// Valida o domínio (depois do `@`): rótulos, hífens e domínio de topo.
///
/// Exige ao menos um ponto, cada rótulo com até 63 caracteres alfanuméricos ou
/// `-` (sem começar/terminar com hífen) e TLD com pelo menos 2 caracteres.
///
/// # Errors
///
/// Retorna [`DomainError::InvalidEmail`] descrevendo o rótulo problemático.
fn validate_domain(domain: &str) -> Result<(), DomainError> {
    if domain.is_empty() {
        return Err(invalid("o domínio não pode ser vazio"));
    }

    if !domain.contains('.') {
        return Err(invalid("o domínio deve conter um ponto"));
    }

    if domain.starts_with('.') || domain.ends_with('.') || domain.contains("..") {
        return Err(invalid(
            "o domínio não pode começar, terminar ou ter pontos consecutivos",
        ));
    }

    for label in domain.split('.') {
        if label.len() > MAX_DOMAIN_LABEL_LEN {
            return Err(invalid(format!(
                "cada trecho do domínio deve ter no máximo {MAX_DOMAIN_LABEL_LEN} caracteres"
            )));
        }

        let valid_characters = label
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-');

        if !valid_characters {
            return Err(invalid("o domínio contém caracteres não permitidos"));
        }

        if label.starts_with('-') || label.ends_with('-') {
            return Err(invalid(
                "trechos do domínio não podem começar ou terminar com '-'",
            ));
        }
    }

    let top_level = domain.rsplit('.').next().unwrap_or_default();
    if top_level.len() < 2 {
        return Err(invalid(
            "o domínio de topo deve ter pelo menos 2 caracteres",
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifica o contrato identificado por `normalizes_case_and_whitespace` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn normalizes_case_and_whitespace() {
        let email = Email::parse("  Admin@Example.COM  ").expect("e-mail válido");

        assert_eq!(email.as_str(), "admin@example.com");
        assert_eq!(email.to_string(), "admin@example.com");
    }

    /// Verifica o contrato identificado por `accepts_addresses_with_common_symbols` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn accepts_addresses_with_common_symbols() {
        for value in [
            "user+tag@example.com",
            "first.last@sub.example.com",
            "user_name%test@example-domain.com",
        ] {
            assert!(Email::parse(value).is_ok(), "deveria aceitar {value}");
        }
    }

    /// Verifica o contrato identificado por `rejects_missing_at_sign` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_missing_at_sign() {
        assert!(matches!(
            Email::parse("user.example.com"),
            Err(DomainError::InvalidEmail(_))
        ));
    }

    /// Verifica o contrato identificado por `rejects_multiple_at_signs` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_multiple_at_signs() {
        assert!(Email::parse("user@example@com").is_err());
    }

    /// Verifica o contrato identificado por `rejects_empty_local_part` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_empty_local_part() {
        assert!(Email::parse("@example.com").is_err());
    }

    /// Verifica o contrato identificado por `rejects_empty_domain` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_empty_domain() {
        assert!(Email::parse("user@").is_err());
    }

    /// Verifica o contrato identificado por `rejects_domain_without_dot` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_domain_without_dot() {
        assert!(Email::parse("user@localhost").is_err());
    }

    /// Verifica o contrato identificado por `rejects_invalid_characters` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_invalid_characters() {
        assert!(Email::parse("user name@example.com").is_err());
        assert!(Email::parse("user@exa_mple.com").is_err());
    }

    /// Verifica o contrato identificado por `rejects_leading_and_trailing_dots` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_leading_and_trailing_dots() {
        assert!(Email::parse(".user@example.com").is_err());
        assert!(Email::parse("user.@example.com").is_err());
        assert!(Email::parse("user@.example.com").is_err());
        assert!(Email::parse("user@example.com.").is_err());
    }

    /// Verifica o contrato identificado por `rejects_too_long_addresses` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_too_long_addresses() {
        let local = "a".repeat(MAX_LOCAL_PART_LEN + 1);

        assert!(Email::parse(format!("{local}@example.com")).is_err());
    }

    /// Verifica o contrato identificado por `parses_from_str` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn parses_from_str() {
        let email: Email = "user@example.com".parse().expect("e-mail válido");

        assert_eq!(email.as_str(), "user@example.com");
    }
}
