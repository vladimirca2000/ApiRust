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
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl FromStr for UserId {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_distinct_v4_ids() {
        assert_ne!(UserId::new_v4(), UserId::new_v4());
    }

    #[test]
    fn round_trips_through_string() {
        let id = UserId::new_v4();

        assert_eq!(UserId::parse(&id.to_string()).expect("id válido"), id);
    }

    #[test]
    fn rejects_invalid_id() {
        assert!(matches!(
            UserId::parse("nao-e-uuid"),
            Err(DomainError::InvalidId(_))
        ));
    }
}
