//! Autenticação: login, rotação de refresh token e logout.
//!
//! O refresh token é opaco e persistido apenas como hash. Cada uso o revoga e
//! emite um sucessor (rotação). Se um token já revogado reaparecer, tratamos
//! como roubo: toda a família daquele usuário é invalidada.

use chrono::{DateTime, Duration, Utc};
use domain::{Email, RefreshTokenSecret, RotationOutcome, TokenPair, UserId};

use crate::error::AppError;
use crate::ports::{PasswordHasher, RefreshTokenStore, TokenService, UserRepository};

/// Validade padrão do refresh token: 30 dias.
pub const REFRESH_TTL_DAYS: i64 = 30;

/// Autentica credenciais e gerencia o ciclo de vida dos tokens.
pub struct AuthenticateUser<'a> {
    /// Repositório consultado para localizar o usuário pelo e-mail.
    users: &'a dyn UserRepository,
    /// Verifica a senha contra o hash persistido.
    hasher: &'a dyn PasswordHasher,
    /// Emite e valida o access token.
    tokens: &'a dyn TokenService,
    /// Persiste e revoga os refresh tokens.
    refresh: &'a dyn RefreshTokenStore,
}

impl<'a> AuthenticateUser<'a> {
    /// Monta o caso de uso com as dependências injetadas.
    #[must_use]
    pub fn new(
        users: &'a dyn UserRepository,
        hasher: &'a dyn PasswordHasher,
        tokens: &'a dyn TokenService,
        refresh: &'a dyn RefreshTokenStore,
    ) -> Self {
        Self {
            users,
            hasher,
            tokens,
            refresh,
        }
    }

    /// Autentica e-mail e senha, emitindo um par de tokens.
    ///
    /// Usuário inexistente, senha errada e conta inativa produzem o mesmo erro
    /// público, para não revelar quais e-mails existem.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Authentication`] para credenciais inválidas e
    /// [`AppError::Infrastructure`] em falhas de banco, hashing ou assinatura.
    pub async fn login(
        &self,
        email: &Email,
        password: &str,
        now: DateTime<Utc>,
    ) -> Result<TokenPair, AppError> {
        let stored = self.users.find_by_email(email).await?;

        let Some(user) = stored else {
            return Err(AppError::Authentication);
        };
        if !user.is_active() {
            return Err(AppError::Authentication);
        }

        let password_ok = self.hasher.verify(password, user.password_hash()).await?;
        if !password_ok {
            return Err(AppError::Authentication);
        }

        let pair = self.issue_pair(&user, now).await?;

        Ok(pair.0)
    }

    /// Rotaciona o refresh token, detectando reutilização.
    ///
    /// Um token válido é revogado e substituído por um novo par. Se o token
    /// apresentado já estava revogado, todos os tokens do usuário são revogados
    /// (a sessão é considerada comprometida).
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Authentication`] para token desconhecido ou expirado
    /// e [`AppError::Infrastructure`] em falhas de persistência.
    pub async fn refresh(
        &self,
        presented: &RefreshTokenSecret,
        hash: &str,
        now: DateTime<Utc>,
    ) -> Result<RotationOutcome, AppError> {
        let _ = presented;
        let Some(record) = self.refresh.find_by_hash(hash).await? else {
            return Ok(RotationOutcome::Invalid);
        };

        if record.revoked_at.is_some() {
            // Token já usado: possível roubo. Invalida toda a família.
            self.refresh
                .revoke_all_for_user(&record.user_id, now)
                .await?;
            return Ok(RotationOutcome::Reused);
        }

        if record.expires_at <= now {
            return Ok(RotationOutcome::Invalid);
        }

        let user_id = UserId::parse(&record.user_id)?;
        let Some(user) = self.users.find_by_id(user_id).await? else {
            return Ok(RotationOutcome::Invalid);
        };
        if !user.is_active() {
            return Ok(RotationOutcome::Invalid);
        }

        let (pair, successor_id) = self.issue_pair(&user, now).await?;
        self.refresh.revoke(hash, Some(&successor_id), now).await?;

        Ok(RotationOutcome::Rotated {
            pair,
            user_id: record.user_id,
        })
    }

    /// Revoga o refresh token apresentado (logout daquela sessão).
    ///
    /// Logout é idempotente: token desconhecido não é erro.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se a revogação falhar.
    pub async fn logout(&self, hash: &str, now: DateTime<Utc>) -> Result<(), AppError> {
        if self.refresh.find_by_hash(hash).await?.is_some() {
            self.refresh.revoke(hash, None, now).await?;
        }
        Ok(())
    }

    /// Emite o par de tokens e persiste o refresh com expiração própria.
    ///
    /// Devolve também o id do registro persistido, usado para encadear
    /// `replaced_by_id` na rotação.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] em falha de assinatura ou gravação.
    async fn issue_pair(
        &self,
        user: &domain::User,
        now: DateTime<Utc>,
    ) -> Result<(TokenPair, String), AppError> {
        let access_token = self.tokens.issue_access(user, now).await?;
        let secret = RefreshTokenSecret::new(random_hex_token());
        let expires_at = now + Duration::days(REFRESH_TTL_DAYS);

        let record = self
            .refresh
            .insert(
                &hash_refresh(secret.expose()),
                &user.id().to_string(),
                expires_at,
                now,
            )
            .await?;

        let pair = TokenPair {
            access_token,
            refresh_token: secret,
            expires_in_secs: self.tokens.access_ttl_secs(),
        };

        Ok((pair, record.id))
    }
}

/// Gera um token opaco aleatório de 32 bytes, em hexadecimal.
///
/// 256 bits de entropia tornam a adivinhação inviável e evitam colisão entre
/// sessões simultâneas.
#[must_use]
pub fn random_hex_token() -> String {
    use rand::RngCore;

    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Calcula o hash SHA-256 do refresh token para persistência.
///
/// SHA-256 (e não Argon2) porque o token já tem 256 bits de entropia: não é
/// adivinhável, então o custo do Argon2 só atrasaria cada renovação.
#[must_use]
pub fn hash_refresh(token: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}
