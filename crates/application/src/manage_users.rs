//! Casos de uso de gerenciamento de usuários.
//!
//! Cada método recebe o [`Actor`] já resolvido (papel e estado lidos do banco
//! pelo handler) e passa pela política central antes de tocar em dados. A
//! política é a única fonte de decisão: nenhum método reimplementa regra de
//! acesso.

use chrono::{DateTime, Utc};
use domain::{Email, Role, User, UserId, validate_password};

use crate::error::AppError;
use crate::policy::{Actor, UserAction};
use crate::ports::{PasswordHasher, UserRepository};

/// Resultado de uma listagem paginada.
#[derive(Debug, Clone)]
pub struct UserPage {
    /// Usuários da página atual.
    pub items: Vec<User>,
    /// Total de usuários vivos no sistema.
    pub total: i64,
    /// Offset aplicado.
    pub offset: i64,
    /// Limite aplicado.
    pub limit: i64,
}

/// Dados para criar um usuário.
#[derive(Debug, Clone)]
pub struct NewUserInput {
    /// Nome de exibição.
    pub name: String,
    /// E-mail (será normalizado).
    pub email: String,
    /// Senha em texto puro; nunca é persistida.
    pub password: String,
    /// Papel inicial, escolhido pelo Admin.
    pub role: Role,
}

/// Dados para atualizar o perfil.
///
/// Campos `None` não são alterados — permite `PATCH` parcial sem exigir que o
/// cliente reenvie tudo.
#[derive(Debug, Clone, Default)]
pub struct UpdateProfileInput {
    /// Novo nome, se informado.
    pub name: Option<String>,
    /// Novo e-mail, se informado.
    pub email: Option<String>,
    /// Nova senha em texto puro, se informada.
    pub password: Option<String>,
}

/// Casos de uso de usuários, com autorização aplicada em cada operação.
pub struct ManageUsers<'a> {
    /// Repositório de usuários.
    users: &'a dyn UserRepository,
    /// Hasher usado ao criar usuário e ao trocar senha.
    hasher: &'a dyn PasswordHasher,
}

impl<'a> ManageUsers<'a> {
    /// Monta o caso de uso com as dependências injetadas.
    #[must_use]
    pub fn new(users: &'a dyn UserRepository, hasher: &'a dyn PasswordHasher) -> Self {
        Self { users, hasher }
    }

    /// Lista usuários vivos com paginação.
    ///
    /// O `limit` é limitado a [`crate::MAX_PAGE_SIZE`] para que um cliente não
    /// consiga pedir a tabela inteira e derrubar a memória do processo.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Forbidden`] se o ator não for Admin e
    /// [`AppError::Infrastructure`] em falha de consulta.
    pub async fn list(&self, actor: Actor, offset: i64, limit: i64) -> Result<UserPage, AppError> {
        UserAction::List.authorize(actor)?;

        let (offset, limit) = crate::pagination::normalize(offset, limit);

        let items = self.users.list(offset, limit).await?;
        let total = self.users.count().await?;

        Ok(UserPage {
            items,
            total,
            offset,
            limit,
        })
    }

    /// Cria um usuário novo com o papel informado pelo Admin.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Forbidden`] se o ator não for Admin,
    /// [`AppError::Domain`] para e-mail/senha inválidos,
    /// [`AppError::EmailAlreadyInUse`] se o e-mail já existir e
    /// [`AppError::Infrastructure`] em falha de hashing ou gravação.
    pub async fn create(
        &self,
        actor: Actor,
        input: NewUserInput,
        now: DateTime<Utc>,
    ) -> Result<User, AppError> {
        UserAction::Create.authorize(actor)?;

        let email = Email::parse(&input.email)?;
        validate_password(&input.password)?;

        let hash = self.hasher.hash(&input.password).await?;
        let user = User::new(input.name, email, hash, input.role, now)?;

        self.users.insert(&user).await?;
        Ok(user)
    }

    /// Lê o perfil de um usuário.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Forbidden`] quando o ator não é Admin nem o próprio
    /// alvo, [`AppError::UserNotFound`] se o usuário não existe e
    /// [`AppError::Infrastructure`] em falha de consulta.
    pub async fn view(&self, actor: Actor, target: UserId) -> Result<User, AppError> {
        UserAction::View { target }.authorize(actor)?;
        self.load(target).await
    }

    /// Atualiza nome, e-mail e/ou senha de um usuário.
    ///
    /// Não altera papel nem estado: esses campos têm operações próprias, com
    /// regras de autorização distintas.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Forbidden`] quando o ator não é Admin nem o próprio
    /// alvo, [`AppError::UserNotFound`] se o usuário não existe,
    /// [`AppError::Domain`] para dados inválidos,
    /// [`AppError::EmailAlreadyInUse`] em colisão de e-mail e
    /// [`AppError::Infrastructure`] em falha de hashing ou gravação.
    pub async fn update_profile(
        &self,
        actor: Actor,
        target: UserId,
        input: UpdateProfileInput,
        now: DateTime<Utc>,
    ) -> Result<User, AppError> {
        UserAction::UpdateProfile { target }.authorize(actor)?;

        let mut user = self.load(target).await?;

        if let Some(name) = input.name {
            user.rename(name, now)?;
        }

        if let Some(email) = input.email {
            user.change_email(Email::parse(&email)?, now)?;
        }

        if let Some(password) = input.password {
            validate_password(&password)?;
            let hash = self.hasher.hash(&password).await?;
            user.change_password(hash, now)?;
        }

        self.users.update(&user).await?;
        Ok(user)
    }

    /// Altera o papel de acesso de um usuário.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Forbidden`] se o ator não for Admin,
    /// [`AppError::SelfModificationForbidden`] se for a própria conta,
    /// [`AppError::UserNotFound`] se o alvo não existe e
    /// [`AppError::Infrastructure`] em falha de gravação.
    pub async fn change_role(
        &self,
        actor: Actor,
        target: UserId,
        role: Role,
        now: DateTime<Utc>,
    ) -> Result<User, AppError> {
        UserAction::ChangeRole { target }.authorize(actor)?;

        let mut user = self.load(target).await?;
        user.change_role(role, now)?;
        self.users.update(&user).await?;
        Ok(user)
    }

    /// Aplica soft delete em um usuário.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Forbidden`] se o ator não for Admin,
    /// [`AppError::SelfModificationForbidden`] para a própria conta,
    /// [`AppError::UserNotFound`] se o alvo não existe e
    /// [`AppError::Infrastructure`] em falha de gravação.
    pub async fn delete(
        &self,
        actor: Actor,
        target: UserId,
        now: DateTime<Utc>,
    ) -> Result<User, AppError> {
        UserAction::Delete { target }.authorize(actor)?;

        let mut user = self.load(target).await?;
        user.soft_delete(now);
        self.users.update(&user).await?;
        Ok(user)
    }

    /// Carrega o usuário vivo ou devolve `UserNotFound`.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::UserNotFound`] quando não há registro vivo com o
    /// identificador e [`AppError::Infrastructure`] em falha de consulta.
    async fn load(&self, target: UserId) -> Result<User, AppError> {
        self.users
            .find_by_id(target)
            .await?
            .ok_or(AppError::UserNotFound)
    }
}
