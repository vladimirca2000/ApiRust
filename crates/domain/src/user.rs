use chrono::{DateTime, Utc};

use crate::email::Email;
use crate::error::DomainError;
use crate::password::PasswordHash;
use crate::role::Role;
use crate::status::EntityStatus;
use crate::user_id::UserId;

/// Comprimento máximo do nome do usuário, em caracteres.
pub const MAX_NAME_LEN: usize = 120;

/// Datas de criação, atualização e exclusão, sempre em UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserTimestamps {
    /// Momento em que o registro foi criado; nunca muda depois.
    created_at: DateTime<Utc>,
    /// Momento da última mutação; nunca retrocede.
    updated_at: DateTime<Utc>,
    /// Momento do soft delete, ou `None` enquanto o registro está vivo.
    deleted_at: Option<DateTime<Utc>>,
}

impl UserTimestamps {
    /// Reconstitui timestamps vindos da persistência.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::InvalidTimestamp`] quando `updated_at` ou
    /// `deleted_at` são anteriores a `created_at`.
    pub fn new(
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
        deleted_at: Option<DateTime<Utc>>,
    ) -> Result<Self, DomainError> {
        if updated_at < created_at {
            return Err(DomainError::InvalidTimestamp(
                "updated_at não pode ser anterior a created_at".to_owned(),
            ));
        }

        if deleted_at.is_some_and(|deleted_at| deleted_at < created_at) {
            return Err(DomainError::InvalidTimestamp(
                "deleted_at não pode ser anterior a created_at".to_owned(),
            ));
        }

        Ok(Self {
            created_at,
            updated_at,
            deleted_at,
        })
    }

