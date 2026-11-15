//! Tokens de autenticação: access JWT e refresh opaco.
//!
//! O access token é um JWT curto e sem estado: a validade vem da assinatura e
//! do `exp`. O refresh token é opaco (aleatório), persistido apenas como hash,
//! com rotação a cada uso e detecção de reutilização.

use std::fmt;

/// Claims mínimas carregadas no access token.
///
/// `sub` identifica o usuário; `jti` permite rastrear o token sem expor dados;
/// `iss`/`aud` travam o token a este emissor e a esta API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessClaims {
    /// Identificador do usuário dono do token (claim `sub`).
    pub subject: String,
    /// Momento de emissão, em segundos desde a época Unix (claim `iat`).
    pub issued_at: u64,
    /// Momento de expiração, em segundos desde a época Unix (claim `exp`).
    pub expires_at: u64,
    /// Identificador único do token (claim `jti`), usado em auditoria.
    pub token_id: String,
    /// Emissor esperado (claim `iss`).
    pub issuer: String,
    /// Audiência esperada (claim `aud`).
    pub audience: String,
}

/// Refresh token em texto puro, recém-gerado e ainda não persistido.
///
/// O valor só existe em memória no momento da emissão: o banco guarda apenas o
/// hash. `Debug` é omitido da implementação derivada para não vazar o segredo.
#[derive(Clone, PartialEq, Eq)]
pub struct RefreshTokenSecret(String);

impl RefreshTokenSecret {
    /// Embrulha o valor aleatório gerado pelo serviço de tokens.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Devolve o texto que será enviado ao cliente no corpo da resposta.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RefreshTokenSecret {
    /// Formata a representação protegida do refresh token sem revelar o segredo e retorna o resultado da escrita.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RefreshTokenSecret(***)")
    }
}

/// Par de tokens devolvido por login e refresh.
#[derive(Debug)]
pub struct TokenPair {
    /// JWT de acesso, curto e sem estado.
    pub access_token: String,
    /// Token opaco de renovação, já rotacionado no banco.
    pub refresh_token: RefreshTokenSecret,
    /// Validade do access token em segundos, para o cliente planejar a renovação.
    pub expires_in_secs: u64,
}

/// Resultado da rotação de um refresh token.
#[derive(Debug)]
pub enum RotationOutcome {
    /// O token era válido e foi substituído por um novo par.
    Rotated {
        /// Par novo devolvido ao cliente.
        pair: TokenPair,
        /// Identificador do usuário dono do token.
        user_id: String,
    },
    /// O token não existe, expirou ou já foi revogado.
    Invalid,
    /// O token já havia sido usado: possível roubo, então toda a família é revogada.
    Reused,
}
