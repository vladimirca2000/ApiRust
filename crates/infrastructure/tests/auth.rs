//! Testes do `SqlxRefreshTokenStore` e da rotação de refresh tokens.
//!
//! Cobre persistência com hash, busca por hash, revogação individual com
//! `replaced_by_id`, revogação em massa e o critério de aceite da Fase 3:
//! detecção de reutilização de refresh token.

mod common;

use application::{
    AuthenticateUser, PasswordHasher, RefreshTokenStore, TokenService, UserRepository,
};
use chrono::{DateTime, Duration, Utc};
use domain::{Email, PasswordHash, RefreshTokenSecret, Role, RotationOutcome, User};
use infrastructure::{
    Argon2PasswordHasher, JwtTokenService, SqlxRefreshTokenStore, SqlxUserRepository,
};

use common::TempDb;

/// Segredo usado para assinar os tokens dos testes.
const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

/// TTL do access token nos testes, em segundos.
const TTL_SECS: u64 = 900;

/// Senha em texto puro do usuário de teste.
const PASSWORD: &str = "Senha-Forte-2026!";

/// Monta um hash Argon2id real para o usuário de teste.
async fn password_hash() -> PasswordHash {
    Argon2PasswordHasher::new()
        .hash(PASSWORD)
        .await
        .expect("gerar hash")
}

/// Cria um usuário ativo no banco e devolve a instância persistida.
async fn seed_user(temp: &TempDb, email: &str) -> User {
    let repository = SqlxUserRepository::new(temp.db.pool().clone());
    let user = User::new(
        "Vladimir",
        Email::parse(email).expect("e-mail válido"),
        password_hash().await,
        Role::Admin,
        Utc::now(),
    )
    .expect("usuário válido");

    repository
        .insert(&user, None)
        .await
        .expect("inserir usuário");
    user
}

/// Estrutura que reúne casos de uso e adaptadores sobre o mesmo banco.
struct Fixture {
    /// Repositório SQLx usado pelo caso de uso de autenticação.
    users: SqlxUserRepository,
    /// Hasher Argon2id usado para criar/verificar credenciais de teste.
    hasher: Argon2PasswordHasher,
    /// Serviço JWT configurado com o segredo exclusivo dos testes.
    tokens: JwtTokenService,
    /// Persistência dos refresh tokens do cenário.
    refresh: SqlxRefreshTokenStore,
}

impl Fixture {
    /// Monta os adaptadores apontando para o pool informado.
    fn new(temp: &TempDb) -> Self {
        Self {
            users: SqlxUserRepository::new(temp.db.pool().clone()),
            hasher: Argon2PasswordHasher::new(),
            tokens: JwtTokenService::new(SECRET, TTL_SECS).expect("serviço de token"),
            refresh: SqlxRefreshTokenStore::new(temp.db.pool().clone()),
        }
    }

    /// Cria o caso de uso de autenticação ligado a estes adaptadores.
    fn authenticate(&self) -> AuthenticateUser<'_> {
        AuthenticateUser::new(&self.users, &self.hasher, &self.tokens, &self.refresh)
    }
}

/// Verifica o contrato identificado por `login_returns_a_verifiable_access_token_and_a_persisted_refresh_hash` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn login_returns_a_verifiable_access_token_and_a_persisted_refresh_hash() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("login").await;
    let fixture = Fixture::new(&temp);
    let user = seed_user(&temp, "user@example.com").await;

    let pair = fixture
        .authenticate()
        .login(
            &Email::parse("user@example.com").unwrap(),
            PASSWORD,
            Utc::now(),
        )
        .await
        .expect("login válido");

    // O access token deve ser verificável pelo mesmo serviço.
    let claims = fixture
        .tokens
        .verify_access(&pair.access_token)
        .await
        .expect("token verificável");
    assert_eq!(claims.subject, user.id().to_string());
    assert_eq!(pair.expires_in_secs, TTL_SECS);

    // O banco guarda o hash, nunca o token em claro.
    let stored: String =
        sqlx::query_scalar("SELECT token_hash FROM refresh_tokens WHERE user_id = $1")
            .bind(user.id().to_string())
            .fetch_one(temp.db.pool())
            .await
            .expect("hash persistido");

    assert_eq!(
        stored,
        application::hash_refresh(pair.refresh_token.expose())
    );
    assert_ne!(
        stored,
        pair.refresh_token.expose(),
        "não pode guardar o token em claro"
    );
    assert_eq!(stored.len(), 64, "SHA-256 em hex tem 64 caracteres");
}

/// Verifica o contrato identificado por `login_rejects_wrong_password_without_revealing_cause` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn login_rejects_wrong_password_without_revealing_cause() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("login-wrong").await;
    let fixture = Fixture::new(&temp);
    seed_user(&temp, "user@example.com").await;

    let error = fixture
        .authenticate()
        .login(
            &Email::parse("user@example.com").unwrap(),
            "Senha-Errada-2026!",
            Utc::now(),
        )
        .await
        .expect_err("senha errada");

    assert!(matches!(error, application::AppError::Authentication));
}

