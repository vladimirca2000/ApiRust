//! Entidade `Category`.
//!
//! Categoria é o agrupamento de produtos. A regra central está em
//! [`DELETE_WITH_PRODUCTS_POLICY`]: remover uma categoria que ainda tem produtos
//! é **rejeitado** (conflito), nunca em cascata — apagar produtos por tabela
//! seria destruir dados comerciais como efeito colateral.

use chrono::{DateTime, Utc};

use crate::error::DomainError;
use crate::status::EntityStatus;
use crate::user_id::UserId;

/// Comprimento máximo do nome de categoria, em caracteres.
pub const MAX_CATEGORY_NAME_LEN: usize = 80;
/// Comprimento máximo da descrição de categoria, em caracteres.
pub const MAX_DESCRIPTION_LEN: usize = 500;

/// Política documentada para exclusão de categoria com produtos associados.
///
/// A escolha é rejeitar com conflito (`409`), e a FK `ON DELETE RESTRICT` no
/// schema garante isso mesmo se a aplicação falhar em checar antes.
pub const DELETE_WITH_PRODUCTS_POLICY: &str =
    "categoria com produtos ativos não pode ser removida; remova ou mova os produtos antes";

/// Categoria de produtos.
#[derive(Debug, Clone)]
pub struct Category {
    /// Identificador da categoria.
    id: UserId,
    /// Nome único entre categorias ativas.
    name: String,
    /// Descrição opcional.
    description: Option<String>,
    /// Estado do registro.
    status: EntityStatus,
    /// Momento de criação.
    created_at: DateTime<Utc>,
    /// Momento da última mutação.
    updated_at: DateTime<Utc>,
    /// Momento do soft delete, quando houver.
    deleted_at: Option<DateTime<Utc>>,
}

impl Category {
    /// Cria uma categoria nova e ativa.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::InvalidName`] para nome inválido e
    /// [`DomainError::InvalidDescription`] para descrição longa demais.
    pub fn new(
        name: impl Into<String>,
        description: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        Ok(Self {
            id: UserId::new_v4(),
            name: validate_name(name)?,
            description: validate_description(description)?,
            status: EntityStatus::Active,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        })
    }

    /// Reconstitui uma categoria já persistida.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError`] quando nome, descrição ou datas são inválidos.
    pub fn restore(
        id: UserId,
        name: impl Into<String>,
        description: Option<String>,
        status: EntityStatus,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
        deleted_at: Option<DateTime<Utc>>,
    ) -> Result<Self, DomainError> {
        if updated_at < created_at {
            return Err(DomainError::InvalidTimestamp(
                "updated_at não pode ser anterior a created_at".to_owned(),
            ));
        }
        Ok(Self {
            id,
            name: validate_name(name)?,
            description: validate_description(description)?,
            status,
            created_at,
            updated_at,
            deleted_at,
        })
    }

    /// Devolve o identificador.
    #[must_use]
    pub fn id(&self) -> UserId {
        self.id
    }

    /// Devolve o nome validado.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Devolve a descrição, quando houver.
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Devolve o estado persistido.
    #[must_use]
    pub fn status(&self) -> EntityStatus {
        self.status
    }

    /// Devolve o momento de criação.
    #[must_use]
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Devolve o momento da última mutação.
    #[must_use]
    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Devolve o momento do soft delete, quando houver.
    #[must_use]
    pub fn deleted_at(&self) -> Option<DateTime<Utc>> {
        self.deleted_at
    }

    /// Indica se a categoria está viva e ativa.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.status.is_active() && self.deleted_at.is_none()
    }

    /// Renomeia a categoria.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::DeletedUser`] se já excluída e
    /// [`DomainError::InvalidName`] para nome inválido.
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

    /// Altera a descrição.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::DeletedUser`] se já excluída e
    /// [`DomainError::InvalidDescription`] para texto longo demais.
    pub fn change_description(
        &mut self,
        description: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<(), DomainError> {
        self.ensure_not_deleted()?;
        self.description = validate_description(description)?;
        self.touch(now);
        Ok(())
    }

    /// Aplica soft delete de forma idempotente.
    pub fn soft_delete(&mut self, now: DateTime<Utc>) {
        if self.deleted_at.is_some() {
            return;
        }
        let moment = now.max(self.updated_at);
        self.status = EntityStatus::Inactive;
        self.deleted_at = Some(moment);
        self.updated_at = moment;
    }

    /// Atualiza `updated_at` sem permitir retrocesso.
    fn touch(&mut self, now: DateTime<Utc>) {
        self.updated_at = now.max(self.updated_at);
    }

    /// Garante que a categoria não recebeu soft delete.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::DeletedUser`] quando `deleted_at` está preenchido.
    fn ensure_not_deleted(&self) -> Result<(), DomainError> {
        if self.deleted_at.is_some() {
            Err(DomainError::DeletedUser)
        } else {
            Ok(())
        }
    }
}

