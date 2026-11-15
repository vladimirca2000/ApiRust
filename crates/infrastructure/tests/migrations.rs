//! Testes do schema: migrations, isolamento por schema, constraints e índices.
//!
//! Cada teste recebe um schema PostgreSQL próprio (`TempDb`), aplica as
//! migrations reais e verifica o comportamento no nível do banco, independente
//! do domínio.

mod common;

use common::TempDb;
use infrastructure::Database;

/// Instante fixo usado nas linhas de teste.
const CREATED_AT: &str = "2026-11-14T00:00:00Z";

/// Insere um usuário cru, sem passar pelo domínio.
///
/// # Errors
///
/// Retorna o erro do banco para que o teste decida se a violação era esperada.
async fn insert_user(
    db: &Database,
    id: &str,
    email: &str,
    deleted_at: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO users (id, name, email, password_hash, role, created_at, updated_at, deleted_at)
         VALUES ($1, 'Vladimir', $2, 'hash', 'admin', $3, $4, $5)",
    )
    .bind(id)
    .bind(email)
    .bind(CREATED_AT)
    .bind(CREATED_AT)
    .bind(deleted_at)
    .execute(db.pool())
    .await?;
    Ok(())
}

/// Insere uma categoria crua.
///
/// # Errors
///
/// Retorna o erro do banco para que o teste decida se a violação era esperada.
async fn insert_category(db: &Database, id: &str, name: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO categories (id, name, created_at, updated_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(name)
    .bind(CREATED_AT)
    .bind(CREATED_AT)
    .execute(db.pool())
    .await?;
    Ok(())
}

/// Insere um produto cru.
///
/// # Errors
///
/// Retorna o erro do banco para que o teste decida se a violação era esperada.
async fn insert_product(
    db: &Database,
    id: &str,
    sku: &str,
    price_cents: i64,
    stock: i64,
    category_id: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO products (id, name, sku, price_cents, stock, category_id, created_at, updated_at)
         VALUES ($1, 'Produto', $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(sku)
    .bind(price_cents)
    .bind(stock)
    .bind(category_id)
    .bind(CREATED_AT)
    .bind(CREATED_AT)
    .execute(db.pool())
    .await?;
    Ok(())
}

/// Verifica o contrato identificado por `migrations_are_idempotent` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn migrations_are_idempotent() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("idempotent").await;

    temp.db.migrate().await.expect("reaplicar migrations");

    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(temp.db.pool())
        .await
        .expect("contar migrations");
    assert!(applied >= 2, "as duas migrations devem ter sido aplicadas");

    // `information_schema` lista apenas as tabelas do `search_path`: o schema
    // isolado, não o `public` de outras execuções.
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = current_schema() ORDER BY table_name",
    )
    .fetch_all(temp.db.pool())
    .await
    .expect("listar tabelas");

    for expected in [
        "audit_log",
        "categories",
        "price_changes",
        "products",
        "refresh_tokens",
        "stock_movements",
        "users",
    ] {
        assert!(
            tables.iter().any(|table| table == expected),
            "falta tabela {expected}; encontradas: {tables:?}"
        );
    }
}

/// Verifica o contrato identificado por `search_path_points_to_the_isolated_schema` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn search_path_points_to_the_isolated_schema() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("search-path").await;

    let schema: String = sqlx::query_scalar("SELECT current_schema()")
        .fetch_one(temp.db.pool())
        .await
        .expect("schema corrente");

    assert_eq!(
        Some(schema.as_str()),
        temp.db.schema(),
        "o pool precisa apontar para o schema da instância"
    );
    assert_ne!(schema, "public", "testes não podem escrever no public");
}

/// Verifica o contrato identificado por `two_instances_do_not_see_each_other` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn two_instances_do_not_see_each_other() {
    let first = TempDb::new("isolado-a").await;
    let second = TempDb::new("isolado-b").await;

    insert_user(&first.db, "u1", "unico@example.com", None)
        .await
        .expect("inserir no primeiro schema");

    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(second.db.pool())
        .await
        .expect("contar no segundo schema");

    assert_eq!(visible, 0, "schemas distintos não compartilham dados");
}

