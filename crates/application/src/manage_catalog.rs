//! Casos de uso do catálogo: categorias e produtos.
//!
//! As regras de acesso vêm do papel do [`Actor`]: manter o catálogo exige
//! [`Role::can_manage_catalog`] (Admin ou Manager). A leitura é pública, o que
//! corresponde ao requisito "User consulta o catálogo".

use chrono::{DateTime, Utc};
use domain::{
    Category, DELETE_WITH_PRODUCTS_POLICY, Money, PriceChange, Product, Sku, StockMovement,
    StockReason, UserId,
};

use crate::audit::{AuditAction, AuditEntity, AuditEntry, AuditRepository};
use crate::catalog_ports::{CategoryRepository, ProductFilter, ProductRepository, ProductSort};
use crate::error::AppError;
use crate::history_ports::{CatalogHistoryRepository, ProductChange};
use crate::pagination::normalize;
use crate::policy::Actor;

/// Página de resultados genérica.
#[derive(Debug, Clone)]
pub struct Page<T> {
    /// Itens da página atual.
    pub items: Vec<T>,
    /// Total de registros que atendem ao filtro.
    pub total: i64,
    /// Offset aplicado.
    pub offset: i64,
    /// Limite aplicado.
    pub limit: i64,
}

/// Dados para criar uma categoria.
#[derive(Debug, Clone)]
pub struct NewCategoryInput {
    /// Nome único entre categorias ativas.
    pub name: String,
    /// Descrição opcional.
    pub description: Option<String>,
}

/// Dados para atualizar uma categoria.
#[derive(Debug, Clone, Default)]
pub struct UpdateCategoryInput {
    /// Novo nome, se informado.
    pub name: Option<String>,
    /// Nova descrição. `Some(None)` limpa o campo.
    pub description: Option<Option<String>>,
}

/// Dados para criar um produto.
#[derive(Debug, Clone)]
pub struct NewProductInput {
    /// Nome comercial.
    pub name: String,
    /// SKU em texto, normalizado no domínio.
    pub sku: String,
    /// Descrição opcional.
    pub description: Option<String>,
    /// Preço em centavos.
    pub price_cents: i64,
    /// Estoque inicial.
    pub stock: i64,
    /// Categoria à qual pertence.
    pub category_id: UserId,
}

/// Dados para atualizar um produto.
#[derive(Debug, Clone, Default)]
pub struct UpdateProductInput {
    /// Novo nome, se informado.
    pub name: Option<String>,
    /// Nova descrição. `Some(None)` limpa o campo.
    pub description: Option<Option<String>>,
    /// Novo preço em centavos, se informado.
    pub price_cents: Option<i64>,
    /// Novo estoque, se informado.
    pub stock: Option<i64>,
    /// Nova categoria, se informada.
    pub category_id: Option<UserId>,
}

/// Casos de uso de categorias.
pub struct ManageCategories<'a> {
    /// Repositório de categorias.
    categories: &'a dyn CategoryRepository,
    /// Trilha de auditoria; `None` desabilita o registro.
    audit: Option<&'a dyn AuditRepository>,
}

impl<'a> ManageCategories<'a> {
    /// Monta o caso de uso com o repositório injetado.
    #[must_use]
    pub fn new(categories: &'a dyn CategoryRepository) -> Self {
        Self {
            categories,
            audit: None,
        }
    }

    /// Liga a trilha de auditoria a este caso de uso.
    #[must_use]
    pub fn with_audit(mut self, audit: &'a dyn AuditRepository) -> Self {
        self.audit = Some(audit);
        self
    }

