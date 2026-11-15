//! Testes do schema: migrations, pragmas do pool, constraints e índices.
//!
//! Cada teste usa um SQLite temporário próprio (`TempDb`), aplica as migrations
//! reais e verifica o comportamento no nível do banco, independente do domínio.

mod common;

use infrastructure::Database;

use common::TempDb;

const CREATED_AT: &str = "2026-11-14T00:00:00Z";

async fn insert_user(
    db: &Database,
    id: &str,
    email: &str,
    deleted_at: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO users (id, name, email, password_hash, role, created_at, updated_at, deleted_at)
         VALUES (?, 'Vladimir', ?, 'hash', 'admin', ?, ?, ?)",
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

async fn insert_category(db: &Database, id: &str, name: &str) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO categories (id, name, created_at, updated_at) VALUES (?, ?, ?, ?)")
        .bind(id)
        .bind(name)
        .bind(CREATED_AT)
        .bind(CREATED_AT)
        .execute(db.pool())
        .await?;
    Ok(())
}

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
         VALUES (?, 'Produto', ?, ?, ?, ?, ?, ?)",
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

#[tokio::test]
async fn migrations_are_idempotent() {
    let temp = TempDb::new("idempotent").await;

    temp.db.migrate().await.expect("reaplicar migrations");

    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(temp.db.pool())
        .await
        .expect("contar migrations");
    assert!(applied >= 1);

    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .fetch_all(temp.db.pool())
            .await
            .expect("listar tabelas");

    for expected in ["users", "categories", "products", "refresh_tokens"] {
        assert!(
            tables.iter().any(|table| table == expected),
            "falta tabela {expected}"
        );
    }
}

#[tokio::test]
async fn enables_foreign_keys_and_wal() {
    let temp = TempDb::new("pragmas").await;

    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(temp.db.pool())
        .await
        .expect("pragma foreign_keys");
    assert_eq!(foreign_keys, 1);

    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(temp.db.pool())
        .await
        .expect("pragma journal_mode");
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
}

#[tokio::test]
async fn applies_busy_timeout_and_normal_synchronous() {
    let temp = TempDb::new("pragmas-timeout").await;

    // `busy_timeout` é configurado em 5s (5000 ms) no pool.
    let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
        .fetch_one(temp.db.pool())
        .await
        .expect("pragma busy_timeout");
    assert_eq!(busy_timeout, 5_000);

    // `synchronous` NORMAL = 1 (0=OFF, 1=NORMAL, 2=FULL, 3=EXTRA).
    let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
        .fetch_one(temp.db.pool())
        .await
        .expect("pragma synchronous");
    assert_eq!(synchronous, 1);
}

#[tokio::test]
async fn category_name_is_unique_only_among_active_records() {
    let temp = TempDb::new("category-unique").await;

    insert_category(&temp.db, "c1", "Eletrônicos")
        .await
        .expect("primeira categoria");

    let duplicate = insert_category(&temp.db, "c2", "Eletrônicos").await;
    assert!(duplicate.is_err(), "nome duplicado deveria falhar");

    // Soft delete libera o nome, graças ao índice parcial.
    sqlx::query("UPDATE categories SET deleted_at = ? WHERE id = 'c1'")
        .bind(CREATED_AT)
        .execute(temp.db.pool())
        .await
        .expect("soft delete da categoria");

    insert_category(&temp.db, "c3", "Eletrônicos")
        .await
        .expect("nome pode ser reutilizado após soft delete");
}

#[tokio::test]
async fn email_is_unique_only_among_active_users() {
    let temp = TempDb::new("email").await;

    insert_user(&temp.db, "u1", "user@example.com", None)
        .await
        .expect("primeiro usuário");

    let duplicate = insert_user(&temp.db, "u2", "user@example.com", None).await;
    assert!(duplicate.is_err(), "e-mail duplicado deveria falhar");

    sqlx::query("UPDATE users SET deleted_at = ? WHERE id = 'u1'")
        .bind(CREATED_AT)
        .execute(temp.db.pool())
        .await
        .expect("soft delete");

    insert_user(&temp.db, "u3", "user@example.com", None)
        .await
        .expect("e-mail pode ser reutilizado após soft delete");
}

#[tokio::test]
async fn product_constraints_are_enforced() {
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

#[tokio::test]
async fn refresh_tokens_cascade_and_unique_hash() {
    let temp = TempDb::new("tokens").await;
    insert_user(&temp.db, "u1", "user@example.com", None)
        .await
        .expect("usuário");

    let insert_token = |id: &'static str| {
        let pool = temp.db.pool().clone();
        async move {
            sqlx::query(
                "INSERT INTO refresh_tokens (id, user_id, token_hash, expires_at, created_at)
                 VALUES (?, 'u1', 'hash-1', ?, ?)",
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