/// Verifica o contrato identificado por `category_name_is_unique_only_among_active_records` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn category_name_is_unique_only_among_active_records() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("category-unique").await;

    insert_category(&temp.db, "c1", "Eletrônicos")
        .await
        .expect("primeira categoria");

    let duplicate = insert_category(&temp.db, "c2", "Eletrônicos").await;
    assert!(duplicate.is_err(), "nome duplicado deveria falhar");

    // Soft delete libera o nome, graças ao índice parcial.
    sqlx::query("UPDATE categories SET deleted_at = $1 WHERE id = 'c1'")
        .bind(CREATED_AT)
        .execute(temp.db.pool())
        .await
        .expect("soft delete da categoria");

    insert_category(&temp.db, "c3", "Eletrônicos")
        .await
        .expect("nome pode ser reutilizado após soft delete");
}

/// Verifica o contrato identificado por `email_is_unique_only_among_active_users` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn email_is_unique_only_among_active_users() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("email").await;

    insert_user(&temp.db, "u1", "user@example.com", None)
        .await
        .expect("primeiro usuário");

    let duplicate = insert_user(&temp.db, "u2", "user@example.com", None).await;
    assert!(duplicate.is_err(), "e-mail duplicado deveria falhar");

    sqlx::query("UPDATE users SET deleted_at = $1 WHERE id = 'u1'")
        .bind(CREATED_AT)
        .execute(temp.db.pool())
        .await
        .expect("soft delete");

    insert_user(&temp.db, "u3", "user@example.com", None)
        .await
        .expect("e-mail pode ser reutilizado após soft delete");
}

/// Verifica o contrato identificado por `product_constraints_are_enforced` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn product_constraints_are_enforced() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("product").await;
    insert_category(&temp.db, "c1", "Eletrônicos")
        .await
        .expect("categoria");

    insert_product(&temp.db, "p1", "SKU-1", 1000, 10, "c1")
        .await
        .expect("produto válido");

    assert!(
        insert_product(&temp.db, "p2", "SKU-1", 1000, 10, "c1")
            .await
            .is_err(),
        "SKU duplicado deveria falhar"
    );
    assert!(
        insert_product(&temp.db, "p3", "SKU-3", 1000, -1, "c1")
            .await
            .is_err(),
        "estoque negativo deveria falhar"
    );
    assert!(
        insert_product(&temp.db, "p4", "SKU-4", 0, 10, "c1")
            .await
            .is_err(),
        "preço zero deveria falhar"
    );
    assert!(
        insert_product(&temp.db, "p5", "SKU-5", 1000, 10, "inexistente")
            .await
            .is_err(),
        "categoria inexistente deveria violar a FK"
    );

    let delete_category = sqlx::query("DELETE FROM categories WHERE id = 'c1'")
        .execute(temp.db.pool())
        .await;
    assert!(
        delete_category.is_err(),
        "categoria com produtos não pode ser removida"
    );
}

/// Verifica o contrato identificado por `refresh_tokens_cascade_and_unique_hash` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn refresh_tokens_cascade_and_unique_hash() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("tokens").await;
    insert_user(&temp.db, "u1", "user@example.com", None)
        .await
        .expect("usuário");

    let insert_token = |id: &'static str| {
        let pool = temp.db.pool().clone();
        async move {
            sqlx::query(
                "INSERT INTO refresh_tokens (id, user_id, token_hash, expires_at, created_at)
                 VALUES ($1, 'u1', 'hash-1', $2, $3)",
            )
            .bind(id)
            .bind(CREATED_AT)
            .bind(CREATED_AT)
            .execute(&pool)
            .await
        }
    };

    insert_token("t1").await.expect("primeiro token");
    assert!(
        insert_token("t2").await.is_err(),
        "token_hash duplicado deveria falhar"
    );

    sqlx::query("DELETE FROM users WHERE id = 'u1'")
        .execute(temp.db.pool())
        .await
        .expect("remover usuário");

    let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM refresh_tokens")
        .fetch_one(temp.db.pool())
        .await
        .expect("contar tokens");
    assert_eq!(tokens, 0, "tokens devem ser removidos em cascata");
}

/// Verifica o contrato identificado por `stock_movement_rejects_zero_delta_at_the_database_level` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn stock_movement_rejects_zero_delta_at_the_database_level() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("movement").await;
    insert_category(&temp.db, "c1", "Eletrônicos")
        .await
        .expect("categoria");
    insert_product(&temp.db, "p1", "SKU-1", 1000, 10, "c1")
        .await
        .expect("produto");

    let result = sqlx::query(
        "INSERT INTO stock_movements (id, product_id, delta, reason, at) \
         VALUES ('m1', 'p1', 0, 'correction', $1)",
    )
    .bind(CREATED_AT)
    .execute(temp.db.pool())
    .await;

    assert!(
        result.is_err(),
        "o CHECK de `delta <> 0` precisa valer também no PostgreSQL"
    );
}