    /// Monta a entrada de auditoria de uma mutação de categoria, quando ligada.
    ///
    /// Devolve a entrada pronta para ser gravada **na mesma transação** da
    /// mutação; gravar depois deixaria a alteração sem autor.
    fn audit_entry(
        &self,
        actor: Actor,
        action: AuditAction,
        before: Option<&Category>,
        after: &Category,
        at: DateTime<Utc>,
    ) -> Option<AuditEntry> {
        self.audit?;

        let snapshot = |category: &Category| {
            serde_json::json!({
                "id": category.id().to_string(),
                "name": category.name(),
                "description": category.description(),
                "status": category.status().as_str(),
            })
        };

        let entity_id = after.id().to_string();
        let after_json = snapshot(after);

        Some(match before {
            Some(before) => AuditEntry::updated(
                Some(actor.id),
                action,
                AuditEntity::Category,
                entity_id,
                snapshot(before),
                after_json,
                at,
            ),
            None => AuditEntry::created(
                Some(actor.id),
                AuditEntity::Category,
                entity_id,
                after_json,
                at,
            ),
        })
    }

    /// Lista categorias vivas com paginação.
    ///
    /// Leitura é pública: qualquer ator autenticado consulta o catálogo.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] em falha de consulta.
    pub async fn list(&self, offset: i64, limit: i64) -> Result<Page<Category>, AppError> {
        let (offset, limit) = normalize(offset, limit)?;

        let items = self.categories.list(offset, limit).await?;
        let total = self.categories.count().await?;

        Ok(Page {
            items,
            total,
            offset,
            limit,
        })
    }

    /// Lê uma categoria pelo identificador.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::CategoryNotFound`] quando não existe e
    /// [`AppError::Infrastructure`] em falha de consulta.
    pub async fn view(&self, id: UserId) -> Result<Category, AppError> {
        self.categories
            .find_by_id(id)
            .await?
            .ok_or(AppError::CategoryNotFound)
    }

    /// Cria uma categoria.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Forbidden`] sem privilégio de catálogo,
    /// [`AppError::Domain`] para dados inválidos,
    /// [`AppError::AlreadyInUse`] se o nome já existir entre categorias ativas
    /// e [`AppError::Infrastructure`] em falha de gravação.
    pub async fn create(
        &self,
        actor: Actor,
        input: NewCategoryInput,
        now: DateTime<Utc>,
    ) -> Result<Category, AppError> {
        require_catalog_manager(actor)?;

        let category = Category::new(input.name, input.description, now)?;
        let audit = self.audit_entry(actor, AuditAction::Create, None, &category, now);
        self.categories.insert(&category, audit.as_ref()).await?;
        Ok(category)
    }

    /// Atualiza nome e/ou descrição de uma categoria.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Forbidden`] sem privilégio,
    /// [`AppError::CategoryNotFound`] se não existir,
    /// [`AppError::AlreadyInUse`] em colisão de nome e
    /// [`AppError::Domain`] para dados inválidos.
    pub async fn update(
        &self,
        actor: Actor,
        id: UserId,
        input: UpdateCategoryInput,
        now: DateTime<Utc>,
    ) -> Result<Category, AppError> {
        require_catalog_manager(actor)?;

        let mut category = self.view(id).await?;
        let before = category.clone();

        if let Some(name) = input.name {
            category.rename(name, now)?;
        }
        if let Some(description) = input.description {
            category.change_description(description, now)?;
        }

        let audit = self.audit_entry(actor, AuditAction::Update, Some(&before), &category, now);
        self.categories.update(&category, audit.as_ref()).await?;
        Ok(category)
    }

