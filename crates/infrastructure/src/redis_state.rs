//! Rate limit com janela deslizante e denylist de access token no Redis.
//!
//! Duas garantias importam aqui e ambas vêm da atomicidade do Redis:
//!
//! 1. **Contagem**: `INCR` e o `EXPIRE` da primeira tentativa acontecem em um
//!    script Lua. Fazer `INCR` e depois `EXPIRE` em dois comandos permite que
//!    uma falha entre eles deixe uma chave sem TTL — o contador nunca expira e o
//!    IP fica bloqueado para sempre.
//! 2. **Denylist**: `SET ... EX` grava o `jti` revogado com expiração igual à do
//!    token. Sem o TTL, a denylist cresceria indefinidamente com entradas que já
//!    não podem mais ser apresentadas.
//!
//! O estado fica fora do processo: reiniciar a API ou rodar três réplicas não
//! reabre a janela de força bruta.

use std::time::Duration;

use application::{
    AccessTokenDenylist, AppError, RateLimitDecision, RateLimitPolicy, RateLimitStore,
};
use async_trait::async_trait;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;

/// Script que incrementa a chave e define o TTL apenas na primeira tentativa.
///
/// Devolve `[contador_atual, ttl_em_milissegundos]`. Executar como `EVAL`
/// garante que ninguém observe o estado entre o `INCR` e o `PEXPIRE`.
const HIT_SCRIPT: &str = r"
local current = redis.call('INCR', KEYS[1])
if current == 1 then
    redis.call('PEXPIRE', KEYS[1], ARGV[1])
end
local ttl = redis.call('PTTL', KEYS[1])
return {current, ttl}
";

/// Prefixo das chaves de rate limit, para não colidir com outras chaves do Redis.
const RATE_LIMIT_PREFIX: &str = "ratelimit:";

/// Prefixo das chaves de denylist de access token.
const DENYLIST_PREFIX: &str = "denylist:jti:";

/// Tempo máximo para estabelecer uma conexão TCP com o Redis.
///
/// Sem este limite, um host que aceita SYN mas não completa o handshake deixa o
/// boot pendurado — e um boot que não termina não loga erro nenhum.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Tempo máximo de espera por cada comando enviado ao Redis.
///
/// Um Redis lento ou em partição não pode segurar a requisição HTTP até o
/// timeout do cliente: com este limite, a falha vira erro tratado.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);

/// Quantas vezes a conexão inicial é retentada antes de desistir.
///
/// Três tentativas absorvem um restart rápido do Redis; mais que isso só
/// atrasaria a descoberta de que ele está fora, e o boot precisa falhar logo.
const INITIAL_CONNECT_RETRIES: usize = 3;

/// Tempo máximo para a conexão inicial completa, incluindo as retentativas.
///
/// Aplicado por fora do crate: mesmo com `connection_timeout` configurado, o
/// laço interno de retentativa pode demorar mais que o esperado (o backoff tem
/// jitter). Este limite garante que o boot falhe com erro em vez de pendurar.
const TOTAL_CONNECT_BUDGET: Duration = Duration::from_secs(5);

/// Cliente Redis compartilhado pelos adaptadores de estado.
#[derive(Clone)]
pub struct RedisState {
    /// Gerenciador de conexão com reconexão automática.
    connection: ConnectionManager,
}

impl RedisState {
    /// Conecta ao Redis informado e devolve o cliente.
    ///
    /// Usa `ConnectionManager`, que reconecta sozinho depois de uma queda — um
    /// `MultiplexedConnection` simples deixaria a API permanentemente sem
    /// rate limit após o primeiro restart do Redis.
    ///
    /// A conexão inteira fica sob um orçamento de tempo: sem esse limite
    /// externo, um host que aceita TCP e não responde deixa o boot pendurado,
    /// e um boot que não termina não registra erro nenhum.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] quando a URL é inválida, quando a
    /// conexão falha ou quando o orçamento de tempo é excedido.
    pub async fn connect(url: &str) -> Result<Self, AppError> {
        let client = redis::Client::open(url).map_err(AppError::infrastructure)?;

        let config = redis::aio::ConnectionManagerConfig::new()
            .set_connection_timeout(CONNECT_TIMEOUT)
            .set_response_timeout(RESPONSE_TIMEOUT)
            .set_number_of_retries(INITIAL_CONNECT_RETRIES);

        let connection = tokio::time::timeout(
            TOTAL_CONNECT_BUDGET,
            client.get_connection_manager_with_config(config),
        )
        .await
        .map_err(|_| {
            AppError::infrastructure(std::io::Error::other(format!(
                "conexão com o Redis não completou em {} s",
                TOTAL_CONNECT_BUDGET.as_secs()
            )))
        })?
        .map_err(AppError::infrastructure)?;

        Ok(Self { connection })
    }