/// Normaliza e valida o nome da categoria.
///
/// # Errors
///
/// Retorna [`DomainError::InvalidName`] para vazio, longo demais ou com
/// caracteres de controle.
fn validate_name(name: impl Into<String>) -> Result<String, DomainError> {
    let name = name.into();
    let trimmed = name.trim();

    if trimmed.is_empty() {
        return Err(DomainError::InvalidName("não pode ser vazio".to_owned()));
    }
    if trimmed.chars().count() > MAX_CATEGORY_NAME_LEN {
        return Err(DomainError::InvalidName(format!(
            "deve ter no máximo {MAX_CATEGORY_NAME_LEN} caracteres"
        )));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(DomainError::InvalidName(
            "não pode conter caracteres de controle".to_owned(),
        ));
    }

    Ok(trimmed.to_owned())
}

/// Normaliza e valida a descrição opcional.
///
/// Usado por categoria e produto, que compartilham a regra de tamanho.
///
/// # Errors
///
/// Retorna [`DomainError::InvalidDescription`] para texto longo demais ou com
/// caracteres de controle; vazio vira `None`.
pub fn validate_description(description: Option<String>) -> Result<Option<String>, DomainError> {
    let Some(description) = description else {
        return Ok(None);
    };
    let trimmed = description.trim();

    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > MAX_DESCRIPTION_LEN {
        return Err(DomainError::InvalidDescription(format!(
            "deve ter no máximo {MAX_DESCRIPTION_LEN} caracteres"
        )));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(DomainError::InvalidDescription(
            "não pode conter caracteres de controle".to_owned(),
        ));
    }

    Ok(Some(trimmed.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    /// Instante fixo dos testes.
    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-14T12:00:00Z")
            .expect("data válida")
            .with_timezone(&Utc)
    }

    #[test]
    fn new_category_is_active_and_trimmed() {
        let category = Category::new("  Eletrônicos  ", None, now()).expect("válida");

        assert_eq!(category.name(), "Eletrônicos");
        assert!(category.is_active());
        assert_eq!(category.created_at(), now());
    }

    #[test]
    fn blank_description_becomes_none() {
        let category = Category::new("X", Some("   ".to_owned()), now()).expect("válida");

        assert_eq!(category.description(), None);
    }

    #[test]
    fn rejects_empty_name() {
        assert!(matches!(
            Category::new("   ", None, now()),
            Err(DomainError::InvalidName(_))
        ));
    }

    #[test]
    fn rejects_name_over_limit() {
        let long = "a".repeat(MAX_CATEGORY_NAME_LEN + 1);
        assert!(Category::new(long, None, now()).is_err());
    }

    #[test]
    fn rejects_description_over_limit() {
        let long = "a".repeat(MAX_DESCRIPTION_LEN + 1);
        assert!(Category::new("X", Some(long), now()).is_err());
    }

    #[test]
    fn rename_updates_timestamp() {
        let mut category = Category::new("Antiga", None, now()).expect("válida");
        let later = now() + Duration::seconds(30);

        category.rename("Nova", later).expect("renomear");

        assert_eq!(category.name(), "Nova");
        assert_eq!(category.updated_at(), later);
    }

    #[test]
    fn timestamp_never_moves_backwards() {
        let mut category = Category::new("X", None, now()).expect("válida");

        category
            .rename("Y", now() - Duration::seconds(30))
            .expect("renomear");

        assert_eq!(category.updated_at(), now());
    }

    #[test]
    fn soft_delete_is_idempotent() {
        let mut category = Category::new("X", None, now()).expect("válida");
        let later = now() + Duration::seconds(60);

        category.soft_delete(later);
        category.soft_delete(later + Duration::seconds(30));

        assert!(!category.is_active());
        assert_eq!(category.deleted_at(), Some(later));
    }

    #[test]
    fn mutations_rejected_after_delete() {
        let mut category = Category::new("X", None, now()).expect("válida");
        category.soft_delete(now());

        assert!(category.rename("Y", now()).is_err());
        assert!(category.change_description(None, now()).is_err());
    }
}
