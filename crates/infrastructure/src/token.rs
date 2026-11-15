//! Adaptador de access token baseado em JWT HS256.
//!
//! O algoritmo é fixado em HS256 no código: nenhum caminho aceita `alg` vindo
//! do token, o que elimina a classe de ataque de algoritmo confuso (`none` ou
//! troca para chave pública).

use application::{AppError, TokenService};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use domain::{AccessClaims, User};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

/// Emissor aceito nos tokens desta API.
pub const TOKEN_ISSUER: &str = "apirust";
/// Audiência aceita nos tokens desta API.
pub const TOKEN_AUDIENCE: &str = "apirust-api";
/// Tolerância de relógio aplicada na validação das claims de tempo.
const CLOCK_SKEW_LEEWAY_SECS: u64 = 30;

/// Claims serializadas no JWT.
///
/// Nomes curtos são obrigatórios pelas claims registradas da RFC 7519.
#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    /// Assunto: identificador do usuário.
    sub: String,
    /// Emitido em (segundos desde a época Unix).
    iat: u64,
    /// Expira em (segundos desde a época Unix).
    exp: u64,
    /// Identificador único do token.
    jti: String,
    /// Emissor.
    iss: String,
    /// Audiência.
    aud: String,
}

/// Serviço de emissão e verificação de access tokens.
pub struct JwtTokenService {
    /// Chave usada para assinar tokens novos.
    encoding: EncodingKey,
    /// Chave usada para verificar tokens recebidos.
    decoding: DecodingKey,
    /// Validade do token emitido, em segundos.
    ttl_secs: u64,
}

impl JwtTokenService {
    /// Cria o serviço a partir do segredo compartilhado e do TTL configurado.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] se `ttl_secs` for zero, o que
    /// geraria tokens já expirados.
    pub fn new(secret: &str, ttl_secs: u64) -> Result<Self, AppError> {
        if ttl_secs == 0 {
            return Err(AppError::infrastructure(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "JWT_ACCESS_TTL_SECS deve ser maior que zero",
            )));
        }

        let bytes = secret.as_bytes();
        Ok(Self {
            encoding: EncodingKey::from_secret(bytes),
            decoding: DecodingKey::from_secret(bytes),
            ttl_secs,
        })
    }

    /// Monta a validação com algoritmo fixo, emissor, audiência e claims exigidas.
    ///
    /// Centralizar aqui garante que emissão e verificação nunca divirjam.
    fn validation(&self) -> Validation {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.leeway = CLOCK_SKEW_LEEWAY_SECS;
        validation.set_issuer(&[TOKEN_ISSUER]);
        validation.set_audience(&[TOKEN_AUDIENCE]);
        validation.set_required_spec_claims(&["exp", "iat", "sub", "iss", "aud"]);
        validation
    }
}

#[async_trait]
impl TokenService for JwtTokenService {
    /// Assina um access token com as claims do usuário e retorna o token ou erro de assinatura.
    async fn issue_access(&self, user: &User, now: DateTime<Utc>) -> Result<String, AppError> {
        let issued_at = u64::try_from(now.timestamp()).map_err(AppError::infrastructure)?;
        let expires_at = u64::try_from((now + Duration::seconds(self.ttl_secs as i64)).timestamp())
            .map_err(AppError::infrastructure)?;

        let claims = Claims {
            sub: user.id().to_string(),
            iat: issued_at,
            exp: expires_at,
            jti: uuid::Uuid::new_v4().to_string(),
            iss: TOKEN_ISSUER.to_owned(),
            aud: TOKEN_AUDIENCE.to_owned(),
        };

        jsonwebtoken::encode(&Header::new(Algorithm::HS256), &claims, &self.encoding)
            .map_err(AppError::infrastructure)
    }
    /// Valida assinatura, algoritmo e validade temporal do access token e retorna as claims ou erro de autenticação.
    async fn verify_access(&self, token: &str) -> Result<AccessClaims, AppError> {
        let decoded = jsonwebtoken::decode::<Claims>(token, &self.decoding, &self.validation())
            .map_err(|_| AppError::Authentication)?;

        Ok(AccessClaims {
            subject: decoded.claims.sub,
            issued_at: decoded.claims.iat,
            expires_at: decoded.claims.exp,
            token_id: decoded.claims.jti,
            issuer: decoded.claims.iss,
            audience: decoded.claims.aud,
        })
    }
    /// Retorna o TTL do access token configurado para este serviço.
    fn access_ttl_secs(&self) -> u64 {
        self.ttl_secs
    }
}
