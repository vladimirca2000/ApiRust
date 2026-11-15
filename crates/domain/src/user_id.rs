//! Value object para identificadores UUID de usuários e entidades relacionadas.
//!
//! Evita espalhar conversões de UUID e validação textual pelas camadas da aplicação.

use std::fmt;
use std::str::FromStr;

use uuid::Uuid;

use crate::error::DomainError;

/// Identificador opaco de usuário, baseado em UUID v4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserId(Uuid);

impl UserId {
    /// Gera um identificador novo e aleatório (UUID v4).
    ///
    /// Usado apenas na criação de entidades; registros vindos do banco passam
    /// por [`UserId::parse`].
    #[must_use]
    pub fn new_v4() -> Self {
        Self(Uuid::new_v4())
    }

    /// Embrulha um UUID já validado, sem gerar novo valor.
    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    /// Devolve o UUID interno para serialização ou bind no banco.
    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }

    /// # Errors
    ///
    /// Retorna [`DomainError::InvalidId`] quando o texto não é um UUID válido.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        Uuid::parse_str(value)
            .map(Self)
            .map_err(|_| DomainError::InvalidId(value.to_owned()))
    }
}

impl fmt::Display for UserId {
    /// Formata o UUID do usuário como texto e retorna o resultado da escrita no formatter.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl FromStr for UserId {
    type Err = DomainError;
    /// Valida e converte o texto em identificador UUID; retorna o ID ou `DomainError` se a entrada for inválida.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifica o contrato identificado por `generates_distinct_v4_ids` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn generates_distinct_v4_ids() {
        assert_ne!(UserId::new_v4(), UserId::new_v4());
    }

    /// Verifica o contrato identificado por `round_trips_through_string` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn round_trips_through_string() {
        let id = UserId::new_v4();

        assert_eq!(UserId::parse(&id.to_string()).expect("id válido"), id);
    }

    /// Verifica o contrato identificado por `rejects_invalid_id` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_invalid_id() {
        assert!(matches!(
            UserId::parse("nao-e-uuid"),
            Err(DomainError::InvalidId(_))
        ));
    }
}
