//! Entidade `Product`.
//!
//! Reúne os dois valores que exigem invariantes próprias — [`Money`] e [`Sku`] —
//! e adiciona estoque não negativo. A categoria é referenciada por id: o produto
//! não carrega a entidade inteira, o que manteria o agregado grande e criaria
//! risco de dados divergentes.

use chrono::{DateTime, Utc};

use crate::category::validate_description;
use crate::error::DomainError;
use crate::money::Money;
use crate::sku::Sku;
use crate::status::EntityStatus;
use crate::user_id::UserId;

/// Comprimento máximo do nome de produto, em caracteres.
pub const MAX_PRODUCT_NAME_LEN: usize = 120;

/// Produto do catálogo.
#[derive(Debug, Clone)]
pub struct Product {
    /// Identificador do produto.
    id: UserId,
    /// Nome comercial.
    name: String,
    /// SKU único entre produtos ativos.
    sku: Sku,
    /// Descrição opcional.
    description: Option<String>,
    /// Preço em centavos, sempre positivo.
    price: Money,
    /// Estoque disponível, nunca negativo.
    stock: i64,
    /// Categoria à qual o produto pertence.
    category_id: UserId,
    /// Estado do registro.
    status: EntityStatus,
    /// Momento de criação.
    created_at: DateTime<Utc>,
    /// Momento da última mutação.
    updated_at: DateTime<Utc>,
    /// Momento do soft delete, quando houver.
    deleted_at: Option<DateTime<Utc>>,
}

impl Product {
    /// Cria um produto novo e ativo.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::InvalidName`] para nome inválido,
    /// [`DomainError::InvalidStock`] para estoque negativo e
    /// [`DomainError::InvalidDescription`] para descrição longa demais.
    pub fn new(
        name: impl Into<String>,
        sku: Sku,
        description: Option<String>,
        price: Money,
        stock: i64,
        category_id: UserId,
        now: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        Ok(Self {
            id: UserId::new_v4(),
            name: validate_name(name)?,
            sku,
            description: validate_description(description)?,
            price,
            stock: validate_stock(stock)?,
            category_id,
            status: EntityStatus::Active,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        })
    }

    /// Reconstitui um produto já persistido.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError`] para nome, estoque, descrição ou datas inválidos.
    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        id: UserId,
        name: impl Into<String>,
        sku: Sku,
        description: Option<String>,
        price: Money,
        stock: i64,
        category_id: UserId,
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
            sku,
            description: validate_description(description)?,
            price,
            stock: validate_stock(stock)?,
            category_id,
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

    /// Devolve o SKU normalizado.
    #[must_use]
    pub fn sku(&self) -> &Sku {
        &self.sku
    }

    /// Devolve a descrição, quando houver.
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Devolve o preço.
    #[must_use]
    pub fn price(&self) -> Money {
        self.price
    }

    /// Devolve o estoque atual.
    #[must_use]
    pub fn stock(&self) -> i64 {
        self.stock
    }

    /// Devolve o identificador da categoria.
    #[must_use]
    pub fn category_id(&self) -> UserId {
        self.category_id
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

    /// Indica se o produto está vivo e ativo.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.status.is_active() && self.deleted_at.is_none()
    }

    /// Renomeia o produto.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::DeletedUser`] se excluído e
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
    /// Retorna [`DomainError::DeletedUser`] se excluído e
    /// [`DomainError::InvalidDescription`] para texto inválido.
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

    /// Troca o preço.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::DeletedUser`] se excluído.
    pub fn change_price(&mut self, price: Money, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.ensure_not_deleted()?;
        self.price = price;
        self.touch(now);
        Ok(())
    }

    /// Define o estoque absoluto.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::DeletedUser`] se excluído e
    /// [`DomainError::InvalidStock`] para valor negativo.
    pub fn set_stock(&mut self, stock: i64, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.ensure_not_deleted()?;
        self.stock = validate_stock(stock)?;
        self.touch(now);
        Ok(())
    }

