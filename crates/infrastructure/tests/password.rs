//! Testes do adaptador Argon2id contra a porta `PasswordHasher`.
//!
//! Verifica geração com salt aleatório, verificação de senha correta/incorreta
//! e rejeição de hash corrompido.

use application::PasswordHasher;
use infrastructure::Argon2PasswordHasher;

/// Verifica o contrato identificado por `hashes_with_argon2id_and_random_salt` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn hashes_with_argon2id_and_random_salt() {
    let hasher = Argon2PasswordHasher::new();
    let password = "Senha-Forte-2026!";

    let first = hasher.hash(password).await.expect("primeiro hash");
    let second = hasher.hash(password).await.expect("segundo hash");

    assert!(first.as_str().starts_with("$argon2id$"));
    assert_ne!(first, second, "o salt deve variar a cada hash");
    assert_ne!(first.as_str(), password);
    assert!(!first.as_str().contains(password));
}

/// Verifica o contrato identificado por `verifies_correct_password_and_rejects_wrong_one` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn verifies_correct_password_and_rejects_wrong_one() {
    let hasher = Argon2PasswordHasher::new();
    let hash = hasher.hash("Senha-Forte-2026!").await.expect("gerar hash");

    assert!(
        hasher
            .verify("Senha-Forte-2026!", &hash)
            .await
            .expect("verificar senha correta")
    );
    assert!(
        !hasher
            .verify("Outra-Senha-2026!", &hash)
            .await
            .expect("verificar senha incorreta")
    );
}

/// Verifica o contrato identificado por `fails_on_corrupted_hash` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn fails_on_corrupted_hash() {
    let hasher = Argon2PasswordHasher::new();
    let corrupted = domain::PasswordHash::parse("$argon2id$v=19$m=1,t=1,p=1$c2FsdA$aGFzaA")
        .expect("formato válido");

    assert!(
        hasher
            .verify("Senha-Forte-2026!", &corrupted)
            .await
            .is_err()
    );
}
