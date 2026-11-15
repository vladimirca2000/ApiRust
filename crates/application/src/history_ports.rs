//! Porta de histórico do catálogo: movimentação de estoque e mudança de preço.
//!
//! O histórico é **append-only**, mas não pode ser gravado em uma chamada
//! separada da alteração do produto: se o `UPDATE` do produto funcionar e o
//! `INSERT` do histórico falhar, o saldo muda sem explicação. Por isso a porta
//! expõe operações que gravam produto, histórico **e auditoria na mesma
//! transação**.
//!
//! A escrita também usa **controle otimista**: o `UPDATE` só confirma se o
//! saldo e o preço ainda são os que foram lidos. Sem isso, duas requisições
//! concorrentes que leram o mesmo saldo gravaram valores absolutos diferentes e
//! registraram deltas incompatíveis — o saldo deixaria de ser a soma das
//! movimentações.

use async_trait::async_trait;
use domain::{PriceChange, Product, StockMovement, UserId};

use crate::audit::AuditEntry;
use crate::error::AppError;

/// Histórico que acompanha uma alteração de produto.
///
/// Os dois primeiros campos são opcionais porque um `PATCH` pode mudar só o
/// preço, só o estoque ou nenhum dos dois (apenas nome/descrição).
#[derive(Debug, Clone, Copy, Default)]
pub struct ProductChange<'a> {
    /// Movimentação de estoque a registrar, quando o saldo mudou.
    pub movement: Option<&'a StockMovement>,
    /// Alteração de preço a registrar, quando o preço mudou.
    pub price: Option<&'a PriceChange>,
    /// Entrada de auditoria gravada na mesma transação, quando houver.
    pub audit: Option<&'a AuditEntry>,
}

impl<'a> ProductChange<'a> {
    /// Indica se há algo a registrar além do próprio produto.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.movement.is_none() && self.price.is_none() && self.audit.is_none()
    }
}

/// Porta de gravação atômica do produto com seu histórico e de consulta da série.
#[async_trait]
pub trait CatalogHistoryRepository: Send + Sync {
    /// Insere um produto, a movimentação inicial e a auditoria, atomicamente.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::AlreadyInUse`] em colisão de SKU,
    /// [`AppError::CategoryNotFound`] quando a FK não encontra a categoria e
    /// [`AppError::Infrastructure`] nas demais falhas. Nenhuma linha é gravada
    /// se qualquer etapa falhar.
    async fn insert_product(
        &self,
        product: &Product,
        movement: Option<&StockMovement>,
        audit: Option<&AuditEntry>,
    ) -> Result<(), AppError>;

    /// Atualiza um produto e grava histórico/auditoria atomicamente.
    ///
    /// O `UPDATE` é condicional ao saldo e ao preço lidos por quem chamou: se
    /// outro pedido alterou o produto nesse meio-tempo, nada é gravado e o
    /// retorno é [`AppError::Conflict`], para o cliente repetir a leitura.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::AlreadyInUse`] em colisão de SKU,
    /// [`AppError::NotFound`] se a linha não existir,
    /// [`AppError::Conflict`] quando a versão lida está obsoleta e
    /// [`AppError::Infrastructure`] nas demais falhas.
    async fn update_product(
        &self,
        product: &Product,
        change: &ProductChange<'_>,
    ) -> Result<(), AppError>;

    /// Lista as movimentações de um produto, da mais recente para a mais antiga.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a consulta falhar.
    async fn stock_movements(
        &self,
        product_id: UserId,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<StockMovement>, AppError>;

    /// Lista as alterações de preço de um produto, da mais recente para a mais antiga.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a consulta falhar.
    async fn price_changes(
        &self,
        product_id: UserId,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<PriceChange>, AppError>;

    /// Conta as movimentações de estoque de um produto.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a consulta falhar.
    async fn count_stock_movements(&self, product_id: UserId) -> Result<i64, AppError>;

    /// Conta as alterações de preço de um produto.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a consulta falhar.
    async fn count_price_changes(&self, product_id: UserId) -> Result<i64, AppError>;
}

/// Deriva os valores esperados de estoque e preço a partir da mudança.
///
/// O chamador só fornece o estado **final**; a versão esperada é reconstruída
/// do histórico que ele registrou:
///
/// - estoque esperado = estoque novo − delta (quando há movimentação);
/// - preço esperado = `old_cents` (quando há alteração de preço).
///
/// Devolve `(stock, price)` na ordem usada pelo `UPDATE` condicional.
#[must_use]
pub fn expected_values(product: &Product, change: &ProductChange<'_>) -> (i64, i64) {
    let expected_stock = match change.movement {
        Some(movement) => product.stock() - movement.delta,
        None => product.stock(),
    };

    let expected_price = match change.price {
        Some(price) => price.old_cents,
        None => product.price().cents(),
    };

    (expected_stock, expected_price)
}
