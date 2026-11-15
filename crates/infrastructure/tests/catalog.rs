//! Testes dos repositórios de catálogo contra SQLite real.
//!
//! Verificam as invariantes **no banco**: unicidade entre registros ativos,
//! restrição de FK na exclusão de categoria e preservação de valores monetários
//! como inteiros.

mod common;

use application::{CategoryRepository, ProductFilter, ProductRepository, ProductSort};
use chrono::{Duration, Utc};
use domain::{Category, Money, Product, Sku, UserId};
use infrastructure::{SqlxCategoryRepository, SqlxProductRepository};

use common::TempDb;

/// Cria uma categoria persistida e devolve a entidade.
async fn seed_category(temp: &TempDb, name: &str) -> Category {
    let repository = SqlxCategoryRepository::new(temp.db.pool().clone());
    let category = Category::new(name, None, Utc::now()).expect("categoria válida");
    repository.insert(&category).await.expect("inserir");
    category
}

/// Cria um produto persistido na categoria informada.
async fn seed_product(
    temp: &TempDb,
    name: &str,
    sku: &str,
    cents: i64,
    stock: i64,
    category_id: UserId,
) -> Product {
    let repository = SqlxProductRepository::new(temp.db.pool().clone());
    let product = Product::new(
        name,
        Sku::parse(sku).expect("sku"),
        None,
        Money::from_cents(cents).expect("preço"),
        stock,
        category_id,
        Utc::now(),
    )
    .expect("produto válido");

    repository.insert(&product).await.expect("inserir");
    product
}

#[tokio::test]
async fn category_round_trips_all_fields() {
    let temp = TempDb::new("cat-roundtrip").await;
    let repository = SqlxCategoryRepository::new(temp.db.pool().clone());

    let category =
        Category::new("Eletrônicos", Some("Periféricos".to_owned()), Utc::now()).expect("válida");
    repository.insert(&category).await.expect("inserir");

    let found = repository
        .find_by_id(category.id())
        .await
        .expect("buscar")
        .expect("existe");

    assert_eq!(found.name(), "Eletrônicos");
    assert_eq!(found.description(), Some("Periféricos"));
    assert_eq!(
        found.created_at().timestamp(),
        category.created_at().timestamp()
    );
}

#[tokio::test]
async fn category_name_is_unique_among_active_records() {
    let temp = TempDb::new("cat-unique").await;
    let repository = SqlxCategoryRepository::new(temp.db.pool().clone());

    let first = Category::new("Duplicada", None, Utc::now()).expect("válida");
    repository.insert(&first).await.expect("primeira");

    let second = Category::new("Duplicada", None, Utc::now()).expect("válida");
    let error = repository
        .insert(&second)
        .await
        .expect_err("nome duplicado deve falhar");

    assert!(matches!(error, application::AppError::AlreadyInUse(_)));
}

#[tokio::test]
async fn soft_deleted_category_frees_its_name() {
    let temp = TempDb::new("cat-frees-name").await;
    let repository = SqlxCategoryRepository::new(temp.db.pool().clone());

    let mut category = Category::new("Reciclável", None, Utc::now()).expect("válida");
    repository.insert(&category).await.expect("inserir");

    category.soft_delete(Utc::now());
    repository.update(&category).await.expect("soft delete");

    // O índice parcial libera o nome para uma nova categoria ativa.
    let duplicate = Category::new("Reciclável", None, Utc::now()).expect("válida");
    repository
        .insert(&duplicate)
        .await
        .expect("nome deve estar livre após soft delete");
}

#[tokio::test]
async fn counting_products_detects_dependency() {
    let temp = TempDb::new("cat-count-products").await;
    let categories = SqlxCategoryRepository::new(temp.db.pool().clone());

    let category = seed_category(&temp, "Com produtos").await;
    assert_eq!(
        categories
            .count_products(category.id())
            .await
            .expect("contar"),
        0
    );

    seed_product(&temp, "Teclado", "SKU-1", 1_000, 5, category.id()).await;

    assert_eq!(
        categories
            .count_products(category.id())
            .await
            .expect("contar"),
        1,
        "a contagem deve refletir o produto vinculado"
    );
}

#[tokio::test]
async fn product_round_trips_money_and_stock_as_integers() {
    let temp = TempDb::new("prod-roundtrip").await;
    let category = seed_category(&temp, "Informática").await;

    let product = seed_product(&temp, "Monitor", "MON-001", 129_990, 42, category.id()).await;

    let repository = SqlxProductRepository::new(temp.db.pool().clone());
    let found = repository
        .find_by_id(product.id())
        .await
        .expect("buscar")
        .expect("existe");

    assert_eq!(found.sku().as_str(), "MON-001");
    assert_eq!(found.price().cents(), 129_990, "centavos preservados");
    assert_eq!(found.stock(), 42);
    assert_eq!(found.category_id(), category.id());

    // O banco guarda inteiro, não texto nem ponto flutuante.
    let stored: i64 = sqlx::query_scalar("SELECT price_cents FROM products WHERE id = ?")
        .bind(product.id().to_string())
        .fetch_one(temp.db.pool())
        .await
        .expect("preço cru");
    assert_eq!(stored, 129_990);
}

#[tokio::test]
async fn sku_is_unique_among_active_products() {
    let temp = TempDb::new("prod-unique-sku").await;
    let category = seed_category(&temp, "C").await;
    let repository = SqlxProductRepository::new(temp.db.pool().clone());

    seed_product(&temp, "Primeiro", "SKU-X", 100, 1, category.id()).await;

    let duplicate = Product::new(
        "Segundo",
        Sku::parse("SKU-X").expect("sku"),
        None,
        Money::from_cents(100).expect("preço"),
        1,
        category.id(),
        Utc::now(),
    )
    .expect("válido");

    let error = repository
        .insert(&duplicate)
        .await
        .expect_err("SKU duplicado");

    assert!(matches!(error, application::AppError::AlreadyInUse(_)));
}