/// Verifica o contrato identificado por `login_rejects_unknown_email_with_the_same_error_as_wrong_password` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn login_rejects_unknown_email_with_the_same_error_as_wrong_password() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("login-unknown").await;
    let fixture = Fixture::new(&temp);

    let unknown = fixture
        .authenticate()
        .login(
            &Email::parse("ninguem@example.com").unwrap(),
            PASSWORD,
            Utc::now(),
        )
        .await
        .expect_err("e-mail inexistente");

    assert!(
        matches!(unknown, application::AppError::Authentication),
        "e-mail inexistente não pode ter erro diferente de senha errada"
    );
}

/// Verifica o contrato identificado por `login_rejects_inactive_user` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn login_rejects_inactive_user() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("login-inactive").await;
    let fixture = Fixture::new(&temp);
    let user = seed_user(&temp, "user@example.com").await;

    sqlx::query("UPDATE users SET status = 'inactive' WHERE id = $1")
        .bind(user.id().to_string())
        .execute(temp.db.pool())
        .await
        .expect("desativar usuário");

    let error = fixture
        .authenticate()
        .login(
            &Email::parse("user@example.com").unwrap(),
            PASSWORD,
            Utc::now(),
        )
        .await
        .expect_err("usuário inativo");

    assert!(matches!(error, application::AppError::Authentication));
}

/// Verifica o contrato identificado por `refresh_rotates_the_token_and_revokes_the_previous_one` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn refresh_rotates_the_token_and_revokes_the_previous_one() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("refresh-rotate").await;
    let fixture = Fixture::new(&temp);
    seed_user(&temp, "user@example.com").await;
    let use_case = fixture.authenticate();

    let pair = use_case
        .login(
            &Email::parse("user@example.com").unwrap(),
            PASSWORD,
            Utc::now(),
        )
        .await
        .expect("login");

    let old_hash = application::hash_refresh(pair.refresh_token.expose());
    let now = Utc::now();

    let outcome = use_case
        .refresh(&pair.refresh_token, &old_hash, now)
        .await
        .expect("rotacionar");

    let new_pair = match outcome {
        RotationOutcome::Rotated { pair, .. } => pair,
        other => panic!("esperado Rotated, recebido {other:?}"),
    };

    assert_ne!(
        new_pair.refresh_token.expose(),
        pair.refresh_token.expose(),
        "o refresh novo deve ser diferente"
    );

    // O token antigo fica revogado e aponta para o sucessor.
    let (revoked_at, replaced_by): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT revoked_at, replaced_by_id FROM refresh_tokens WHERE token_hash = $1",
    )
    .bind(&old_hash)
    .fetch_one(temp.db.pool())
    .await
    .expect("registro antigo");

    assert!(revoked_at.is_some(), "token antigo deve estar revogado");
    assert!(replaced_by.is_some(), "deve registrar o sucessor");
}

/// Verifica o contrato identificado por `reusing_a_rotated_refresh_token_revokes_the_whole_family` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn reusing_a_rotated_refresh_token_revokes_the_whole_family() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("refresh-reuse").await;
    let fixture = Fixture::new(&temp);
    let user = seed_user(&temp, "user@example.com").await;
    let use_case = fixture.authenticate();

    let pair = use_case
        .login(
            &Email::parse("user@example.com").unwrap(),
            PASSWORD,
            Utc::now(),
        )
        .await
        .expect("login");

    let old_hash = application::hash_refresh(pair.refresh_token.expose());

    // Primeiro uso: rotaciona normalmente.
    let rotated = use_case
        .refresh(&pair.refresh_token, &old_hash, Utc::now())
        .await
        .expect("primeira rotação");
    let new_pair = match rotated {
        RotationOutcome::Rotated { pair, .. } => pair,
        other => panic!("esperado Rotated, recebido {other:?}"),
    };

    // Segundo uso do token antigo: reuso detectado.
    let reused = use_case
        .refresh(&pair.refresh_token, &old_hash, Utc::now())
        .await
        .expect("detectar reuso");

    assert!(matches!(reused, RotationOutcome::Reused));

    // Toda a família deve estar revogada — inclusive o token ainda válido.
    let alive: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM refresh_tokens WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user.id().to_string())
    .fetch_one(temp.db.pool())
    .await
    .expect("contar vivos");

    assert_eq!(alive, 0, "reuso deve revogar todos os tokens do usuário");

    // O token que estava válido também não pode mais ser usado.
    let after_reuse = use_case
        .refresh(
            &new_pair.refresh_token,
            &application::hash_refresh(new_pair.refresh_token.expose()),
            Utc::now(),
        )
        .await
        .expect("consultar token novo");

    assert!(matches!(
        after_reuse,
        RotationOutcome::Reused | RotationOutcome::Invalid
    ));
}

