//! Integração do provisionamento do Admin inicial contra SQLite real.
//!
//! Cobre o critério de aceite da Fase 2: o primeiro boot cria o Admin, o
//! segundo registra `AlreadyExists` e **não altera a linha** persistida.

mod common;

use application::{EnsureAdmin, PasswordHasher, SeedOutcome, UserRepository};
use chrono::{DateTime, Utc};
use domain::{DomainError, Email, Role};
use infrastructure::{Argon2PasswordHasher, SqlxUserRepository};

use common::TempDb;

const ADMIN_EMAIL: &str = "admin@example.com";
const ADMIN_PASSWORD: &str = "Senha-Forte-2026!";

fn email() -> Email {
    Email::parse(ADMIN_EMAIL).expect("e-mail válido")
}

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-14T12:00:00Z")
        .expect("data válida")
        .with_timezone(&Utc)
}

async fn count_users(db: &infrastructure::Database) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM users WHERE deleted_at IS NULL")
        .fetch_one(db.pool())
        .await
        .expect("contar usuários")
}

#[tokio::test]
async fn first_boot_creates_admin_with_argon2id_hash() {
    let temp = TempDb::new("seed-create").await;
    let repository = SqlxUserRepository::new(temp.db.pool().clone());
    let hasher = Argon2PasswordHasher::new();

    let outcome = EnsureAdmin::new(&repository, &hasher)
        .execute(&email(), ADMIN_PASSWORD, now())
        .await
        .expect("criar admin");

    assert!(matches!(outcome, SeedOutcome::Created(_)));
    assert_eq!(count_users(&temp.db).await, 1);

    let stored = repository
        .find_by_email(&email())
        .await
        .expect("consultar")
        .expect("admin persistido");

    assert_eq!(stored.role(), Role::Admin);
    assert!(stored.is_active());

    // A senha em texto puro nunca chega ao banco, apenas o hash Argon2id.
    let raw_hash = stored.password_hash().as_str();
    assert!(raw_hash.starts_with("$argon2id$"), "hash não é Argon2id");
    assert!(
        !raw_hash.contains(ADMIN_PASSWORD),
        "hash não pode conter a senha em texto puro"
    );
    assert!(
        hasher
            .verify(ADMIN_PASSWORD, stored.password_hash())
            .await
            .expect("verificar senha"),
        "o hash persistido deve validar a senha informada"
    );
}

#[tokio::test]
async fn second_boot_is_a_no_op_and_keeps_the_same_row() {
    let temp = TempDb::new("seed-idempotent").await;
    let repository = SqlxUserRepository::new(temp.db.pool().clone());
    let hasher = Argon2PasswordHasher::new();
    let use_case = EnsureAdmin::new(&repository, &hasher);

    let first = use_case
        .execute(&email(), ADMIN_PASSWORD, now())
        .await
        .expect("primeiro boot");

    let created_id = match first {
        SeedOutcome::Created(id) => id,
        other => panic!("esperado Created, recebido {other:?}"),
    };

    let before = repository
        .find_by_id(created_id)
        .await
        .expect("consultar")
        .expect("admin existe");

    // Segundo boot muito depois: se a rotina regravasse a linha, updated_at
    // mudaria; se regeresse o hash, o valor seria diferente (salt aleatório).
    let later = now() + chrono::Duration::hours(24);
    let second = use_case
        .execute(&email(), "Outra-Senha-Forte-2026!", later)
        .await
        .expect("segundo boot");

    assert_eq!(second, SeedOutcome::AlreadyExists(created_id));
    assert_eq!(count_users(&temp.db).await, 1, "não pode duplicar");

    let after = repository
        .find_by_id(created_id)
        .await
        .expect("consultar")
        .expect("admin existe");

    assert_eq!(after.id(), before.id());
    assert_eq!(
        after.password_hash(),
        before.password_hash(),
        "o hash não pode ser regravado no segundo boot"
    );
    assert_eq!(
        after.updated_at(),
        before.updated_at(),
        "updated_at não pode ser tocado no segundo boot"
    );
    assert_eq!(after.created_at(), before.created_at());

    // A senha original continua válida; a do segundo boot foi ignorada.
    assert!(
        hasher
            .verify(ADMIN_PASSWORD, after.password_hash())
            .await
            .expect("verificar senha original")
    );
    assert!(
        !hasher
            .verify("Outra-Senha-Forte-2026!", after.password_hash())
            .await
            .expect("verificar senha do segundo boot")
    );
}

#[tokio::test]
async fn weak_password_is_rejected_before_touching_the_database() {
    let temp = TempDb::new("seed-weak").await;
    let repository = SqlxUserRepository::new(temp.db.pool().clone());
    let hasher = Argon2PasswordHasher::new();

    let error = EnsureAdmin::new(&repository, &hasher)
        .execute(&email(), "fraca", now())
        .await
        .expect_err("senha fraca");

    assert!(matches!(
        error,
        application::AppError::Domain(DomainError::InvalidPassword(_))
    ));
    assert_eq!(count_users(&temp.db).await, 0);
}

#[tokio::test]
async fn admin_can_be_reprovisioned_after_soft_delete() {
    let temp = TempDb::new("seed-soft-delete").await;
    let repository = SqlxUserRepository::new(temp.db.pool().clone());
    let hasher = Argon2PasswordHasher::new();
    let use_case = EnsureAdmin::new(&repository, &hasher);

    let first = use_case
        .execute(&email(), ADMIN_PASSWORD, now())
        .await
        .expect("primeiro admin");

    let first_id = match first {
        SeedOutcome::Created(id) => id,
        other => panic!("esperado Created, recebido {other:?}"),
    };

    sqlx::query("UPDATE users SET deleted_at = ?, status = 'inactive' WHERE id = ?")
        .bind("2026-11-15T12:00:00Z")
        .bind(first_id.to_string())
        .execute(temp.db.pool())
        .await
        .expect("soft delete do admin");

    // O índice parcial libera o e-mail, então o seed volta a criar.
    let second = use_case
        .execute(&email(), ADMIN_PASSWORD, now())
        .await
        .expect("reprovisionar admin");

    let new_id = match second {
        SeedOutcome::Created(id) => id,
        other => panic!("esperado Created, recebido {other:?}"),
    };

    assert_ne!(new_id, first_id, "deve criar um novo registro");
    assert_eq!(count_users(&temp.db).await, 1, "apenas o novo fica ativo");
}
