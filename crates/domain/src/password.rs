use std::fmt;

use crate::error::DomainError;

/// Comprimento mínimo aceito para senhas em texto puro.
pub const MIN_PASSWORD_LEN: usize = 12;

/// Comprimento máximo aceito para senhas em texto puro.
pub const MAX_PASSWORD_LEN: usize = 128;

/// Prefixo de um hash PHC produzido com Argon2id.
const ARGON2ID_PREFIX: &str = "$argon2id$";

/// Comprimento máximo do hash serializado, em bytes.
const MAX_HASH_LEN: usize = 255;

/// Quantidade mínima de segmentos separados por `$` num PHC completo:
/// vazio + algoritmo + versão + parâmetros + salt + hash.
const MIN_PHC_SEGMENTS: usize = 6;

/// Valida a política de senha antes de qualquer hashing.
///
/// Exige entre 12 e 128 caracteres, com letra minúscula, letra maiúscula,
/// número e símbolo. Espaços são permitidos (frases-senha); caracteres de
/// controle não.
///
/// # Errors
///
/// Retorna [`DomainError::InvalidPassword`] quando a senha não atende à
/// política.
pub fn validate_password(password: &str) -> Result<(), DomainError> {
    let length = password.chars().count();

    if length < MIN_PASSWORD_LEN {
        return Err(DomainError::InvalidPassword(format!(
            "deve ter pelo menos {MIN_PASSWORD_LEN} caracteres"
        )));
    }

    if length > MAX_PASSWORD_LEN {
        return Err(DomainError::InvalidPassword(format!(
            "deve ter no máximo {MAX_PASSWORD_LEN} caracteres"
        )));
    }

    if password.chars().any(char::is_control) {
        return Err(DomainError::InvalidPassword(
            "não pode conter caracteres de controle".to_owned(),
        ));
    }

    let has_lowercase = password.chars().any(char::is_lowercase);
    let has_uppercase = password.chars().any(char::is_uppercase);
    let has_digit = password.chars().any(|character| character.is_ascii_digit());
    let has_symbol = password
        .chars()
        .any(|character| !character.is_alphanumeric());

    if !(has_lowercase && has_uppercase && has_digit && has_symbol) {
        return Err(DomainError::InvalidPassword(
            "deve conter letra minúscula, letra maiúscula, número e símbolo".to_owned(),
        ));
    }

    Ok(())
}

/// Hash de senha já calculado, validado e pronto para persistência.
///
/// Nunca contém a senha em texto puro: apenas a representação PHC do Argon2id.
/// `Debug` é implementado manualmente para não expor o hash em logs.
#[derive(Clone, PartialEq, Eq)]
pub struct PasswordHash(String);

impl fmt::Debug for PasswordHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PasswordHash(***)")
    }
}

impl PasswordHash {
    /// Valida a representação PHC de um hash Argon2id.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::InvalidPasswordHash`] quando o valor não parece
    /// um hash Argon2id serializado.
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
        // Texto recebido do banco ou de um hash recém-gerado, ainda não validado.
        let raw = value.into();
        let trimmed = raw.trim();

        if trimmed.len() > MAX_HASH_LEN {
            return Err(invalid(format!(
                "deve ter no máximo {MAX_HASH_LEN} caracteres"
            )));
        }

        if !trimmed.starts_with(ARGON2ID_PREFIX) {
            return Err(invalid("deve ser um hash Argon2id (`$argon2id$...`)"));
        }

        if trimmed.split('$').count() < MIN_PHC_SEGMENTS || trimmed.chars().any(char::is_whitespace)
        {
            return Err(invalid("formato PHC incompleto"));
        }

        Ok(Self(trimmed.to_owned()))
    }

    /// Devolve a representação PHC para persistência e verificação.
    ///
    /// Este é o único valor derivado da senha que pode sair do domínio.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PasswordHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for PasswordHash {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Atalho para construir a variante de hash inválido.
fn invalid(reason: impl Into<String>) -> DomainError {
    DomainError::InvalidPasswordHash(reason.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRONG_PASSWORD: &str = "Senha-Forte-2026!";
    const VALID_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdGUtZGV0ZXN0ZQ$aGFzaC1kZS10ZXN0ZQ";

    #[test]
    fn accepts_strong_password() {
        assert!(validate_password(STRONG_PASSWORD).is_ok());
    }

    #[test]
    fn rejects_short_password() {
        assert!(matches!(
            validate_password("Curta-1!"),
            Err(DomainError::InvalidPassword(_))
        ));
    }

    #[test]
    fn rejects_password_without_all_character_classes() {
        for password in [
            "senhaforte-2026!",
            "SENHAFORTE-2026!",
            "SenhaForte-AAAA!",
            "SenhaForte20261",
        ] {
            assert!(
                validate_password(password).is_err(),
                "deveria rejeitar {password}"
            );
        }
    }

    #[test]
    fn rejects_control_characters() {
        assert!(validate_password("Senha-Forte-2026!\u{7}").is_err());
    }

    #[test]
    fn accepts_password_with_spaces() {
        assert!(validate_password("Senha forte 2026!").is_ok());
    }

    #[test]
    fn parses_argon2id_hash() {
        let hash = PasswordHash::parse(VALID_HASH).expect("hash válido");

        assert_eq!(hash.as_str(), VALID_HASH);
    }

    #[test]
    fn rejects_non_argon2id_hash() {
        assert!(PasswordHash::parse("texto-puro").is_err());
        assert!(PasswordHash::parse("$argon2i$v=19$m=1,t=1,p=1$c2FsdA$aGFzaA").is_err());
        assert!(PasswordHash::parse("$argon2id$v=19$m=1,t=1,p=1").is_err());
    }

    #[test]
    fn rejects_hash_with_whitespace() {
        assert!(PasswordHash::parse(format!("{VALID_HASH} extra")).is_err());
    }

    #[test]
    fn debug_does_not_leak_the_hash() {
        let hash = PasswordHash::parse(VALID_HASH).expect("hash válido");

        let debug = format!("{hash:?}");

        assert_eq!(debug, "PasswordHash(***)");
        assert!(
            !debug.contains("argon2id"),
            "o debug não pode conter o hash PHC"
        );
        assert!(!debug.contains(VALID_HASH));
    }
}