    /// Exclui uma categoria, recusando quando há produtos vinculados.
    ///
    /// Este é o ponto que implementa [`DELETE_WITH_PRODUCTS_POLICY`]: verificar
    /// antes evita depender apenas do erro de FK, que chegaria como falha de
    /// infraestrutura em vez de conflito de negócio.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Forbidden`] sem privilégio,
    /// [`AppError::CategoryNotFound`] se não existir,
    /// [`AppError::Conflict`] quando há produtos ativos e
    /// [`AppError::Infrastructure`] em falha de gravação.
    pub async fn delete(
        &self,
        actor: Actor,
        id: UserId,
        now: DateTime<Utc>,
    ) -> Result<Category, AppError> {
        require_catalog_manager(actor)?;

        let mut category = self.view(id).await?;
        let before = category.clone();

        let products = self.categories.count_products(id).await?;
        if products > 0 {
            return Err(AppError::Conflict(DELETE_WITH_PRODUCTS_POLICY.to_owned()));
        }

        category.soft_delete(now);
        let audit = self.audit_entry(actor, AuditAction::Delete, Some(&before), &category, now);
        self.categories.update(&category, audit.as_ref()).await?;
        Ok(category)
    }
}

/// Casos de uso de produtos.
pub struct ManageProducts<'a> {
    /// Repositório de produtos.
    products: &'a dyn ProductRepository,
    /// Repositório de categorias, para validar a referência antes de gravar.
    categories: &'a dyn CategoryRepository,
    /// Histórico que acompanha cada alteração de estoque e preço.
    history: &'a dyn CatalogHistoryRepository,
    /// Trilha de auditoria; `None` desabilita o registro.
    audit: Option<&'a dyn AuditRepository>,
}

impl<'a> ManageProducts<'a> {
    /// Monta o caso de uso com os repositórios injetados.
    ///
    /// A porta de histórico é obrigatória: sem ela, um produto poderia ser
    /// gravado sem deixar rastro de estoque/preço, e a fase 8 existe justamente
    /// para eliminar esse ponto cego.
    #[must_use]
    pub fn new(
        products: &'a dyn ProductRepository,
        categories: &'a dyn CategoryRepository,
        history: &'a dyn CatalogHistoryRepository,
    ) -> Self {
        Self {
            products,
            categories,
            history,
            audit: None,
        }
    }

    /// Liga a trilha de auditoria a este caso de uso.
    #[must_use]
    pub fn with_audit(mut self, audit: &'a dyn AuditRepository) -> Self {
        self.audit = Some(audit);
        self
    }

    /// Registra uma mutação de produto na trilha, quando ligada.
    ///
    /// Devolve a entrada pronta em vez de gravá-la: quem chama precisa gravá-la
    /// **na mesma transação** da mutação, senão a alteração pode ficar sem
    /// registro de autor.
    ///
    /// # Errors
    ///
    /// Não falha; devolve `None` quando não há trilha configurada.
    fn audit_entry(
        &self,
        actor: Actor,
        action: AuditAction,
        before: Option<&Product>,
        after: &Product,
        at: DateTime<Utc>,
    ) -> Option<AuditEntry> {
        self.audit?;

        let snapshot = |product: &Product| {
            serde_json::json!({
                "id": product.id().to_string(),
                "name": product.name(),
                "sku": product.sku().as_str(),
                "price_cents": product.price().cents(),
                "stock": product.stock(),
                "category_id": product.category_id().to_string(),
                "status": product.status().as_str(),
            })
        };

        let entity_id = after.id().to_string();
        let after_json = snapshot(after);

        Some(match before {
            Some(before) => AuditEntry::updated(
                Some(actor.id),
                action,
                AuditEntity::Product,
                entity_id,
                snapshot(before),
                after_json,
                at,
            ),
            None => AuditEntry::created(
                Some(actor.id),
                AuditEntity::Product,
                entity_id,
                after_json,
                at,
            ),
        })
    }

    /// Lista produtos aplicando filtros, ordenação e paginação.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::InvalidSort`] para ordenação fora da allowlist e
    /// [`AppError::Infrastructure`] em falha de consulta.
    pub async fn list(
        &self,
        filter: ProductFilter,
        sort: ProductSort,
        offset: i64,
        limit: i64,
    ) -> Result<Page<Product>, AppError> {
        let (offset, limit) = normalize(offset, limit)?;

        let items = self.products.list(&filter, sort, offset, limit).await?;
        let total = self.products.count(&filter).await?;

        Ok(Page {
            items,
            total,
            offset,
            limit,
        })
    }

