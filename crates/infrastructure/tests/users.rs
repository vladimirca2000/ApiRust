//! Testes do `SqlxUserRepository` contra SQLite real e migrado.
//!
//! Cobre busca por id/e-mail, rejeição de e-mail duplicado entre ativos,
//! ocultação de registros com soft delete e rejeição de linha inválida
//! persistida.

mod common;

use application::{AppError, UserRepository};
use chrono::{DateTime, Utc};
use domain::{Email, EntityStatus, PasswordHash, Role, User, UserId};
use infrastructure::SqlxUserRepository;

use common::TempDb;

/// Hash Argon2id fixo para construir usuários sem executar hashing em cada teste.
const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdGUtZGV0ZXN0ZQ$aGFzaC1kZS10ZXN0ZQ";
/// Retorna um instante UTC fixo para tornar as verificações determinísticas.
fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-14T12:00:00Z")
        .expect("data válida")
        .with_timezone(&Utc)
}
/// Cria um e-mail válido e fixo para os cenários de teste; retorna o value object correspondente.
fn email(value: &str) -> Email {
    Email::parse(value).expect("e-mail válido")
}
/// Cria um hash Argon2id de fixture para os cenários de teste e retorna `PasswordHash`.
fn password_hash() -> PasswordHash {
    PasswordHash::parse(HASH).expect("hash válido")
}
/// Monta e retorna um usuário válido de fixture com os dados informados pelo cenário.
fn user(email_value: &str) -> User {
    User::new(
        "Vladimir",
        email(email_value),
        password_hash(),
        Role::Admin,
        now(),
    )
    .expect("usuário válido")
}

/// Verifica o contrato identificado por `inserts_and_finds_user_by_email_and_id` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn inserts_and_finds_user_by_email_and_id() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("users-find").await;
    let repository = SqlxUserRepository::new(temp.db.pool().clone());
    let admin = user("admin@example.com");

    repository
        .insert(&admin, None)
        .await
        .expect("inserir usuário");

    let by_id = repository
        .find_by_id(admin.id())
        .await
        .expect("consultar por id")
        .expect("usuário encontrado");
    let by_email = repository
        .find_by_email(admin.email())
        .await
        .expect("consultar por e-mail")
        .expect("usuário encontrado");

    assert_eq!(by_id.id(), admin.id());
    assert_eq!(by_email.id(), admin.id());
    assert_eq!(by_email.name(), "Vladimir");
    assert_eq!(by_email.email(), admin.email());
    assert_eq!(by_email.password_hash(), &password_hash());
    assert_eq!(by_email.role(), Role::Admin);
    assert_eq!(by_email.status(), EntityStatus::Active);
    assert_eq!(by_email.created_at(), now());
    assert_eq!(by_email.updated_at(), now());
    assert!(by_email.deleted_at().is_none());
}

/// Verifica o contrato identificado por `returns_none_for_unknown_user` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn returns_none_for_unknown_user() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("users-missing").await;
    let repository = SqlxUserRepository::new(temp.db.pool().clone());

    assert!(
        repository
            .find_by_id(UserId::new_v4())
            .await
            .expect("consultar por id")
            .is_none()
    );
    assert!(
        repository
            .find_by_email(&email("ninguem@example.com"))
            .await
            .expect("consultar por e-mail")
            .is_none()
    );
}

/// Verifica o contrato identificado por `rejects_duplicate_active_email` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn rejects_duplicate_active_email() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("users-duplicate").await;
    let repository = SqlxUserRepository::new(temp.db.pool().clone());

    repository
        .insert(&user("admin@example.com"), None)
        .await
        .expect("primeiro usuário");

    let error = repository
        .insert(&user("admin@example.com"), None)
        .await
        .expect_err("e-mail duplicado");

    assert!(matches!(error, AppError::EmailAlreadyInUse));
}

/// Verifica o contrato identificado por `hides_soft_deleted_users_and_allows_email_reuse` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn hides_soft_deleted_users_and_allows_email_reuse() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("users-soft-delete").await;
    let repository = SqlxUserRepository::new(temp.db.pool().clone());
    let admin = user("admin@example.com");

    repository
        .insert(&admin, None)
        .await
        .expect("inserir usuário");

    sqlx::query("UPDATE users SET deleted_at = $1, status = 'inactive' WHERE id = $2")
        .bind("2026-11-15T12:00:00Z")
        .bind(admin.id().to_string())
        .execute(temp.db.pool())
        .await
        .expect("soft delete");

    assert!(
        repository
            .find_by_id(admin.id())
            .await
            .expect("consultar por id")
            .is_none()
    );
    assert!(
        repository
            .find_by_email(&email("admin@example.com"))
            .await
            .expect("consultar por e-mail")
            .is_none()
    );

    repository
        .insert(&user("admin@example.com"), None)
        .await
        .expect("e-mail pode ser reutilizado após soft delete");
}

/// Verifica o contrato identificado por `rejects_invalid_data_already_stored` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn rejects_invalid_data_already_stored() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("users-invalid-row").await;
    let id = UserId::new_v4();

    sqlx::query(
        "INSERT INTO users (id, name, email, password_hash, role, status, created_at, updated_at) \
         VALUES ($1, 'Vladimir', 'admin@example.com', 'texto-puro', 'admin', 'active', $2, $3)",
    )
    .bind(id.to_string())
    .bind("2026-11-14T12:00:00Z")
    .bind("2026-11-14T12:00:00Z")
    .execute(temp.db.pool())
    .await
    .expect("linha inválida para o domínio");

    let repository = SqlxUserRepository::new(temp.db.pool().clone());
    let error = repository.find_by_id(id).await.expect_err("hash inválido");

    assert!(matches!(
        error,
        AppError::Domain(domain::DomainError::InvalidPasswordHash(_))
    ));
}
