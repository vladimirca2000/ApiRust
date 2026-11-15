//! Estado ativo/inativo compartilhado pelas entidades do domínio.
//!
//! Faz a conversão textual e a consulta de atividade de modo consistente.

use std::fmt;
use std::str::FromStr;

use crate::error::DomainError;

/// Estado de uma entidade persistida.
///
/// Compartilhado por usuários, categorias e produtos para manter o mesmo
/// vocabulário no domínio e no schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityStatus {
    /// Registro visível e utilizável pelo sistema.
    Active,
    /// Registro mantido no banco, porém fora de uso (desativado ou excluído).
    Inactive,
}

impl EntityStatus {
    /// Todos os estados conhecidos.
    pub const ALL: [Self; 2] = [Self::Active, Self::Inactive];

    /// Representação persistida do estado (`active`, `inactive`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
        }
    }

    /// Converte o texto persistido no estado correspondente.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::InvalidStatus`] para qualquer valor desconhecido.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "active" => Ok(Self::Active),
            "inactive" => Ok(Self::Inactive),
            other => Err(DomainError::InvalidStatus(other.to_owned())),
        }
    }

    /// Indica se o registro está em uso (`true`) ou arquivado (`false`).
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }
}

impl fmt::Display for EntityStatus {
    /// Formata o estado da entidade como texto estável e retorna o resultado da escrita no formatter.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for EntityStatus {
    type Err = DomainError;
    /// Converte o texto em estado de entidade; retorna o estado conhecido ou `DomainError` para valores desconhecidos.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifica o contrato identificado por `round_trips_through_string` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn round_trips_through_string() {
        for status in EntityStatus::ALL {
            assert_eq!(
                EntityStatus::parse(status.as_str()).expect("estado válido"),
                status
            );
        }
    }

    /// Verifica o contrato identificado por `rejects_unknown_status` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_unknown_status() {
        assert_eq!(
            EntityStatus::parse("archived"),
            Err(DomainError::InvalidStatus("archived".to_owned()))
        );
    }

    /// Verifica o contrato identificado por `reports_activity` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn reports_activity() {
        assert!(EntityStatus::Active.is_active());
        assert!(!EntityStatus::Inactive.is_active());
    }
}