    /// Verifica se o Redis responde a um `PING`.
    ///
    /// Usado por `/ready` quando o Redis é obrigatório: um Redis fora do ar
    /// significa "não pronto", mesmo com o banco saudável.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Infrastructure`] quando o `PING` falha.
    pub async fn ping(&self) -> Result<(), AppError> {
        let mut connection = self.connection.clone();
        redis::cmd("PING")
            .query_async::<String>(&mut connection)
            .await
            .map_err(AppError::infrastructure)?;

        Ok(())
    }
}

/// Rate limit distribuído com contador por janela fixa.
#[derive(Clone)]
pub struct RedisRateLimitStore {
    /// Cliente Redis compartilhado.
    state: RedisState,
}

impl RedisRateLimitStore {
    /// Monta o adaptador sobre um cliente já conectado.
    #[must_use]
    pub fn new(state: RedisState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl RateLimitStore for RedisRateLimitStore {
    /// Incrementa atomicamente o contador da chave e retorna a decisão do rate limit ou erro do Redis.
    async fn hit(&self, key: &str, policy: RateLimitPolicy) -> Result<RateLimitDecision, AppError> {
        let mut connection = self.state.connection.clone();
        let redis_key = format!("{RATE_LIMIT_PREFIX}{key}");
        let window_millis = policy.window.as_millis().max(1);

        let (count, ttl_millis): (u32, i64) = redis::Script::new(HIT_SCRIPT)
            .key(redis_key)
            .arg(window_millis)
            .invoke_async(&mut connection)
            .await
            .map_err(AppError::infrastructure)?;

        if count > policy.max_attempts {
            // `PTTL` pode devolver -1/-2 em corrida com expiração; nesse caso
            // informamos a janela cheia, que é o pior caso para o cliente.
            let remaining = if ttl_millis > 0 {
                Duration::from_millis(ttl_millis.unsigned_abs())
            } else {
                policy.window
            };
            return Ok(RateLimitDecision::denied(remaining));
        }

        Ok(RateLimitDecision::allowed())
    }
}

/// Denylist de access tokens distribuída.
#[derive(Clone)]
pub struct RedisAccessTokenDenylist {
    /// Cliente Redis compartilhado.
    state: RedisState,
}

impl RedisAccessTokenDenylist {
    /// Monta o adaptador sobre um cliente já conectado.
    #[must_use]
    pub fn new(state: RedisState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl AccessTokenDenylist for RedisAccessTokenDenylist {
    /// Marca o identificador do token como revogado até sua expiração; retorna sucesso ou erro se o armazenamento distribuído falhar.
    async fn revoke(&self, jti: &str, expires_at_unix: i64) -> Result<(), AppError> {
        let mut connection = self.state.connection.clone();
        let key = format!("{DENYLIST_PREFIX}{jti}");

        // TTL calculado no cliente exigiria o relógio do cliente; usar
        // `EXPIREAT` no servidor mantém a expiração coerente com o `exp` do JWT.
        let _: () = redis::cmd("SET")
            .arg(&key)
            .arg("1")
            .arg("EXAT")
            .arg(expires_at_unix)
            .query_async(&mut connection)
            .await
            .map_err(AppError::infrastructure)?;

        Ok(())
    }
    /// Consulta a revogação do identificador e retorna o estado encontrado ou erro se não for possível verificar a denylist.
    async fn is_revoked(&self, jti: &str) -> Result<bool, AppError> {
        let mut connection = self.state.connection.clone();
        let key = format!("{DENYLIST_PREFIX}{jti}");

        let exists: bool = connection
            .exists(&key)
            .await
            .map_err(AppError::infrastructure)?;

        Ok(exists)
    }
}