    /// Lê um produto pelo identificador.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::NotFound`] quando não existe e
    /// [`AppError::Infrastructure`] em falha de consulta.
    pub async fn view(&self, id: UserId) -> Result<Product, AppError> {
        self.products
            .find_by_id(id)
            .await?
            .ok_or(AppError::NotFound)
    }

    /// Cria um produto, validando a categoria referenciada.
    ///
    /// A verificação da categoria é explícita para devolver
    /// [`AppError::CategoryNotFound`] em vez de deixar a FK falhar como erro
    /// interno.
    ///
    /// O estoque inicial é registrado no histórico com motivo
    /// [`StockReason::Initial`], na mesma transação da inserção do produto.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Forbidden`] sem privilégio,
    /// [`AppError::Domain`] para SKU/preço/estoque inválidos,
    /// [`AppError::CategoryNotFound`] se a categoria não existir,
    /// [`AppError::AlreadyInUse`] em colisão de SKU e
    /// [`AppError::Infrastructure`] em falha de gravação.
    pub async fn create(
        &self,
        actor: Actor,
        input: NewProductInput,
        now: DateTime<Utc>,
    ) -> Result<Product, AppError> {
        require_catalog_manager(actor)?;

        self.ensure_category_exists(input.category_id).await?;

        let sku = Sku::parse(&input.sku)?;
        let price = Money::from_cents(input.price_cents)?;

        let product = Product::new(
            input.name,
            sku,
            input.description,
            price,
            input.stock,
            input.category_id,
            now,
        )?;

        // Estoque inicial só entra no histórico quando é diferente de zero:
        // uma movimentação de delta zero não explicaria nada.
        let movement = if input.stock > 0 {
            Some(StockMovement::new(
                product.id(),
                input.stock,
                StockReason::Initial,
                Some(actor.id),
                now,
            )?)
        } else {
            None
        };

        let audit = self.audit_entry(actor, AuditAction::Create, None, &product, now);
        self.history
            .insert_product(&product, movement.as_ref(), audit.as_ref())
            .await?;
        Ok(product)
    }

    /// Atualiza campos de um produto.
    ///
    /// Mudanças de estoque e de preço geram histórico com o **delta** e o valor
    /// anterior, respectivamente. Produto e histórico são gravados na mesma
    /// transação: um saldo que mudou sem explicação é pior do que um erro.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Forbidden`] sem privilégio,
    /// [`AppError::NotFound`] se o produto não existir,
    /// [`AppError::CategoryNotFound`] se a nova categoria não existir,
    /// [`AppError::Domain`] para valores inválidos e
    /// [`AppError::Infrastructure`] em falha de gravação.
    pub async fn update(
        &self,
        actor: Actor,
        id: UserId,
        input: UpdateProductInput,
        now: DateTime<Utc>,
    ) -> Result<Product, AppError> {
        require_catalog_manager(actor)?;

        let mut product = self.view(id).await?;
        let before = product.clone();

        // Guarda o estado anterior antes de mutar: é o que permite calcular o
        // delta do estoque e o preço antigo do histórico.
        let previous_stock = product.stock();
        let previous_price = product.price().cents();

        if let Some(name) = input.name {
            product.rename(name, now)?;
        }
        if let Some(description) = input.description {
            product.change_description(description, now)?;
        }
        if let Some(cents) = input.price_cents {
            product.change_price(Money::from_cents(cents)?, now)?;
        }
        if let Some(stock) = input.stock {
            product.set_stock(stock, now)?;
        }
        if let Some(category_id) = input.category_id {
            self.ensure_category_exists(category_id).await?;
            product.change_category(category_id, now)?;
        }

        let stock_delta = product.stock() - previous_stock;
        let movement = if stock_delta == 0 {
            None
        } else {
            Some(StockMovement::new(
                product.id(),
                stock_delta,
                StockReason::Correction,
                Some(actor.id),
                now,
            )?)
        };

        let price_change = if product.price().cents() == previous_price {
            None
        } else {
            Some(PriceChange::new(
                product.id(),
                previous_price,
                product.price().cents(),
                Some(actor.id),
                now,
            )?)
        };

        let audit = self.audit_entry(actor, AuditAction::Update, Some(&before), &product, now);
        let change = ProductChange {
            movement: movement.as_ref(),
            price: price_change.as_ref(),
            audit: audit.as_ref(),
        };
        self.history.update_product(&product, &change).await?;
        Ok(product)
    }

