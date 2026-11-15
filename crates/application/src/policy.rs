//! Política de autorização centralizada.
//!
//! Toda decisão de "quem pode fazer o quê" vive aqui, e não nos handlers. Isso
//! evita a falha clássica de cada endpoint reimplementar a regra de forma
//! ligeiramente diferente — e é justamente onde IDOR e escalonamento de
//! privilégio aparecem.
//!
//! O ator é montado a partir do **estado atual no banco**, nunca apenas das
//! claims do token: um usuário rebaixado ou desativado não mantém os poderes
//! que tinha quando o token foi emitido.

use domain::{Role, UserId};

use crate::error::AppError;

/// Quem está executando a operação.
///
/// Construído pelo handler após reler o usuário no banco, então reflete o papel
/// e o estado correntes.
#[derive(Debug, Clone, Copy)]
pub struct Actor {
    /// Identificador do usuário autenticado.
    pub id: UserId,
    /// Papel atual, vindo do registro persistido.
    pub role: Role,
}

impl Actor {
    /// Cria o ator a partir do identificador e do papel vigente.
    #[must_use]
    pub fn new(id: UserId, role: Role) -> Self {
        Self { id, role }
    }

    /// Indica se o ator é administrador.
    #[must_use]
    pub fn is_admin(self) -> bool {
        self.role.can_manage_users()
    }

    /// Indica se o ator é o próprio usuário alvo.
    #[must_use]
    pub fn is_self(self, target: UserId) -> bool {
        self.id == target
    }
}

/// Operação sobre o recurso de usuários que se deseja autorizar.
#[derive(Debug, Clone, Copy)]
pub enum UserAction {
    /// Listar todos os usuários.
    List,
    /// Criar um usuário novo.
    Create,
    /// Ler o perfil de um usuário.
    View {
        /// Usuário alvo da leitura.
        target: UserId,
    },
    /// Alterar dados de perfil (nome, e-mail, senha).
    UpdateProfile {
        /// Usuário alvo da alteração.
        target: UserId,
    },
    /// Alterar o papel de acesso.
    ChangeRole {
        /// Usuário cujo papel será alterado.
        target: UserId,
    },
    /// Excluir (soft delete) um usuário.
    Delete {
        /// Usuário alvo da exclusão.
        target: UserId,
    },
}

impl UserAction {
    /// Verifica a permissão do ator para esta operação.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Forbidden`] quando o ator não tem o privilégio
    /// exigido e [`AppError::SelfModificationForbidden`] quando um Admin tenta
    /// alterar o próprio papel ou excluir a própria conta.
    pub fn authorize(self, actor: Actor) -> Result<(), AppError> {
        match self {
            // Listar e criar exigem administração: um usuário comum não pode
            // descobrir quem existe nem criar contas.
            Self::List | Self::Create => require_admin(actor),

            // Leitura e atualização de perfil valem para o próprio dono ou para
            // um Admin. O alvo vem da URL, então esta é a barreira anti-IDOR.
            Self::View { target } | Self::UpdateProfile { target } => {
                if actor.is_admin() || actor.is_self(target) {
                    Ok(())
                } else {
                    Err(AppError::Forbidden)
                }
            }

            // Só Admin muda papel — é aqui que o escalonamento seria possível —
            // e nunca o próprio. Sem essa segunda regra, o último Admin poderia
            // se rebaixar e o sistema ficaria sem ninguém capaz de administrar.
            Self::ChangeRole { target } => {
                require_admin(actor)?;
                if actor.is_self(target) {
                    return Err(AppError::SelfModificationForbidden);
                }
                Ok(())
            }

            // Excluir exige Admin e nunca a própria conta, pelo mesmo motivo.
            Self::Delete { target } => {
                require_admin(actor)?;
                if actor.is_self(target) {
                    return Err(AppError::SelfModificationForbidden);
                }
                Ok(())
            }
        }
    }

    /// Texto estável da ação, usado em testes e logs de recusa.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::List => "list_users",
            Self::Create => "create_user",
            Self::View { .. } => "view_user",
            Self::UpdateProfile { .. } => "update_profile",
            Self::ChangeRole { .. } => "change_role",
            Self::Delete { .. } => "delete_user",
        }
    }
}

