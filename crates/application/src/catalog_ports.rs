//! Portas de persistência do catálogo: categorias e produtos.
//!
//! Separadas das portas de usuário porque mudam por motivos diferentes e
//! permitem testar o catálogo sem arrastar autenticação.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use domain::{Category, Product, Sku, UserId};

use crate::error::AppError;

/// Campos pelos quais uma listagem de produtos pode ser ordenada.
///
/// É uma allowlist deliberada: aceitar um nome de coluna arbitrário vindo do
/// cliente abriria caminho para injeção em `ORDER BY`, onde parâmetros
/// vinculados não são suportados.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductSort {
    /// Mais recentes primeiro.
    Newest,
    /// Mais antigos primeiro.
    Oldest,
    /// Nome em ordem alfabética.
    Name,
    /// Menor preço primeiro.
    Cheapest,
    /// Maior preço primeiro.
    MostExpensive,
}

impl ProductSort {
    /// Converte o texto da query string na ordenação correspondente.
    ///
    /// `None` significa "não informado" e usa [`ProductSort::Newest`].
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::InvalidSort`] para qualquer valor fora da allowlist —
    /// o critério de aceite exige `400` em ordenação inválida, não ignorar.
    pub fn parse(value: Option<&str>) -> Result<Self, AppError> {
        match value {
            None | Some("") => Ok(Self::Newest),
            Some("newest") => Ok(Self::Newest),
            Some("oldest") => Ok(Self::Oldest),
            Some("name") => Ok(Self::Name),
            Some("cheapest") => Ok(Self::Cheapest),
            Some("most_expensive") => Ok(Self::MostExpensive),
            Some(other) => Err(AppError::InvalidSort(other.to_owned())),
        }
    }

    /// Fragmento `ORDER BY` correspondente.
    ///
    /// Constante e derivado do enum, nunca do texto do cliente.
    #[must_use]
    pub const fn order_by(self) -> &'static str {
        match self {
            Self::Newest => "created_at DESC, id DESC",
            Self::Oldest => "created_at ASC, id ASC",
            Self::Name => "name ASC, id ASC",
            Self::Cheapest => "price_cents ASC, id ASC",
            Self::MostExpensive => "price_cents DESC, id DESC",
        }
    }
}

/// Filtros aceitos na listagem de produtos.
#[derive(Debug, Clone, Default)]
pub struct ProductFilter {
    /// Restringe a uma categoria.
    pub category_id: Option<UserId>,
    /// Busca por trecho do nome (case-insensitive).
    pub name_contains: Option<String>,
    /// Restringe ao status informado.
    pub only_active: Option<bool>,
}

/// Porta de persistência de categorias.
#[async_trait]
pub trait CategoryRepository: Send + Sync {
    /// Busca uma categoria viva por identificador.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] em falha de consulta.
    async fn find_by_id(&self, id: UserId) -> Result<Option<Category>, AppError>;

    /// Busca uma categoria viva pelo nome.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] em falha de consulta.
    async fn find_by_name(&self, name: &str) -> Result<Option<Category>, AppError>;

    /// Persiste uma categoria nova.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::AlreadyInUse`] em colisão de nome com outra categoria
    /// ativa e [`AppError::Infrastructure`] nas demais falhas.
    async fn insert(&self, category: &Category) -> Result<(), AppError>;

    /// Persiste alterações em uma categoria existente.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::AlreadyInUse`] em colisão de nome,
    /// [`AppError::CategoryNotFound`] se a linha não existir e
    /// [`AppError::Infrastructure`] nas demais falhas.
    async fn update(&self, category: &Category) -> Result<(), AppError>;

    /// Lista categorias vivas com paginação estável.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] em falha de consulta.
    async fn list(&self, offset: i64, limit: i64) -> Result<Vec<Category>, AppError>;

    /// Conta as categorias vivas.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] em falha de consulta.
    async fn count(&self) -> Result<i64, AppError>;

    /// Conta quantos produtos vivos estão vinculados à categoria.
    ///
    /// Usado antes de excluir: existir produto implica conflito.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] em falha de consulta.
    async fn count_products(&self, category_id: UserId) -> Result<i64, AppError>;
}

/// Porta de persistência de produtos.
#[async_trait]
pub trait ProductRepository: Send + Sync {
    /// Busca um produto vivo por identificador.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] em falha de consulta.
    async fn find_by_id(&self, id: UserId) -> Result<Option<Product>, AppError>;

    /// Busca um produto vivo por SKU.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] em falha de consulta.
    async fn find_by_sku(&self, sku: &Sku) -> Result<Option<Product>, AppError>;

    /// Persiste um produto novo.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::AlreadyInUse`] em colisão de SKU e
    /// [`AppError::CategoryNotFound`] quando a FK não encontra a categoria.
    async fn insert(&self, product: &Product) -> Result<(), AppError>;

    /// Persiste alterações em um produto existente.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::AlreadyInUse`] em colisão de SKU,
    /// [`AppError::NotFound`] se a linha não existir e
    /// [`AppError::Infrastructure`] nas demais falhas.
    async fn update(&self, product: &Product) -> Result<(), AppError>;

    /// Lista produtos vivos aplicando filtros e ordenação.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] em falha de consulta.
    async fn list(
        &self,
        filter: &ProductFilter,
        sort: ProductSort,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<Product>, AppError>;

    /// Conta os produtos vivos que atendem aos filtros.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] em falha de consulta.
    async fn count(&self, filter: &ProductFilter) -> Result<i64, AppError>;
}

/// Formata um instante para o padrão de texto do SQLite.
///
/// Centralizado para que todos os adaptadores gravem datas comparáveis como
/// texto (RFC 3339 UTC com microssegundos).
#[must_use]
pub fn format_timestamp(value: DateTime<Utc>) -> String {
    use chrono::SecondsFormat;
    value.to_rfc3339_opts(SecondsFormat::Micros, true)
}