    /// Exclui um produto via soft delete.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Forbidden`] sem privilégio,
    /// [`AppError::NotFound`] se não existir e
    /// [`AppError::Infrastructure`] em falha de gravação.
    pub async fn delete(
        &self,
        actor: Actor,
        id: UserId,
        now: DateTime<Utc>,
    ) -> Result<Product, AppError> {
        require_catalog_manager(actor)?;

        let mut product = self.view(id).await?;
        let before = product.clone();
        product.soft_delete(now);

        let audit = self.audit_entry(actor, AuditAction::Delete, Some(&before), &product, now);
        let change = ProductChange {
            movement: None,
            price: None,
            audit: audit.as_ref(),
        };
        self.history.update_product(&product, &change).await?;
        Ok(product)
    }

    /// Garante que a categoria existe e está viva.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::CategoryNotFound`] quando não encontrada e
    /// [`AppError::Infrastructure`] em falha de consulta.
    async fn ensure_category_exists(&self, category_id: UserId) -> Result<(), AppError> {
        if self.categories.find_by_id(category_id).await?.is_none() {
            return Err(AppError::CategoryNotFound);
        }
        Ok(())
    }
}

/// Exige que o ator possa manter o catálogo (Admin ou Manager).
///
/// # Errors
///
/// Retorna [`AppError::Forbidden`] para qualquer outro papel.
fn require_catalog_manager(actor: Actor) -> Result<(), AppError> {
    if actor.role.can_manage_catalog() {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog_ports::format_timestamp;
    use async_trait::async_trait;
    use chrono::Duration;
    use domain::{Role, UserId};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Instante fixo dos testes.
    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-14T12:00:00Z")
            .expect("data válida")
            .with_timezone(&Utc)
    }

    /// Ator com o papel informado.
    fn actor(role: Role) -> Actor {
        Actor::new(UserId::new_v4(), role)
    }

    /// Repositório em memória de categorias, para testar casos de uso isolados.
    #[derive(Default)]
    struct FakeCategories {
        /// Categorias cadastradas no dublê, indexadas pelo identificador.
        items: Mutex<HashMap<UserId, Category>>,
        /// Quantidade simulada de produtos vinculados a cada categoria.
        products: Mutex<HashMap<UserId, i64>>,
    }

    #[async_trait]
    impl CategoryRepository for FakeCategories {
        /// Busca no estado em memória do dublê e retorna `Some` quando o ID coincide, `None` caso contrário.
        async fn find_by_id(&self, id: UserId) -> Result<Option<Category>, AppError> {
            Ok(self.items.lock().expect("lock").get(&id).cloned())
        }
        /// Busca a categoria pelo nome e retorna `Some` quando encontrada, `None` quando ausente ou erro de persistência.
        async fn find_by_name(&self, name: &str) -> Result<Option<Category>, AppError> {
            Ok(self
                .items
                .lock()
                .expect("lock")
                .values()
                .find(|category| category.name() == name)
                .cloned())
        }
        /// Adiciona o registro ao estado em memória do dublê e retorna sucesso; os dublês de corrida podem devolver o conflito configurado.
        async fn insert(
            &self,
            category: &Category,
            _audit: Option<&AuditEntry>,
        ) -> Result<(), AppError> {
            let mut guard = self.items.lock().expect("lock");
            if guard
                .values()
                .any(|existing| existing.name() == category.name())
            {
                return Err(AppError::AlreadyInUse(category.name().to_owned()));
            }
            guard.insert(category.id(), category.clone());
            Ok(())
        }
        /// Substitui o registro no estado em memória e retorna sucesso ou o erro de ausência definido pelo cenário.
        async fn update(
            &self,
            category: &Category,
            _audit: Option<&AuditEntry>,
        ) -> Result<(), AppError> {
            let mut guard = self.items.lock().expect("lock");
            if !guard.contains_key(&category.id()) {
                return Err(AppError::CategoryNotFound);
            }
            guard.insert(category.id(), category.clone());
            Ok(())
        }
        /// Retorna do estado em memória a página solicitada pelo offset e limite.
        async fn list(&self, offset: i64, limit: i64) -> Result<Vec<Category>, AppError> {
            let guard = self.items.lock().expect("lock");
            let mut items: Vec<_> = guard.values().cloned().collect();
            items.sort_by_key(|category| category.created_at());
            Ok(items
                .into_iter()
                .skip(offset as usize)
                .take(limit as usize)
                .collect())
        }
        /// Retorna a quantidade de registros mantidos pelo dublê em memória.
        async fn count(&self) -> Result<i64, AppError> {
            Ok(self.items.lock().expect("lock").len() as i64)
        }
        /// Retorna a quantidade de produtos de fixture associada à categoria.
        async fn count_products(&self, category_id: UserId) -> Result<i64, AppError> {
            Ok(*self
                .products
                .lock()
                .expect("lock")
                .get(&category_id)
                .unwrap_or(&0))
        }
    }

    impl FakeCategories {
        /// Registra que a categoria tem `count` produtos vinculados.
        fn set_products(&self, category_id: UserId, count: i64) {
            self.products
                .lock()
                .expect("lock")
                .insert(category_id, count);
        }
    }

    /// Verifica o contrato identificado por `admin_creates_category` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[tokio::test]
    async fn admin_creates_category() {
        let repository = FakeCategories::default();
        let use_case = ManageCategories::new(&repository);

        let category = use_case
            .create(
                actor(Role::Admin),
                NewCategoryInput {
                    name: "Eletrônicos".to_owned(),
                    description: None,
                },
                now(),
            )
            .await
            .expect("criar");

        assert_eq!(category.name(), "Eletrônicos");
        assert_eq!(repository.count().await.expect("count"), 1);
    }

    /// Verifica o contrato identificado por `manager_creates_category` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[tokio::test]
    async fn manager_creates_category() {
        let repository = FakeCategories::default();
        let use_case = ManageCategories::new(&repository);

        assert!(
            use_case
                .create(
                    actor(Role::Manager),
                    NewCategoryInput {
                        name: "Livros".to_owned(),
                        description: None,
                    },
                    now(),
                )
                .await
                .is_ok()
        );
    }

    /// Verifica o contrato identificado por `user_cannot_create_category` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[tokio::test]
    async fn user_cannot_create_category() {
        let repository = FakeCategories::default();
        let use_case = ManageCategories::new(&repository);

        let error = use_case
            .create(
                actor(Role::User),
                NewCategoryInput {
                    name: "Proibida".to_owned(),
                    description: None,
                },
                now(),
            )
            .await
            .expect_err("sem permissão");

        assert!(matches!(error, AppError::Forbidden));
    }

    /// Verifica o contrato identificado por `duplicate_name_is_rejected` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[tokio::test]
    async fn duplicate_name_is_rejected() {
        let repository = FakeCategories::default();
        let use_case = ManageCategories::new(&repository);
        let input = NewCategoryInput {
            name: "Eletrônicos".to_owned(),
            description: None,
        };

        use_case
            .create(actor(Role::Admin), input.clone(), now())
            .await
            .expect("primeira");

        let error = use_case
            .create(actor(Role::Admin), input, now())
            .await
            .expect_err("duplicada");

        assert!(matches!(error, AppError::AlreadyInUse(_)));
    }

    /// Verifica o contrato identificado por `deleting_category_with_products_is_a_conflict` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[tokio::test]
    async fn deleting_category_with_products_is_a_conflict() {
        let repository = FakeCategories::default();
        let use_case = ManageCategories::new(&repository);

        let category = use_case
            .create(
                actor(Role::Admin),
                NewCategoryInput {
                    name: "Com produtos".to_owned(),
                    description: None,
                },
                now(),
            )
            .await
            .expect("criar");

        repository.set_products(category.id(), 3);

        let error = use_case
            .delete(actor(Role::Admin), category.id(), now())
            .await
            .expect_err("deve conflitar");

        assert!(
            matches!(error, AppError::Conflict(ref message) if message.contains("produtos")),
            "mensagem deve explicar o conflito: {error:?}"
        );

        // E a categoria continua viva.
        assert!(use_case.view(category.id()).await.is_ok());
    }

    /// Verifica o contrato identificado por `deleting_empty_category_succeeds` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[tokio::test]
    async fn deleting_empty_category_succeeds() {
        let repository = FakeCategories::default();
        let use_case = ManageCategories::new(&repository);

        let category = use_case
            .create(
                actor(Role::Admin),
                NewCategoryInput {
                    name: "Vazia".to_owned(),
                    description: None,
                },
                now(),
            )
            .await
            .expect("criar");

        let deleted = use_case
            .delete(actor(Role::Admin), category.id(), now())
            .await
            .expect("excluir");

        assert!(deleted.deleted_at().is_some());
    }

    /// Verifica o contrato identificado por `update_renames_category` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[tokio::test]
    async fn update_renames_category() {
        let repository = FakeCategories::default();
        let use_case = ManageCategories::new(&repository);

        let category = use_case
            .create(
                actor(Role::Admin),
                NewCategoryInput {
                    name: "Antiga".to_owned(),
                    description: None,
                },
                now(),
            )
            .await
            .expect("criar");

        let later = now() + Duration::seconds(30);
        let updated = use_case
            .update(
                actor(Role::Manager),
                category.id(),
                UpdateCategoryInput {
                    name: Some("Nova".to_owned()),
                    description: Some(Some("descrição".to_owned())),
                },
                later,
            )
            .await
            .expect("atualizar");

        assert_eq!(updated.name(), "Nova");
        assert_eq!(updated.description(), Some("descrição"));
    }

    /// Verifica o contrato identificado por `product_sort_accepts_only_allowlisted_values` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn product_sort_accepts_only_allowlisted_values() {
        assert_eq!(
            ProductSort::parse(None).expect("padrão"),
            ProductSort::Newest
        );
        assert_eq!(
            ProductSort::parse(Some("cheapest")).expect("válido"),
            ProductSort::Cheapest
        );

        let error = ProductSort::parse(Some("price; DROP TABLE users")).expect_err("inválido");
        assert!(matches!(error, AppError::InvalidSort(_)));
    }

    /// Verifica o contrato identificado por `product_sort_fragments_are_fixed_strings` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn product_sort_fragments_are_fixed_strings() {
        // O fragmento nunca vem do cliente, então não há superfície de injeção.
        for sort in [
            ProductSort::Newest,
            ProductSort::Oldest,
            ProductSort::Name,
            ProductSort::Cheapest,
            ProductSort::MostExpensive,
        ] {
            assert!(!sort.order_by().is_empty());
            assert!(!sort.order_by().contains(';'));
        }
    }

    /// Verifica o contrato identificado por `timestamps_are_formatable` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn timestamps_are_formatable() {
        assert!(format_timestamp(now()).starts_with("2026-11-14T12:00:00"));
    }
}