    /// Cria os timestamps de um registro recém-criado.
    ///
    /// `created_at` e `updated_at` recebem o mesmo instante e `deleted_at` fica
    /// vazio, representando um registro ativo.
    #[must_use]
    pub const fn at(now: DateTime<Utc>) -> Self {
        Self {
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    /// Devolve o momento de criação em UTC.
    #[must_use]
    pub fn created_at(self) -> DateTime<Utc> {
        self.created_at
    }

    /// Devolve o momento da última mutação em UTC.
    #[must_use]
    pub fn updated_at(self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Devolve o momento do soft delete, ou `None` se o registro está vivo.
    #[must_use]
    pub fn deleted_at(self) -> Option<DateTime<Utc>> {
        self.deleted_at
    }
}

/// Usuário do sistema.
///
/// Todas as instâncias passaram por validação de nome, e-mail, hash de senha e
/// timestamps. As mutações preservam essas invariantes e atualizam
/// `updated_at` sem permitir retrocesso.
#[derive(Debug, Clone)]
pub struct User {
    /// Identificador estável da entidade, gerado na criação.
    id: UserId,
    /// Nome de exibição, já sem espaços nas pontas.
    name: String,
    /// E-mail normalizado e único entre registros ativos.
    email: Email,
    /// Hash Argon2id da senha; nunca a senha em texto puro.
    password_hash: PasswordHash,
    /// Papel de acesso, sempre atribuído pelo servidor.
    role: Role,
    /// Estado do registro (ativo/inativo).
    status: EntityStatus,
    /// Datas de criação, atualização e soft delete.
    timestamps: UserTimestamps,
}

impl User {
    /// Cria um usuário novo, ativo e com identificador gerado.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::InvalidName`] quando o nome é vazio, longo demais
    /// ou contém caracteres de controle.
    pub fn new(
        name: impl Into<String>,
        email: Email,
        password_hash: PasswordHash,
        role: Role,
        now: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        Ok(Self {
            id: UserId::new_v4(),
            name: validate_name(name)?,
            email,
            password_hash,
            role,
            status: EntityStatus::Active,
            timestamps: UserTimestamps::at(now),
        })
    }

    /// Reconstitui um usuário já persistido, preservando identificador,
    /// timestamps e estado.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::InvalidName`] para nomes inválidos. Os demais
    /// valores já chegam validados pelos seus tipos.
    pub fn restore(
        id: UserId,
        name: impl Into<String>,
        email: Email,
        password_hash: PasswordHash,
        role: Role,
        status: EntityStatus,
        timestamps: UserTimestamps,
    ) -> Result<Self, DomainError> {
        Ok(Self {
            id,
            name: validate_name(name)?,
            email,
            password_hash,
            role,
            status,
            timestamps,
        })
    }

    /// Devolve o identificador do usuário.
    #[must_use]
    pub fn id(&self) -> UserId {
        self.id
    }

    /// Devolve o nome de exibição já validado.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Devolve o e-mail normalizado.
    #[must_use]
    pub fn email(&self) -> &Email {
        &self.email
    }

    /// Devolve o hash da senha para persistência e verificação.
    ///
    /// Nunca exponha este valor em DTOs de resposta ou logs.
    #[must_use]
    pub fn password_hash(&self) -> &PasswordHash {
        &self.password_hash
    }

    /// Devolve o papel de acesso atual.
    #[must_use]
    pub fn role(&self) -> Role {
        self.role
    }

    /// Devolve o estado ativo/inativo do registro.
    #[must_use]
    pub fn status(&self) -> EntityStatus {
        self.status
    }

    /// Devolve o momento de criação em UTC.
    #[must_use]
    pub fn created_at(&self) -> DateTime<Utc> {
        self.timestamps.created_at
    }

    /// Devolve o momento da última mutação em UTC.
    #[must_use]
    pub fn updated_at(&self) -> DateTime<Utc> {
        self.timestamps.updated_at
    }

    /// Devolve o momento do soft delete, ou `None` se o registro está vivo.
    #[must_use]
    pub fn deleted_at(&self) -> Option<DateTime<Utc>> {
        self.timestamps.deleted_at
    }

    /// Indica se o usuário pode autenticar e operar no sistema.
    ///
    /// Exige estado `active` **e** ausência de soft delete: um registro
    /// desativado continua no banco, mas não conta como ativo.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.status.is_active() && self.timestamps.deleted_at.is_none()
    }

    /// Indica se o usuário recebeu soft delete.
    #[must_use]
    pub fn is_deleted(&self) -> bool {
        self.timestamps.deleted_at.is_some()
    }

    /// # Errors
    ///
    /// Retorna [`DomainError::InvalidName`] para nomes inválidos e
    /// [`DomainError::DeletedUser`] se o usuário já foi excluído.
    pub fn rename(
        &mut self,
        name: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Result<(), DomainError> {
        self.ensure_not_deleted()?;
        self.name = validate_name(name)?;
        self.touch(now);
        Ok(())
    }

    /// Troca o e-mail do usuário.
    ///
    /// O e-mail só pode entrar no agregado por um [`Email`] já validado, então
    /// qualquer troca passa pela normalização e validação de formato.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::DeletedUser`] se o usuário já foi excluído.
    pub fn change_email(&mut self, email: Email, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.ensure_not_deleted()?;
        self.email = email;
        self.touch(now);
        Ok(())
    }

    /// Troca o papel de acesso do usuário.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::DeletedUser`] se o usuário já foi excluído.
    pub fn change_role(&mut self, role: Role, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.ensure_not_deleted()?;
        self.role = role;
        self.touch(now);
        Ok(())
    }

    /// Substitui o hash da senha por um novo já calculado.
    ///
    /// A senha em texto puro não passa por aqui: quem chama deve hashear antes.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::DeletedUser`] se o usuário já foi excluído.
    pub fn change_password(
        &mut self,
        password_hash: PasswordHash,
        now: DateTime<Utc>,
    ) -> Result<(), DomainError> {
        self.ensure_not_deleted()?;
        self.password_hash = password_hash;
        self.touch(now);
        Ok(())
    }

    /// Marca o usuário como ativo, permitindo autenticação.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::DeletedUser`] se o usuário já foi excluído.
    pub fn activate(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.ensure_not_deleted()?;
        self.status = EntityStatus::Active;
        self.touch(now);
        Ok(())
    }

    /// Marca o usuário como inativo, bloqueando autenticação sem apagar o registro.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::DeletedUser`] se o usuário já foi excluído.
    pub fn deactivate(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.ensure_not_deleted()?;
        self.status = EntityStatus::Inactive;
        self.touch(now);
        Ok(())
    }

    /// Aplica soft delete de forma idempotente e desativa o usuário.
    pub fn soft_delete(&mut self, now: DateTime<Utc>) {
        if self.timestamps.deleted_at.is_some() {
            return;
        }

        let moment = now.max(self.timestamps.updated_at);
        self.status = EntityStatus::Inactive;
        self.timestamps.deleted_at = Some(moment);
        self.timestamps.updated_at = moment;
    }

    /// Atualiza `updated_at` para o instante informado, sem permitir retrocesso.
    ///
    /// Se o relógio do chamador estiver atrasado em relação ao último registro,
    /// mantém o valor mais recente para não invalidar auditoria.
    fn touch(&mut self, now: DateTime<Utc>) {
        self.timestamps.updated_at = now.max(self.timestamps.updated_at);
    }

    /// Garante que a entidade não recebeu soft delete antes de uma mutação.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::DeletedUser`] quando `deleted_at` está preenchido.
    fn ensure_not_deleted(&self) -> Result<(), DomainError> {
        if self.is_deleted() {
            Err(DomainError::DeletedUser)
        } else {
            Ok(())
        }
    }
}

/// Normaliza e valida o nome de exibição.
///
/// Remove espaços das pontas, rejeita vazio, excede [`MAX_NAME_LEN`] ou contém
/// caracteres de controle.
///
/// # Errors
///
/// Retorna [`DomainError::InvalidName`] descrevendo a violação.
fn validate_name(name: impl Into<String>) -> Result<String, DomainError> {
    let name = name.into();
    let trimmed = name.trim();

    if trimmed.is_empty() {
        return Err(DomainError::InvalidName("não pode ser vazio".to_owned()));
    }

    if trimmed.chars().count() > MAX_NAME_LEN {
        return Err(DomainError::InvalidName(format!(
            "deve ter no máximo {MAX_NAME_LEN} caracteres"
        )));
    }

    if trimmed.chars().any(char::is_control) {
        return Err(DomainError::InvalidName(
            "não pode conter caracteres de controle".to_owned(),
        ));
    }

    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-14T12:00:00Z")
            .expect("data válida")
            .with_timezone(&Utc)
    }

    fn email() -> Email {
        Email::parse("user@example.com").expect("e-mail válido")
    }

    fn hash() -> PasswordHash {
        PasswordHash::parse("$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA").expect("hash válido")
    }

    fn user() -> User {
        User::new("Vladimir", email(), hash(), Role::Admin, now()).expect("usuário válido")
    }

    #[test]
    fn new_user_is_active_and_trims_name() {
        let user = User::new("  Vladimir  ", email(), hash(), Role::Manager, now())
            .expect("usuário válido");

        assert_eq!(user.name(), "Vladimir");
        assert_eq!(user.role(), Role::Manager);
        assert_eq!(user.status(), EntityStatus::Active);
        assert!(user.is_active());
        assert!(!user.is_deleted());
        assert_eq!(user.created_at(), now());
        assert_eq!(user.updated_at(), now());
        assert!(user.deleted_at().is_none());
    }

    #[test]
    fn rejects_invalid_name() {
        assert!(matches!(
            User::new("   ", email(), hash(), Role::User, now()),
            Err(DomainError::InvalidName(_))
        ));
    }

    #[test]
    fn rejects_name_over_limit() {
        let long_name = "a".repeat(MAX_NAME_LEN + 1);

        assert!(User::new(long_name, email(), hash(), Role::User, now()).is_err());
    }

    #[test]
    fn rename_updates_timestamp() {
        let mut user = user();
        let later = now() + Duration::seconds(30);

        user.rename("  Novo Nome  ", later).expect("renomear");

        assert_eq!(user.name(), "Novo Nome");
        assert_eq!(user.updated_at(), later);
        assert!(user.rename("Inválido\u{7}", later).is_err());
    }

    #[test]
    fn timestamp_never_moves_backwards() {
        let mut user = user();
        let earlier = now() - Duration::seconds(30);

        user.rename("Nome", earlier).expect("renomear");

        assert_eq!(user.updated_at(), now());
    }

    #[test]
    fn changes_role_password_and_status() {
        let mut user = user();
        let later = now() + Duration::seconds(60);
        let new_hash = PasswordHash::parse("$argon2id$v=19$m=19456,t=2,p=1$b3V0cm8$aGFzaC1ub3Zv")
            .expect("hash válido");

        user.change_role(Role::User, later).expect("trocar papel");
        user.change_password(new_hash.clone(), later)
            .expect("trocar senha");
        user.deactivate(later).expect("desativar");

        assert_eq!(user.role(), Role::User);
        assert_eq!(user.password_hash(), &new_hash);
        assert_eq!(user.status(), EntityStatus::Inactive);
        assert!(!user.is_active());
    }

    #[test]
    fn soft_delete_deactivates_and_is_idempotent() {
        let mut user = user();
        let later = now() + Duration::seconds(90);

        user.soft_delete(later);
        user.soft_delete(later + Duration::seconds(30));

        assert!(user.is_deleted());
        assert!(!user.is_active());
        assert_eq!(user.status(), EntityStatus::Inactive);
        assert_eq!(user.deleted_at(), Some(later));
        assert_eq!(user.updated_at(), later);
    }

    #[test]
    fn mutations_are_rejected_after_delete() {
        let mut user = user();
        user.soft_delete(now());

        assert_eq!(user.rename("Outro", now()), Err(DomainError::DeletedUser));
        assert_eq!(
            user.change_role(Role::User, now()),
            Err(DomainError::DeletedUser)
        );
        assert_eq!(user.activate(now()), Err(DomainError::DeletedUser));
    }

    #[test]
    fn restore_preserves_state() {
        let original = user();
        let timestamps = UserTimestamps::new(original.created_at(), original.updated_at(), None)
            .expect("timestamps válidos");

        let restored = User::restore(
            original.id(),
            original.name(),
            original.email().clone(),
            original.password_hash().clone(),
            original.role(),
            original.status(),
            timestamps,
        )
        .expect("restaurar usuário");

        assert_eq!(restored.id(), original.id());
        assert_eq!(restored.name(), original.name());
        assert_eq!(restored.email(), original.email());
        assert_eq!(restored.password_hash(), original.password_hash());
    }

    #[test]
    fn timestamps_reject_inconsistent_order() {
        let later = now() + Duration::seconds(30);

        assert!(UserTimestamps::new(later, now(), None).is_err());
        assert!(UserTimestamps::new(now(), later, Some(now() - Duration::seconds(30))).is_err());
        assert!(UserTimestamps::new(now(), later, Some(later)).is_ok());
    }

    /// O critério de aceite da Fase 2 exige que nenhum hash chegue a logs.
    /// `tracing` formata o valor com `Debug`, então este é o caminho real.
    #[test]
    fn debug_does_not_expose_the_password_hash() {
        let user = user();

        let debug = format!("{user:?}");

        assert!(
            !debug.contains("argon2id"),
            "Debug de User não pode conter o hash: {debug}"
        );
        assert!(
            !debug.contains(user.password_hash().as_str()),
            "Debug de User não pode conter o valor do hash"
        );
        // Os demais campos continuam visíveis para depuração.
        assert!(debug.contains("Vladimir"));
        assert!(debug.contains("user@example.com"));
        assert!(debug.contains("Admin"));
    }
}