/// Exige que o ator seja administrador.
///
/// # Errors
///
/// Retorna [`AppError::Forbidden`] para qualquer outro papel.
fn require_admin(actor: Actor) -> Result<(), AppError> {
    if actor.is_admin() {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Identificador fixo do ator nos testes.
    fn actor_id() -> UserId {
        UserId::parse("11111111-1111-4111-8111-111111111111").expect("uuid válido")
    }

    /// Identificador fixo de outro usuário nos testes.
    fn other_id() -> UserId {
        UserId::parse("22222222-2222-4222-8222-222222222222").expect("uuid válido")
    }

    /// Ator com o papel informado.
    fn actor(role: Role) -> Actor {
        Actor::new(actor_id(), role)
    }

    /// Verifica o contrato identificado por `only_admin_can_list_and_create` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn only_admin_can_list_and_create() {
        assert!(UserAction::List.authorize(actor(Role::Admin)).is_ok());
        assert!(UserAction::Create.authorize(actor(Role::Admin)).is_ok());

        for role in [Role::Manager, Role::User] {
            assert!(matches!(
                UserAction::List.authorize(actor(role)),
                Err(AppError::Forbidden)
            ));
            assert!(matches!(
                UserAction::Create.authorize(actor(role)),
                Err(AppError::Forbidden)
            ));
        }
    }

    /// Verifica o contrato identificado por `user_can_read_and_update_own_profile` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn user_can_read_and_update_own_profile() {
        assert!(
            UserAction::View { target: actor_id() }
                .authorize(actor(Role::User))
                .is_ok()
        );
        assert!(
            UserAction::UpdateProfile { target: actor_id() }
                .authorize(actor(Role::User))
                .is_ok()
        );
    }

    /// Verifica o contrato identificado por `user_cannot_read_or_update_someone_else` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn user_cannot_read_or_update_someone_else() {
        assert!(matches!(
            UserAction::View { target: other_id() }.authorize(actor(Role::User)),
            Err(AppError::Forbidden)
        ));
        assert!(matches!(
            UserAction::UpdateProfile { target: other_id() }.authorize(actor(Role::User)),
            Err(AppError::Forbidden)
        ));
    }

    /// Verifica o contrato identificado por `manager_has_no_user_management_privileges` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn manager_has_no_user_management_privileges() {
        assert!(matches!(
            UserAction::View { target: other_id() }.authorize(actor(Role::Manager)),
            Err(AppError::Forbidden)
        ));
        assert!(matches!(
            UserAction::ChangeRole { target: other_id() }.authorize(actor(Role::Manager)),
            Err(AppError::Forbidden)
        ));
        assert!(matches!(
            UserAction::Delete { target: other_id() }.authorize(actor(Role::Manager)),
            Err(AppError::Forbidden)
        ));
    }

    /// Verifica o contrato identificado por `admin_can_manage_other_users` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn admin_can_manage_other_users() {
        for action in [
            UserAction::View { target: other_id() },
            UserAction::UpdateProfile { target: other_id() },
            UserAction::ChangeRole { target: other_id() },
            UserAction::Delete { target: other_id() },
        ] {
            assert!(
                action.authorize(actor(Role::Admin)).is_ok(),
                "admin deveria poder executar {}",
                action.name()
            );
        }
    }

    /// Verifica o contrato identificado por `admin_cannot_change_own_role` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn admin_cannot_change_own_role() {
        assert!(matches!(
            UserAction::ChangeRole { target: actor_id() }.authorize(actor(Role::Admin)),
            Err(AppError::SelfModificationForbidden)
        ));
    }

    /// Verifica o contrato identificado por `admin_cannot_delete_own_account` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn admin_cannot_delete_own_account() {
        assert!(matches!(
            UserAction::Delete { target: actor_id() }.authorize(actor(Role::Admin)),
            Err(AppError::SelfModificationForbidden)
        ));
    }

    /// Verifica o contrato identificado por `admin_can_update_own_profile` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn admin_can_update_own_profile() {
        assert!(
            UserAction::UpdateProfile { target: actor_id() }
                .authorize(actor(Role::Admin))
                .is_ok()
        );
    }
}
