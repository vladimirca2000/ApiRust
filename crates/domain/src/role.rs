use std::fmt;
use std::str::FromStr;

use crate::error::DomainError;

/// Papel de acesso do usuário, sempre atribuído pelo servidor.
///
/// Nunca é lido de entrada do cliente: vem das claims verificadas do token ou
/// do próprio registro persistido.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Acesso total, incluindo gerenciamento de usuários.
    Admin,
    /// Mantém o catálogo (categorias e produtos), sem gerenciar usuários.
    Manager,
    /// Consulta o catálogo e administra apenas o próprio perfil.
    User,
}

impl Role {
    /// Todos os papéis conhecidos, em ordem de privilégio decrescente.
    pub const ALL: [Self; 3] = [Self::Admin, Self::Manager, Self::User];

    /// Representação persistida do papel (`admin`, `manager`, `user`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Manager => "manager",
            Self::User => "user",
        }
    }

    /// # Errors
    ///
    /// Retorna [`DomainError::InvalidRole`] para qualquer valor desconhecido.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "admin" => Ok(Self::Admin),
            "manager" => Ok(Self::Manager),
            "user" => Ok(Self::User),
            other => Err(DomainError::InvalidRole(other.to_owned())),
        }
    }

    /// Indica se o papel pode gerenciar usuários.
    #[must_use]
    pub const fn can_manage_users(self) -> bool {
        matches!(self, Self::Admin)
    }

    /// Indica se o papel pode criar e manter o catálogo.
    #[must_use]
    pub const fn can_manage_catalog(self) -> bool {
        matches!(self, Self::Admin | Self::Manager)
    }
}

impl fmt::Display for Role {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Role {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_string() {
        for role in Role::ALL {
            assert_eq!(Role::parse(role.as_str()).expect("papel válido"), role);
        }
    }

    #[test]
    fn parses_case_insensitively() {
        assert_eq!(Role::parse(" ADMIN ").expect("papel válido"), Role::Admin);
    }

    #[test]
    fn rejects_unknown_role() {
        assert_eq!(
            Role::parse("owner"),
            Err(DomainError::InvalidRole("owner".to_owned()))
        );
    }

    #[test]
    fn applies_least_privilege() {
        assert!(Role::Admin.can_manage_users());
        assert!(Role::Admin.can_manage_catalog());

        assert!(!Role::Manager.can_manage_users());
        assert!(Role::Manager.can_manage_catalog());

        assert!(!Role::User.can_manage_users());
        assert!(!Role::User.can_manage_catalog());
    }
}