    /// Move o produto para outra categoria.
    ///
    /// A existência da categoria é responsabilidade de quem chama: o domínio não
    /// consulta repositórios.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::DeletedUser`] se excluído.
    pub fn change_category(
        &mut self,
        category_id: UserId,
        now: DateTime<Utc>,
    ) -> Result<(), DomainError> {
        self.ensure_not_deleted()?;
        self.category_id = category_id;
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

    /// Garante que o produto não recebeu soft delete.
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

/// Normaliza e valida o nome do produto.
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
    if trimmed.chars().count() > MAX_PRODUCT_NAME_LEN {
        return Err(DomainError::InvalidName(format!(
            "deve ter no máximo {MAX_PRODUCT_NAME_LEN} caracteres"
        )));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(DomainError::InvalidName(
            "não pode conter caracteres de controle".to_owned(),
        ));
    }

    Ok(trimmed.to_owned())
}

/// Valida que o estoque não é negativo.
///
/// # Errors
///
/// Retorna [`DomainError::InvalidStock`] para valor negativo.
fn validate_stock(stock: i64) -> Result<i64, DomainError> {
    if stock < 0 {
        return Err(DomainError::InvalidStock(
            "o estoque não pode ser negativo".to_owned(),
        ));
    }
    Ok(stock)
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

    /// SKU válido para reuso.
    fn sku() -> Sku {
        Sku::parse("SKU-001").expect("sku válido")
    }

    /// Preço válido para reuso.
    fn price() -> Money {
        Money::from_cents(1_999).expect("preço válido")
    }

    /// Categoria fixa usada como referência.
    fn category_id() -> UserId {
        UserId::new_v4()
    }

    /// Produto válido construído com os auxiliares acima.
    fn product() -> Product {
        Product::new("Teclado", sku(), None, price(), 10, category_id(), now()).expect("válido")
    }

    #[test]
    fn new_product_is_active() {
        let product = product();

        assert_eq!(product.name(), "Teclado");
        assert_eq!(product.sku().as_str(), "SKU-001");
        assert_eq!(product.price().cents(), 1_999);
        assert_eq!(product.stock(), 10);
        assert!(product.is_active());
    }

    #[test]
    fn rejects_negative_stock() {
        let result = Product::new("Teclado", sku(), None, price(), -1, category_id(), now());

        assert!(matches!(result, Err(DomainError::InvalidStock(_))));
    }

    #[test]
    fn accepts_zero_stock() {
        let result = Product::new("Esgotado", sku(), None, price(), 0, category_id(), now());

        assert!(result.is_ok(), "estoque zero é válido (produto esgotado)");
    }

    #[test]
    fn rejects_empty_name() {
        let result = Product::new("   ", sku(), None, price(), 1, category_id(), now());

        assert!(matches!(result, Err(DomainError::InvalidName(_))));
    }

    #[test]
    fn set_stock_rejects_negative() {
        let mut product = product();

        assert!(product.set_stock(-5, now()).is_err());
        assert_eq!(product.stock(), 10, "estoque não pode ter mudado");
    }

    #[test]
    fn set_stock_accepts_zero() {
        let mut product = product();

        product.set_stock(0, now()).expect("estoque zero");

        assert_eq!(product.stock(), 0);
    }

    #[test]
    fn change_price_updates_timestamp() {
        let mut product = product();
        let later = now() + Duration::seconds(30);
        let new_price = Money::from_cents(2_500).expect("válido");

        product
            .change_price(new_price, later)
            .expect("trocar preço");

        assert_eq!(product.price().cents(), 2_500);
        assert_eq!(product.updated_at(), later);
    }

    #[test]
    fn change_category_moves_product() {
        let mut product = product();
        let other = UserId::new_v4();

        product.change_category(other, now()).expect("mover");

        assert_eq!(product.category_id(), other);
    }

    #[test]
    fn soft_delete_is_idempotent() {
        let mut product = product();
        let later = now() + Duration::seconds(60);

        product.soft_delete(later);
        product.soft_delete(later + Duration::seconds(30));

        assert!(!product.is_active());
        assert_eq!(product.deleted_at(), Some(later));
    }

    #[test]
    fn mutations_rejected_after_delete() {
        let mut product = product();
        product.soft_delete(now());

        assert!(product.rename("Outro", now()).is_err());
        assert!(product.set_stock(1, now()).is_err());
        assert!(product.change_price(price(), now()).is_err());
    }
}
