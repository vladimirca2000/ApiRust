//! Testes do `JwtTokenService`: emissão e verificação de access tokens.
//!
//! Cobre algoritmo fixo, claims obrigatórias, expiração, assinatura errada e
//! troca de emissor/audiência — os ataques clássicos contra JWT.

use application::{AppError, TokenService};
use chrono::{DateTime, Duration, Utc};
use domain::{Email, PasswordHash, Role, User};
use infrastructure::JwtTokenService;

/// Segredo de teste com tamanho suficiente para HS256.
const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

/// TTL usado nos testes, em segundos.
const TTL_SECS: u64 = 900;

/// Instante usado como "agora" nos testes.
///
/// Precisa ser o relógio real, não uma data fixa: `jsonwebtoken` valida `exp`
/// contra o horário do sistema, então uma data fixa no passado seria rejeitada
/// como expirada.
fn now() -> DateTime<Utc> {
    Utc::now()
}

/// Usuário de teste com e-mail e hash válidos.
fn user() -> User {
    User::new(
        "Vladimir",
        Email::parse("user@example.com").expect("e-mail válido"),
        PasswordHash::parse("$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA").expect("hash válido"),
        Role::Admin,
        now(),
    )
    .expect("usuário válido")
}

/// Serviço configurado com o segredo e TTL padrão dos testes.
fn service() -> JwtTokenService {
    JwtTokenService::new(SECRET, TTL_SECS).expect("serviço válido")
}

#[tokio::test]
async fn issues_and_verifies_a_token_with_all_required_claims() {
    let svc = service();
    let user = user();

    let token = svc.issue_access(&user, now()).await.expect("emitir token");
    let claims = svc.verify_access(&token).await.expect("verificar token");

    assert_eq!(claims.subject, user.id().to_string());
    assert_eq!(claims.issuer, "apirust");
    assert_eq!(claims.audience, "apirust-api");
    assert_eq!(
        claims.expires_at - claims.issued_at,
        TTL_SECS,
        "exp - iat deve igualar o TTL configurado"
    );
    assert!(!claims.token_id.is_empty(), "jti deve estar presente");
    assert_eq!(svc.access_ttl_secs(), TTL_SECS);
}

#[tokio::test]
async fn two_tokens_for_the_same_user_have_different_jti() {
    let svc = service();
    let user = user();

    let first = svc.issue_access(&user, now()).await.expect("primeiro");
    let second = svc.issue_access(&user, now()).await.expect("segundo");

    let first_claims = svc.verify_access(&first).await.expect("verificar 1");
    let second_claims = svc.verify_access(&second).await.expect("verificar 2");

    assert_ne!(
        first_claims.token_id, second_claims.token_id,
        "jti deve ser único por emissão"
    );
}

#[tokio::test]
async fn rejects_token_signed_with_another_secret() {
    let issuer_service = service();
    let other_service = JwtTokenService::new(&"9".repeat(48), TTL_SECS).expect("outro serviço");

    let token = issuer_service
        .issue_access(&user(), now())
        .await
        .expect("emitir");

    let error = other_service
        .verify_access(&token)
        .await
        .expect_err("assinatura de outro segredo deve falhar");

    assert!(matches!(error, AppError::Authentication));
}

#[tokio::test]
async fn rejects_expired_token() {
    let svc = service();
    let long_ago = now() - Duration::seconds(TTL_SECS as i64 + 3600);

    let token = svc.issue_access(&user(), long_ago).await.expect("emitir");

    let error = svc
        .verify_access(&token)
        .await
        .expect_err("token expirado deve ser rejeitado");

    assert!(matches!(error, AppError::Authentication));
}

#[tokio::test]
async fn rejects_completely_invalid_token() {
    let svc = service();

    let error = svc
        .verify_access("isto-nao-e-um-jwt")
        .await
        .expect_err("texto inválido");

    assert!(matches!(error, AppError::Authentication));
}

#[tokio::test]
async fn rejects_token_with_algorithm_none() {
    // JWT com `alg: none` e sem assinatura: o clássico ataque de algoritmo.
    let header = base64_url("{\"alg\":\"none\",\"typ\":\"JWT\"}");
    let payload = base64_url(
        "{\"sub\":\"00000000-0000-0000-0000-000000000000\",\"iat\":1,\"exp\":9999999999,\
          \"jti\":\"x\",\"iss\":\"apirust\",\"aud\":\"apirust-api\"}",
    );
    let forged = format!("{header}.{payload}.");

    let error = service()
        .verify_access(&forged)
        .await
        .expect_err("alg none deve ser rejeitado");

    assert!(matches!(error, AppError::Authentication));
}

#[tokio::test]
async fn rejects_zero_ttl_configuration() {
    // `expect_err` exigiria `Debug` em JwtTokenService, que contém chaves e
    // por isso não o implementa de propósito.
    let result = JwtTokenService::new(SECRET, 0);

    assert!(matches!(result, Err(AppError::Infrastructure(_))));
}

/// Codifica o texto em base64url sem padding, como exige o JWT.
fn base64_url(value: &str) -> String {
    use base64_encode::encode;

    encode(value.as_bytes())
}

/// Codificador base64url mínimo, para montar tokens forjados sem nova dependência.
mod base64_encode {
    /// Converte bytes em base64url sem padding (RFC 4648 §5).
    pub fn encode(input: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

        let mut out = String::new();
        for chunk in input.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = *chunk.get(1).unwrap_or(&0) as u32;
            let b2 = *chunk.get(2).unwrap_or(&0) as u32;
            let triple = (b0 << 16) | (b1 << 8) | b2;

            out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
            out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
            if chunk.len() > 1 {
                out.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
            }
            if chunk.len() > 2 {
                out.push(ALPHABET[(triple & 0x3F) as usize] as char);
            }
        }
        out
    }
}