/// Verifica o contrato identificado por `refresh_rejects_expired_token` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn refresh_rejects_expired_token() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("refresh-expired").await;
    let fixture = Fixture::new(&temp);
    let user = seed_user(&temp, "user@example.com").await;
    let use_case = fixture.authenticate();

    let secret = application::random_hex_token();
    let hash = application::hash_refresh(&secret);
    let past = Utc::now() - Duration::days(1);

    fixture
        .refresh
        .insert(
            &hash,
            &user.id().to_string(),
            past,
            past - Duration::days(30),
        )
        .await
        .expect("inserir expirado");

    let outcome = use_case
        .refresh(&RefreshTokenSecret::new(secret), &hash, Utc::now())
        .await
        .expect("consultar expirado");

    assert!(matches!(outcome, RotationOutcome::Invalid));
}

/// Verifica o contrato identificado por `refresh_rejects_unknown_token` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn refresh_rejects_unknown_token() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("refresh-unknown").await;
    let fixture = Fixture::new(&temp);
    let use_case = fixture.authenticate();

    let unknown = RefreshTokenSecret::new("token-que-nunca-existiu");
    let outcome = use_case
        .refresh(
            &unknown,
            &application::hash_refresh(unknown.expose()),
            Utc::now(),
        )
        .await
        .expect("consultar desconhecido");

    assert!(matches!(outcome, RotationOutcome::Invalid));
}

/// Verifica o contrato identificado por `logout_revokes_the_presented_token_and_is_idempotent` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn logout_revokes_the_presented_token_and_is_idempotent() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("logout").await;
    let fixture = Fixture::new(&temp);
    seed_user(&temp, "user@example.com").await;
    let use_case = fixture.authenticate();

    let pair = use_case
        .login(
            &Email::parse("user@example.com").unwrap(),
            PASSWORD,
            Utc::now(),
        )
        .await
        .expect("login");
    let hash = application::hash_refresh(pair.refresh_token.expose());

    use_case.logout(&hash, Utc::now()).await.expect("logout");
    use_case
        .logout(&hash, Utc::now())
        .await
        .expect("logout repetido");

    let revoked: Option<String> =
        sqlx::query_scalar("SELECT revoked_at FROM refresh_tokens WHERE token_hash = $1")
            .bind(&hash)
            .fetch_one(temp.db.pool())
            .await
            .expect("registro");

    assert!(revoked.is_some(), "logout deve revogar o token");

    // Token revogado não renova mais.
    let outcome = use_case
        .refresh(&pair.refresh_token, &hash, Utc::now())
        .await
        .expect("usar token revogado");
    assert!(matches!(outcome, RotationOutcome::Reused));
}

/// Verifica o contrato identificado por `revoke_all_for_user_only_affects_that_user` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn revoke_all_for_user_only_affects_that_user() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("revoke-all").await;
    let fixture = Fixture::new(&temp);
    let first = seed_user(&temp, "a@example.com").await;
    let second = seed_user(&temp, "b@example.com").await;

    let now = Utc::now();
    let future = now + Duration::days(30);
    let hash_a = application::hash_refresh(&application::random_hex_token());
    let hash_b = application::hash_refresh(&application::random_hex_token());

    fixture
        .refresh
        .insert(&hash_a, &first.id().to_string(), future, now)
        .await
        .expect("token do primeiro");
    fixture
        .refresh
        .insert(&hash_b, &second.id().to_string(), future, now)
        .await
        .expect("token do segundo");

    let revoked = fixture
        .refresh
        .revoke_all_for_user(&first.id().to_string(), now)
        .await
        .expect("revogar em massa");
    assert_eq!(revoked, 1);

    let second_alive: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM refresh_tokens WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(second.id().to_string())
    .fetch_one(temp.db.pool())
    .await
    .expect("contar do segundo");

    assert_eq!(second_alive, 1, "o outro usuário não pode ser afetado");
}

/// Garante que o timestamp persistido é comparável como texto.
#[tokio::test]
async fn persisted_timestamps_round_trip() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = TempDb::new("refresh-ts").await;
    let fixture = Fixture::new(&temp);
    let user = seed_user(&temp, "user@example.com").await;

    let now: DateTime<Utc> = Utc::now();
    let expires = now + Duration::days(7);
    let hash = application::hash_refresh(&application::random_hex_token());

    fixture
        .refresh
        .insert(&hash, &user.id().to_string(), expires, now)
        .await
        .expect("inserir");

    let record = fixture
        .refresh
        .find_by_hash(&hash)
        .await
        .expect("buscar")
        .expect("registro existe");

    assert_eq!(
        record.expires_at.timestamp(),
        expires.timestamp(),
        "a expiração deve sobreviver ao round-trip"
    );
    assert!(record.revoked_at.is_none());
}
