//! Adaptador de hashing de senhas usando Argon2id.
//!
//! Implementa a porta da aplicação e converte falhas da biblioteca criptográfica
//! em erros tipados sem registrar senhas ou hashes.

use application::{AppError, PasswordHasher};
use argon2::password_hash::{PasswordHash as PhcHash, SaltString, rand_core::OsRng};
use argon2::{Argon2, PasswordHasher as _, PasswordVerifier as _};
use async_trait::async_trait;
use domain::PasswordHash;

/// Adaptador de hashing baseado em Argon2id.
///
/// Usa os parâmetros padrão do crate `argon2` (Argon2id v19, m=19456 KiB,
/// t=2, p=1) e executa o trabalho em `spawn_blocking` para não bloquear o
/// runtime assíncrono.
#[derive(Debug, Clone, Copy, Default)]
pub struct Argon2PasswordHasher;

impl Argon2PasswordHasher {
    /// Cria o adaptador sem estado; todas as operações derivam o salt na hora.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PasswordHasher for Argon2PasswordHasher {
    /// Gera um hash Argon2id com salt aleatório para a senha recebida; retorna
    /// `PasswordHash` ou erro se o processamento criptográfico falhar.
    async fn hash(&self, password: &str) -> Result<PasswordHash, AppError> {
        let password = password.to_owned();

        tokio::task::spawn_blocking(move || hash_password(&password))
            .await
            .map_err(AppError::infrastructure)?
    }
    /// Compara a senha com o hash Argon2id sem expor o segredo; retorna `true` se coincidir, `false` se não coincidir ou erro de verificação.
    async fn verify(&self, password: &str, hash: &PasswordHash) -> Result<bool, AppError> {
        let password = password.to_owned();
        let hash = hash.as_str().to_owned();

        tokio::task::spawn_blocking(move || verify_password(&password, &hash))
            .await
            .map_err(AppError::infrastructure)?
    }
}

/// Calcula o hash Argon2id de uma senha, com salt aleatório.
///
/// Executa em thread de bloqueio (chamado via `spawn_blocking`) porque Argon2 é
/// intencionalmente custoso em CPU.
///
/// # Errors
///
/// Retorna [`AppError::Infrastructure`] se o hashing falhar ou o resultado não
/// for um PHC válido.
fn hash_password(password: &str) -> Result<PasswordHash, AppError> {
    let salt = SaltString::generate(&mut OsRng);

    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(AppError::infrastructure)?;

    PasswordHash::parse(hash.to_string()).map_err(AppError::from)
}

/// Confere uma senha em texto puro contra um hash PHC persistido.
///
/// Senha incorreta é resposta normal (`Ok(false)`), não erro; apenas hash
/// malformado ou falha do algoritmo viram `Err`.
///
/// # Errors
///
/// Retorna [`AppError::Infrastructure`] quando o hash não é PHC válido ou o
/// algoritmo falha por motivo diferente de senha errada.
fn verify_password(password: &str, hash: &str) -> Result<bool, AppError> {
    let parsed = PhcHash::new(hash).map_err(AppError::infrastructure)?;

    match Argon2::default().verify_password(password.as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        // Senha incorreta não é falha de infraestrutura.
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(error) => Err(AppError::infrastructure(error)),
    }
}
