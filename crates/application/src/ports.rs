use async_trait::async_trait;
use chrono::{DateTime, Utc};
use domain::{AccessClaims, Email, PasswordHash, User, UserId};

use crate::error::AppError;

/// Porta de hashing de senhas.
///
/// A implementação deve usar um algoritmo resistente a força bruta (Argon2id)
/// e nunca expor a senha em texto puro.
#[async_trait]
pub trait PasswordHasher: Send + Sync {
    /// Gera o hash de uma senha em texto puro.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se o hashing falhar.
    async fn hash(&self, password: &str) -> Result<PasswordHash, AppError>;

    /// Confere uma senha em texto puro contra um hash persistido.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se o hash for inválido ou o
    /// algoritmo falhar. Senha incorreta resulta em `Ok(false)`.
    async fn verify(&self, password: &str, hash: &PasswordHash) -> Result<bool, AppError>;
}

/// Porta de persistência de usuários.
///
/// Toda implementação deve ignorar registros com soft delete (`deleted_at`).
#[async_trait]
pub trait UserRepository: Send + Sync {
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a consulta falhar.
    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, AppError>;

    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a consulta falhar.
    async fn find_by_email(&self, email: &Email) -> Result<Option<User>, AppError>;

    /// Persiste um usuário novo.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::EmailAlreadyInUse`] quando já existe um usuário
    /// ativo com o mesmo e-mail e [`AppError::Infrastructure`] para as demais
    /// falhas.
    async fn insert(&self, user: &User) -> Result<(), AppError>;

    /// Lista usuários vivos, com paginação por offset.
    ///
    /// Implementações devem excluir registros com soft delete e ordenar de
    /// forma estável (por `created_at` e depois `id`) para que a paginação não
    /// repita nem pule linhas.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a consulta falhar.
    async fn list(&self, offset: i64, limit: i64) -> Result<Vec<User>, AppError>;

    /// Conta os usuários vivos, para calcular o total de páginas.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a consulta falhar.
    async fn count(&self) -> Result<i64, AppError>;

    /// Persiste alterações em um usuário existente.
    ///
    /// Grava nome, e-mail, hash de senha, papel, estado e timestamps do valor
    /// recebido. A linha é localizada pelo `id` do próprio usuário.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::EmailAlreadyInUse`] quando o novo e-mail colide com
    /// outro registro ativo, [`AppError::UserNotFound`] quando a linha não
    /// existe e [`AppError::Infrastructure`] nas demais falhas.
    async fn update(&self, user: &User) -> Result<(), AppError>;
}

/// Porta de emissão e verificação do access token (JWT).
///
/// A implementação fixa o algoritmo de assinatura; o chamador só escolhe o
/// usuário e o instante de emissão. Nenhum método aceita algoritmo vindo de
/// fora, o que impede a troca para `none` ou chave pública.
#[async_trait]
pub trait TokenService: Send + Sync {
    /// Emite um access token para o usuário informado.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a assinatura falhar.
    async fn issue_access(&self, user: &User, now: DateTime<Utc>) -> Result<String, AppError>;

    /// Verifica assinatura, algoritmo, expiração, emissor e audiência.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Authentication`] quando o token é inválido, expirou
    /// ou foi assinado com outro segredo/algoritmo.
    async fn verify_access(&self, token: &str) -> Result<AccessClaims, AppError>;

    /// Validade do access token, em segundos, informada ao cliente.
    fn access_ttl_secs(&self) -> u64;
}

/// Registro persistido de um refresh token emitido.
#[derive(Debug, Clone)]
pub struct RefreshRecord {
    /// Chave primária interna do registro.
    pub id: String,
    /// Usuário dono do token.
    pub user_id: String,
    /// Momento de expiração do token.
    pub expires_at: DateTime<Utc>,
    /// Preenchido quando o token foi revogado (logout, rotação ou reuso).
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Porta de armazenamento de refresh tokens.
///
/// Guarda apenas o hash do token, nunca o valor em texto puro. A implementação
/// em memória serve ao desenvolvimento; Redis entra quando configurado.
#[async_trait]
pub trait RefreshTokenStore: Send + Sync {
    /// Persiste um token novo, associado ao usuário.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a gravação falhar.
    async fn insert(
        &self,
        token_hash: &str,
        user_id: &str,
        expires_at: DateTime<Utc>,
        created_at: DateTime<Utc>,
    ) -> Result<RefreshRecord, AppError>;

    /// Busca o registro pelo hash do token apresentado.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a consulta falhar.
    async fn find_by_hash(&self, token_hash: &str) -> Result<Option<RefreshRecord>, AppError>;

    /// Marca o token como usado, apontando para o sucessor quando houver.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a gravação falhar.
    async fn revoke(
        &self,
        token_hash: &str,
        replaced_by_id: Option<&str>,
        revoked_at: DateTime<Utc>,
    ) -> Result<(), AppError>;

    /// Revoga todos os tokens vivos de um usuário (logout global ou reuso detectado).
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a gravação falhar.
    async fn revoke_all_for_user(
        &self,
        user_id: &str,
        revoked_at: DateTime<Utc>,
    ) -> Result<u64, AppError>;
}