#[tokio::test]
async fn inserting_product_with_unknown_category_reports_category_not_found() {
    let temp = TempDb::new("prod-fk").await;
    let repository = SqlxProductRepository::new(temp.db.pool().clone());

    let orphan = Product::new(
        "Órfão",
        Sku::parse("ORPHAN-1").expect("sku"),
        None,
        Money::from_cents(100).expect("preço"),
        1,
        UserId::new_v4(),
        Utc::now(),
    )
    .expect("válido");

    let error = repository
        .insert(&orphan)
        .await
        .expect_err("FK deve falhar");

    assert!(
        matches!(error, application::AppError::CategoryNotFound),
        "a violação de FK deve virar CategoryNotFound, não erro interno"
    );
}

#[tokio::test]
async fn hard_delete_of_category_with_products_is_blocked_by_fk() {
    let temp = TempDb::new("prod-restrict").await;
    let category = seed_category(&temp, "Protegida").await;
    seed_product(&temp, "P", "SKU-P", 100, 1, category.id()).await;

    // Mesmo tentando apagar direto no SQL, o banco impede (ON DELETE RESTRICT).
    let attempt = sqlx::query("DELETE FROM categories WHERE id = ?")
        .bind(category.id().to_string())
        .execute(temp.db.pool())
        .await;

    assert!(
        attempt.is_err(),
        "FK RESTRICT deve impedir apagar categoria com produtos"
    );
}

#[tokio::test]
async fn listing_filters_by_category_and_name() {
    let temp = TempDb::new("prod-filter").await;
    let notebooks = seed_category(&temp, "Notebooks").await;
    let perifericos = seed_category(&temp, "Periféricos").await;

    seed_product(&temp, "Notebook Pro", "NB-1", 500_000, 3, notebooks.id()).await;
    seed_product(&temp, "Notebook Air", "NB-2", 300_000, 5, notebooks.id()).await;
    seed_product(&temp, "Mouse", "MS-1", 5_000, 40, perifericos.id()).await;

    let repository = SqlxProductRepository::new(temp.db.pool().clone());

    let by_category = ProductFilter {
        category_id: Some(notebooks.id()),
        ..Default::default()
    };
    let found = repository
        .list(&by_category, ProductSort::Name, 0, 10)
        .await
        .expect("listar");
    assert_eq!(found.len(), 2);
    assert_eq!(repository.count(&by_category).await.expect("contar"), 2);

    let by_name = ProductFilter {
        name_contains: Some("air".to_owned()),
        ..Default::default()
    };
    let found = repository
        .list(&by_name, ProductSort::Name, 0, 10)
        .await
        .expect("listar");
    assert_eq!(found.len(), 1, "a busca deve ser case-insensitive");
    assert_eq!(found[0].name(), "Notebook Air");
}

#[tokio::test]
async fn listing_respects_each_allowlisted_sort() {
    let temp = TempDb::new("prod-sort").await;
    let category = seed_category(&temp, "S").await;

    seed_product(&temp, "Caro", "A-1", 900_000, 1, category.id()).await;
    seed_product(&temp, "Barato", "B-2", 1_000, 1, category.id()).await;

    let repository = SqlxProductRepository::new(temp.db.pool().clone());
    let filter = ProductFilter::default();

    let cheapest = repository
        .list(&filter, ProductSort::Cheapest, 0, 10)
        .await
        .expect("mais barato primeiro");
    assert_eq!(cheapest[0].name(), "Barato");

    let expensive = repository
        .list(&filter, ProductSort::MostExpensive, 0, 10)
        .await
        .expect("mais caro primeiro");
    assert_eq!(expensive[0].name(), "Caro");

    let by_name = repository
        .list(&filter, ProductSort::Name, 0, 10)
        .await
        .expect("nome");
    assert_eq!(by_name[0].name(), "Barato");
}

#[tokio::test]
async fn listing_applies_offset_and_limit() {
    let temp = TempDb::new("prod-page").await;
    let category = seed_category(&temp, "P").await;

    for index in 0..5 {
        seed_product(
            &temp,
            &format!("Produto {index}"),
            &format!("SKU-{index}"),
            1_000 + index,
            1,
            category.id(),
        )
        .await;
    }

    let repository = SqlxProductRepository::new(temp.db.pool().clone());
    let page = repository
        .list(&ProductFilter::default(), ProductSort::Name, 1, 2)
        .await
        .expect("paginar");

    assert_eq!(page.len(), 2);
    assert_eq!(page[0].name(), "Produto 1");
    assert_eq!(page[1].name(), "Produto 2");
}

#[tokio::test]
async fn soft_deleted_product_disappears_from_find_and_list() {
    let temp = TempDb::new("prod-soft-delete").await;
    let category = seed_category(&temp, "SD").await;
    let product = seed_product(&temp, "Some", "GONE-1", 100, 1, category.id()).await;

    let repository = SqlxProductRepository::new(temp.db.pool().clone());
    let mut updated = repository
        .find_by_id(product.id())
        .await
        .expect("buscar")
        .expect("existe");
    updated.soft_delete(Utc::now() + Duration::seconds(1));
    repository.update(&updated).await.expect("soft delete");

    assert!(
        repository
            .find_by_id(product.id())
            .await
            .expect("buscar")
            .is_none()
    );
    assert_eq!(
        repository
            .count(&ProductFilter::default())
            .await
            .expect("contar"),
        0
    );

    // E o SKU volta a estar disponível para um novo produto ativo.
    seed_product(&temp, "Novo", "GONE-1", 100, 1, category.id()).await;
}
