//! Leitura e validação tipada da configuração obtida do ambiente.
//!
//! Centraliza defaults, limites operacionais e regras que fazem o processo
//! falhar cedo quando há valores ausentes ou combinações inseguras.

use std::fmt;
use std::net::IpAddr;

use domain::{Email, validate_password};
use thiserror::Error;

/// Comprimento mínimo aceito para o segredo JWT, em bytes.
const MIN_JWT_SECRET_LEN: usize = 32;

/// Valor padrão de `APP_HOST`: apenas loopback, para não expor a API por engano.
const DEFAULT_HOST: &str = "127.0.0.1";
/// Valor padrão de `APP_PORT`.
const DEFAULT_PORT: &str = "8080";
/// Valor padrão de `JWT_ACCESS_TTL_SECS` (15 minutos).
const DEFAULT_JWT_ACCESS_TTL_SECS: u64 = 900;
/// Teto mínimo de requisições HTTP simultâneas.
const MIN_MAX_CONCURRENT_REQUESTS: usize = 1;
/// Teto máximo aceito para `MAX_CONCURRENT_REQUESTS` (proteção contra valor absurdo).
const ABSOLUTE_MAX_CONCURRENT_REQUESTS: usize = 10_000;

/// Ambiente de execução da aplicação.
///
/// Muda o formato de log (pretty em desenvolvimento, JSON em produção) e
/// habilita bloqueios de configuração insegura fora de produção.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    /// Desenvolvimento local: log legível e recursos auxiliares habilitados.
    Development,
    /// Produção: log JSON, sem atalhos de desenvolvimento.
    Production,
}

impl Environment {
    /// Converte `APP_ENV` no ambiente correspondente.
    ///
    /// # Errors
    ///
    /// Retorna [`ConfigError::Invalid`] para valores diferentes de
    /// `development`/`dev` e `production`/`prod`.
    fn parse(value: &str) -> Result<Self, ConfigError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "development" | "dev" => Ok(Self::Development),
            "production" | "prod" => Ok(Self::Production),
            other => Err(ConfigError::Invalid {
                key: "APP_ENV",
                reason: format!("esperado development ou production, recebido `{other}`"),
            }),
        }
    }
}

/// Segredo usado para assinar JWTs.
///
/// O valor é deliberadamente omitido de `Debug` para não vazar em logs, e só
/// pode ser lido por [`JwtSecret::expose`], deixando explícito na chamada que
/// um segredo está sendo manipulado.
#[derive(Clone)]
pub struct JwtSecret(String);

impl JwtSecret {
    /// Expõe o valor bruto para o serviço de tokens.
    ///
    /// Chame apenas no ponto que assina ou verifica o token; nunca em log,
    /// DTO de resposta ou mensagem de erro.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for JwtSecret {
    /// Formata o erro de configuração para diagnóstico sem expor segredos; escreve no formatter e retorna o resultado da formatação.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JwtSecret(***)")
    }
}

/// Credenciais do Admin inicial, lidas do ambiente.
///
/// `Debug` omite os dois campos para que nem a senha nem o e-mail apareçam em
/// logs de configuração.
#[derive(Clone)]
pub struct AdminSeed {
    /// E-mail já normalizado e validado do Admin a provisionar.
    email: Email,
    /// Senha em texto puro; vive em memória apenas até o hashing no boot.
    password: String,
}

impl AdminSeed {
    /// Devolve o e-mail normalizado do Admin inicial.
    #[must_use]
    pub fn email(&self) -> &Email {
        &self.email
    }

    /// Devolve a senha em texto puro para o hashing de provisionamento.
    ///
    /// Não logue nem serialize este valor.
    #[must_use]
    pub fn password(&self) -> &str {
        &self.password
    }
}

impl fmt::Debug for AdminSeed {
    /// Formata o erro de configuração para diagnóstico sem expor segredos; escreve no formatter e retorna o resultado da formatação.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AdminSeed { email: <oculto>, password: <oculto> }")
    }
}

/// Configuração tipada da aplicação, validada no boot.
///
/// Só existe instância válida: os campos são preenchidos por
/// [`AppConfig::from_env`], que aborta o processo antes de abrir a porta se algo
/// estiver errado.
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// Ambiente de execução, que define o formato de log e bloqueios.
    pub environment: Environment,
    /// Endereço IP em que o servidor faz bind.
    pub host: IpAddr,
    /// Porta TCP em que o servidor escuta.
    pub port: u16,
    /// URL de conexão do PostgreSQL (`postgres://usuario:senha@host:porta/banco`).
    pub database_url: String,
    /// Schema do PostgreSQL usado pela aplicação.
    ///
    /// Quando `None`, o pool usa o `search_path` padrão do usuário (normalmente
    /// `public`). Em desenvolvimento compartilhado, definir `DATABASE_SCHEMA`
    /// isola as tabelas da API das de outras aplicações do mesmo banco.
    pub database_schema: Option<String>,
    /// Se `true`, aplica as migrations embutidas durante o boot.
    pub run_migrations: bool,
    /// URL do Redis, quando configurado (uso efetivo a partir da Fase 3).
    pub redis_url: Option<String>,
    /// Segredo de assinatura dos JWTs.
    pub jwt_secret: JwtSecret,
    /// TTL do access token, em segundos.
    pub jwt_access_ttl_secs: u64,
    /// Se `true`, encerra o servidor 10 s após a última aba fechar (só em dev).
    pub shutdown_on_tab_close: bool,
    /// Credenciais do Admin inicial, presentes apenas quando ambas as variáveis
    /// `ADMIN_EMAIL` e `ADMIN_PASSWORD` estão definidas.
    pub admin_seed: Option<AdminSeed>,
    /// Se `true`, monta `/swagger-ui` e `/api-docs/openapi.json`.
    ///
    /// **Nunca** `true` em produção: o boot rejeita a combinação em
    /// `AppConfig::from_env`.
    pub expose_api_docs: bool,
    /// Origens permitidas por CORS, lidas de `CORS_ALLOWED_ORIGINS`.
    ///
    /// Vazio (padrão) significa "nenhuma origem cruzada", o que é o correto para
    /// uma API consumida por cliente próprio. Produção costuma exigir a lista
    /// explícita dos domínios do frontend.
    pub allowed_origins: Vec<String>,
    /// Máximo de requisições HTTP simultâneas; o excesso responde `503`.
    pub max_concurrent_requests: usize,
    /// Segundos de espera das conexões em voo após o sinal de shutdown.
    pub shutdown_drain_secs: u64,
    /// Token exigido por `/metrics`; `None` mantém a rota desabilitada.
    pub metrics_token: Option<String>,
    /// IPs de proxy confiáveis, lidos de `TRUSTED_PROXIES`.
    ///
    /// Vazio (padrão) significa "a API é exposta direto": o IP do cliente é o
    /// peer do TCP e `X-Forwarded-For` é ignorado. Atrás de reverse proxy, liste
    /// os IPs do proxy; só então o cabeçalho é considerado — e apenas quando o
    /// peer está nesta lista.
    pub trusted_proxies: Vec<IpAddr>,
}

impl AppConfig {
    /// Carrega `.env` (se existir) e valida a configuração do ambiente.
    ///
    /// # Errors
    ///
    /// Retorna [`ConfigError`] quando uma variável obrigatória está ausente ou
    /// possui valor inválido.
    pub fn from_env() -> Result<Self, ConfigError> {
        let _ = dotenvy::dotenv();
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Constrói a configuração a partir de pares chave/valor.
    ///
    /// Exposto para testes e para integrações que já têm a configuração em mãos.
    /// Produção usa [`AppConfig::from_env`].
    ///
    /// # Errors
    ///
    /// Retorna [`ConfigError`] nas mesmas condições de `from_env`.
    pub fn from_map(pairs: &[(&str, &str)]) -> Result<Self, ConfigError> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();

        Self::from_lookup(|key| {
            owned
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        })
    }

    /// Constrói a configuração a partir de um leitor de variáveis injetado.
    ///
    /// Manter o `lookup` genérico é o que permite testar todas as regras de
    /// validação sem tocar em `std::env`, que é global e não isolável.
    ///
    /// # Errors
    ///
    /// Retorna [`ConfigError::Missing`] para variável obrigatória ausente e
    /// [`ConfigError::Invalid`] para valor fora do formato esperado.
    fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let environment = Environment::parse(&optional(&lookup, "APP_ENV", "development"))?;

        let host = optional(&lookup, "APP_HOST", DEFAULT_HOST)
            .parse()
            .map_err(|error| ConfigError::Invalid {
                key: "APP_HOST",
                reason: format!("endereço IP inválido: {error}"),
            })?;

        let port = optional(&lookup, "APP_PORT", DEFAULT_PORT)
            .parse()
            .map_err(|error| ConfigError::Invalid {
                key: "APP_PORT",
                reason: format!("porta inválida: {error}"),
            })?;

        let database_url = required(&lookup, "DATABASE_URL")?;
        // Falha cedo: uma URL `sqlite://` herdada de configuração antiga abriria
        // um erro obscuro na primeira consulta, muito depois do boot.
        if !database_url.starts_with("postgres://") && !database_url.starts_with("postgresql://") {
            return Err(ConfigError::Invalid {
                key: "DATABASE_URL",
                reason: "deve começar com postgres:// ou postgresql:// (o banco é PostgreSQL)"
                    .to_owned(),
            });
        }
        validate_url_credentials("DATABASE_URL", &database_url)?;

        let run_migrations = parse_bool(
            &optional(&lookup, "RUN_MIGRATIONS", "true"),
            "RUN_MIGRATIONS",
        )?;

        let redis_url = lookup_value(&lookup, "REDIS_URL");
        if let Some(url) = redis_url.as_deref() {
            validate_url_credentials("REDIS_URL", url)?;
        }

        // Schema opcional: em desenvolvimento compartilhado evita misturar
        // tabelas da API com as de outras aplicações no mesmo banco.
        let database_schema = lookup_value(&lookup, "DATABASE_SCHEMA");

        let admin_seed = parse_admin_seed(&lookup)?;

        let shutdown_on_tab_close = parse_bool(
            &optional(&lookup, "SHUTDOWN_ON_TAB_CLOSE", "false"),
            "SHUTDOWN_ON_TAB_CLOSE",
        )?;
        if environment == Environment::Production && shutdown_on_tab_close {
            return Err(ConfigError::Invalid {
                key: "SHUTDOWN_ON_TAB_CLOSE",
                reason: "não pode ser habilitado em produção".to_owned(),
            });
        }

        let jwt_secret = required(&lookup, "JWT_SECRET")?;
        if jwt_secret.len() < MIN_JWT_SECRET_LEN {
            return Err(ConfigError::Invalid {
                key: "JWT_SECRET",
                reason: format!(
                    "deve ter pelo menos {MIN_JWT_SECRET_LEN} caracteres, recebido {}",
                    jwt_secret.len()
                ),
            });
        }

        let jwt_access_ttl_secs =
            parse_positive_u64(&lookup, "JWT_ACCESS_TTL_SECS", DEFAULT_JWT_ACCESS_TTL_SECS)?;

        let expose_api_docs = parse_api_docs(&lookup, environment)?;

        let allowed_origins = parse_allowed_origins(&lookup)?;

        let max_concurrent_requests = parse_max_concurrent_requests(&lookup)?;
        let shutdown_drain_secs = parse_positive_u64(
            &lookup,
            "SHUTDOWN_DRAIN_SECS",
            crate::operation::DEFAULT_SHUTDOWN_DRAIN.as_secs(),
        )?;

        // Token curto protege `/metrics`; vazio vira `None` e a rota não existe.
        let metrics_token = lookup_value(&lookup, "METRICS_TOKEN");

        let trusted_proxies = parse_trusted_proxies(&lookup)?;

        Ok(Self {
            environment,
            host,
            port,
            database_url,
            run_migrations,
            redis_url,
            database_schema,
            jwt_secret: JwtSecret(jwt_secret),
            jwt_access_ttl_secs,
            shutdown_on_tab_close,
            admin_seed,
            expose_api_docs,
            allowed_origins,
            max_concurrent_requests,
            shutdown_drain_secs,
            metrics_token,
            trusted_proxies,
        })
    }
}

/// Falhas de configuração detectadas no boot.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    /// A variável obrigatória não foi definida (ou está vazia).
    #[error("configuração obrigatória ausente: {key}")]
    Missing {
        /// Nome da variável de ambiente em falta.
        key: &'static str,
    },

    /// A variável existe, mas viola o formato ou a regra de segurança.
    #[error("configuração inválida em {key}: {reason}")]
    Invalid {
        /// Nome da variável de ambiente inválida.
        key: &'static str,
        /// Explicação legível, sem expor o valor recebido.
        reason: String,
    },
}

/// Lê uma variável obrigatória, já sem espaços.
///
/// # Errors
///
/// Retorna [`ConfigError::Missing`] quando a chave está ausente ou vazia.
fn required<F>(lookup: &F, key: &'static str) -> Result<String, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    lookup(key)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or(ConfigError::Missing { key })
}

/// Bytes de entropia recomendados ao gerar senha de serviço.
///
/// Vai na mensagem de erro como orientação concreta; o valor acompanha o
/// exemplo `openssl rand -hex 32` documentado no `.env.example`.
const RECOMMENDED_PASSWORD_BYTES: u8 = 32;

/// Rejeita credencial contendo caractere que quebra o componente de URL.
///
/// Senhas geradas com `openssl rand -base64` produzem `/`, `+` e `=`. Dentro de
/// `postgres://user:SENHA@host`, o `/` encerra a autoridade e `+` é lido como
/// espaço — a URL deixa de apontar para o host certo, e o erro só aparece como
/// "connection refused" no boot.
///
/// A checagem olha apenas o trecho `user:senha@`, para não recusar caracteres
/// legítimos em query string ou caminho.
///
/// # Errors
///
/// Retorna [`ConfigError::Invalid`] quando a credencial tem `/`, `+` ou espaço.
fn validate_url_credentials(key: &'static str, url: &str) -> Result<(), ConfigError> {
    // Sem `@`, não há credencial embutida: nada a validar.
    let Some((authority, _)) = url.split_once('@') else {
        return Ok(());
    };
    let credentials = authority
        .rsplit_once("//")
        .map_or(authority, |(_, rest)| rest);

    for forbidden in ['/', '+', ' ', '#', '?'] {
        if credentials.contains(forbidden) {
            return Err(ConfigError::Invalid {
                key,
                reason: format!(
                    "credencial contém `{forbidden}`, que é inválido numa URL; \
                     gere a senha com `openssl rand -hex {RECOMMENDED_PASSWORD_BYTES}`"
                ),
            });
        }
    }

    Ok(())
}

/// Lê `TRUSTED_PROXIES` como lista de IPs separados por vírgula.
///
/// Vazio significa "API exposta diretamente": o rate limit usa o peer do socket
/// e ignora `X-Forwarded-For`, que é forjável pelo cliente.
///
/// # Errors
///
/// Retorna [`ConfigError::Invalid`] quando algum valor não é um IP válido.
/// Recusar no boot evita o pior cenário: uma config com erro de digitação faria
/// o proxy não ser reconhecido, e o limite cairia no IP do proxy — exatamente o
/// problema que `TRUSTED_PROXIES` existe para resolver.
fn parse_trusted_proxies<F>(lookup: &F) -> Result<Vec<std::net::IpAddr>, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let Some(raw) = lookup("TRUSTED_PROXIES") else {
        return Ok(Vec::new());
    };

    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry.parse().map_err(|error| ConfigError::Invalid {
                key: "TRUSTED_PROXIES",
                reason: format!("`{entry}` não é um IP válido: {error}"),
            })
        })
        .collect()
}

/// Lê uma variável opcional, caindo no `default` quando ausente ou vazia.
fn optional<F>(lookup: &F, key: &'static str, default: &str) -> String
where
    F: Fn(&str) -> Option<String>,
{
    lookup_value(lookup, key).unwrap_or_else(|| default.to_owned())
}

/// Lê e normaliza uma variável, tratando string vazia como ausente.
///
/// Devolve `None` quando a chave não existe, está vazia ou contém só espaços.
fn lookup_value<F>(lookup: &F, key: &str) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    lookup(key)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Monta as credenciais do Admin inicial, exigindo as duas variáveis juntas.
///
/// # Errors
///
/// Retorna [`ConfigError::Missing`] quando apenas uma das variáveis está
/// definida e [`ConfigError::Invalid`] quando o e-mail ou a senha violam a
/// política de domínio.
fn parse_admin_seed<F>(lookup: &F) -> Result<Option<AdminSeed>, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let email = lookup_value(lookup, "ADMIN_EMAIL");
    let password = lookup_value(lookup, "ADMIN_PASSWORD");

    match (email, password) {
        (None, None) => Ok(None),
        (Some(_), None) => Err(ConfigError::Missing {
            key: "ADMIN_PASSWORD",
        }),
        (None, Some(_)) => Err(ConfigError::Missing { key: "ADMIN_EMAIL" }),
        (Some(email), Some(password)) => {
            let email = Email::parse(&email).map_err(|error| ConfigError::Invalid {
                key: "ADMIN_EMAIL",
                reason: error.to_string(),
            })?;

            validate_password(&password).map_err(|error| ConfigError::Invalid {
                key: "ADMIN_PASSWORD",
                reason: error.to_string(),
            })?;

            Ok(Some(AdminSeed { email, password }))
        }
    }
}

/// Decide se a documentação interativa deve ser montada.
///
/// Padrão: habilitada em `development`, desabilitada em `production`. A variável
/// `EXPOSE_API_DOCS` pode **desabilitar** em desenvolvimento (útil para testar o
/// cenário de produção), mas **nunca habilitar** em produção — essa combinação
/// aborta o boot com erro de configuração, em vez de ser ignorada em silêncio.
///
/// # Errors
///
/// Retorna [`ConfigError::Invalid`] quando `EXPOSE_API_DOCS` tem valor inválido
/// ou quando se tenta habilitar a documentação em produção.
fn parse_api_docs<F>(lookup: &F, environment: Environment) -> Result<bool, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let requested = lookup_value(lookup, "EXPOSE_API_DOCS")
        .map(|value| parse_bool(&value, "EXPOSE_API_DOCS"))
        .transpose()?;

    match (environment, requested) {
        // Produção com pedido explícito de habilitar é rejeitado.
        (Environment::Production, Some(true)) => Err(ConfigError::Invalid {
            key: "EXPOSE_API_DOCS",
            reason: "documentação interativa não pode ser habilitada em produção".to_owned(),
        }),
        // Produção: sempre desabilitada, independente da ausência da variável.
        (Environment::Production, _) => Ok(false),
        // Desenvolvimento: padrão habilitado, mas pode ser desligado.
        (Environment::Development, Some(false)) => Ok(false),
        (Environment::Development, _) => Ok(true),
    }
}

/// Lê a lista de origens permitidas para CORS.
///
/// Formato: valores separados por vírgula em `CORS_ALLOWED_ORIGINS`, por exemplo
/// `https://app.exemplo.com,https://admin.exemplo.com`. Vazio significa nenhuma
/// origem cruzada permitida.
///
/// Rejeitar `*` é deliberado: liberar qualquer origem anula o propósito do CORS
/// em uma API que usa `Authorization`.
///
/// # Errors
///
/// Retorna [`ConfigError::Invalid`] quando a lista contém `*`.
fn parse_allowed_origins<F>(lookup: &F) -> Result<Vec<String>, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let Some(raw) = lookup_value(lookup, "CORS_ALLOWED_ORIGINS") else {
        return Ok(Vec::new());
    };

    let origins: Vec<String> = raw
        .split(',')
        .map(|origin| origin.trim().to_owned())
        .filter(|origin| !origin.is_empty())
        .collect();

    if origins.iter().any(|origin| origin == "*") {
        return Err(ConfigError::Invalid {
            key: "CORS_ALLOWED_ORIGINS",
            reason: "`*` não é permitido; liste as origens explicitamente".to_owned(),
        });
    }

    Ok(origins)
}

/// Converte um texto em `bool`, aceitando as formas usuais de configuração.
///
/// # Errors
///
/// Retorna [`ConfigError::Invalid`] para qualquer valor fora de
/// `true/1/yes/false/0/no`.
fn parse_bool(value: &str, key: &'static str) -> Result<bool, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        other => Err(ConfigError::Invalid {
            key,
            reason: format!("esperado true ou false, recebido `{other}`"),
        }),
    }
}

/// Lê um inteiro positivo em segundos, usando `default` quando a chave falta.
///
/// Zero é rejeitado: um TTL ou drain nulo não é configuração válida.
fn parse_positive_u64<F>(lookup: &F, key: &'static str, default: u64) -> Result<u64, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = optional(lookup, key, &default.to_string());

    let seconds = raw.parse::<u64>().map_err(|error| ConfigError::Invalid {
        key,
        reason: format!("esperado um número de segundos, recebido `{raw}`: {error}"),
    })?;

    if seconds == 0 {
        return Err(ConfigError::Invalid {
            key,
            reason: "deve ser maior que zero".to_owned(),
        });
    }

    Ok(seconds)
}

/// Lê o teto de requisições simultâneas, rejeitando zero e valores absurdos.
fn parse_max_concurrent_requests<F>(lookup: &F) -> Result<usize, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let default = crate::operation::DEFAULT_MAX_CONCURRENT_REQUESTS;
    let raw = optional(lookup, "MAX_CONCURRENT_REQUESTS", &default.to_string());
    let max_concurrent = raw.parse::<usize>().map_err(|error| ConfigError::Invalid {
        key: "MAX_CONCURRENT_REQUESTS",
        reason: format!("esperado um inteiro positivo, recebido `{raw}`: {error}"),
    })?;

    if !(MIN_MAX_CONCURRENT_REQUESTS..=ABSOLUTE_MAX_CONCURRENT_REQUESTS).contains(&max_concurrent) {
        return Err(ConfigError::Invalid {
            key: "MAX_CONCURRENT_REQUESTS",
            reason: format!(
                "deve estar entre {MIN_MAX_CONCURRENT_REQUESTS} e {ABSOLUTE_MAX_CONCURRENT_REQUESTS}"
            ),
        });
    }

    Ok(max_concurrent)
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Busca uma variável no ambiente simulado e retorna `None` quando ela não foi configurada.
    fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    /// Segredo JWT com tamanho mínimo válido para os testes de configuração.
    const VALID_SECRET: &str = "0123456789abcdef0123456789abcdef";
    /// Senha de fixture que satisfaz a política do administrador inicial.
    const ADMIN_PASSWORD: &str = "ChangeMe-Admin-2026!";

    /// Verifica o contrato identificado por `applies_defaults_for_optional_values` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn applies_defaults_for_optional_values() {
        let pairs = [
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", VALID_SECRET),
        ];

        let config = AppConfig::from_lookup(lookup(&pairs)).expect("configuração válida");

        assert_eq!(config.environment, Environment::Development);
        assert_eq!(config.host.to_string(), DEFAULT_HOST);
        assert_eq!(config.port, 8080);
        assert_eq!(
            config.database_url,
            "postgres://user:pass@localhost:5432/test"
        );
        assert!(!config.shutdown_on_tab_close);
        assert!(config.run_migrations);
        assert!(config.redis_url.is_none());
        assert!(config.admin_seed.is_none());
        assert_eq!(config.jwt_access_ttl_secs, DEFAULT_JWT_ACCESS_TTL_SECS);
        assert_eq!(
            config.max_concurrent_requests,
            crate::operation::DEFAULT_MAX_CONCURRENT_REQUESTS
        );
        assert_eq!(
            config.shutdown_drain_secs,
            crate::operation::DEFAULT_SHUTDOWN_DRAIN.as_secs()
        );
    }

    /// Verifica o contrato identificado por `parses_operational_limits_from_the_environment` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn parses_operational_limits_from_the_environment() {
        let pairs = [
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", VALID_SECRET),
            ("MAX_CONCURRENT_REQUESTS", "32"),
            ("SHUTDOWN_DRAIN_SECS", "15"),
        ];

        let config = AppConfig::from_lookup(lookup(&pairs)).expect("configuração válida");

        assert_eq!(config.max_concurrent_requests, 32);
        assert_eq!(config.shutdown_drain_secs, 15);
    }

    /// Verifica o contrato identificado por `rejects_zero_max_concurrent_requests` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_zero_max_concurrent_requests() {
        let pairs = [
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", VALID_SECRET),
            ("MAX_CONCURRENT_REQUESTS", "0"),
        ];

        let error = AppConfig::from_lookup(lookup(&pairs)).expect_err("teto zero");

        assert!(matches!(
            error,
            ConfigError::Invalid {
                key: "MAX_CONCURRENT_REQUESTS",
                ..
            }
        ));
    }

    /// Verifica o contrato identificado por `parses_admin_seed_when_both_values_are_present` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn parses_admin_seed_when_both_values_are_present() {
        let pairs = [
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", VALID_SECRET),
            ("ADMIN_EMAIL", " Admin@Example.COM "),
            ("ADMIN_PASSWORD", ADMIN_PASSWORD),
        ];

        let config = AppConfig::from_lookup(lookup(&pairs)).expect("configuração válida");
        let seed = config.admin_seed.expect("seed configurado");

        assert_eq!(seed.email().as_str(), "admin@example.com");
        assert_eq!(seed.password(), ADMIN_PASSWORD);
    }

    /// Verifica o contrato identificado por `rejects_admin_seed_with_only_email` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_admin_seed_with_only_email() {
        let pairs = [
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", VALID_SECRET),
            ("ADMIN_EMAIL", "admin@example.com"),
        ];

        let error = AppConfig::from_lookup(lookup(&pairs)).expect_err("senha ausente");

        assert_eq!(
            error,
            ConfigError::Missing {
                key: "ADMIN_PASSWORD"
            }
        );
    }

    /// Verifica o contrato identificado por `rejects_admin_seed_with_only_password` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_admin_seed_with_only_password() {
        let pairs = [
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", VALID_SECRET),
            ("ADMIN_PASSWORD", ADMIN_PASSWORD),
        ];

        let error = AppConfig::from_lookup(lookup(&pairs)).expect_err("e-mail ausente");

        assert_eq!(error, ConfigError::Missing { key: "ADMIN_EMAIL" });
    }

    /// Verifica o contrato identificado por `rejects_invalid_admin_email` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_invalid_admin_email() {
        let pairs = [
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", VALID_SECRET),
            ("ADMIN_EMAIL", "nao-e-email"),
            ("ADMIN_PASSWORD", ADMIN_PASSWORD),
        ];

        let error = AppConfig::from_lookup(lookup(&pairs)).expect_err("e-mail inválido");

        assert!(matches!(
            error,
            ConfigError::Invalid {
                key: "ADMIN_EMAIL",
                ..
            }
        ));
    }

    /// Verifica o contrato identificado por `rejects_weak_admin_password` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_weak_admin_password() {
        let pairs = [
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", VALID_SECRET),
            ("ADMIN_EMAIL", "admin@example.com"),
            ("ADMIN_PASSWORD", "fraca"),
        ];

        let error = AppConfig::from_lookup(lookup(&pairs)).expect_err("senha fraca");

        assert!(matches!(
            error,
            ConfigError::Invalid {
                key: "ADMIN_PASSWORD",
                ..
            }
        ));
    }

    /// Verifica o contrato identificado por `debug_redacts_admin_seed` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn debug_redacts_admin_seed() {
        let pairs = [
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", VALID_SECRET),
            ("ADMIN_EMAIL", "admin@example.com"),
            ("ADMIN_PASSWORD", ADMIN_PASSWORD),
        ];

        let config = AppConfig::from_lookup(lookup(&pairs)).expect("configuração válida");
        let seed = config.admin_seed.expect("seed configurado");
        let debug = format!("{seed:?}");

        assert!(!debug.contains(ADMIN_PASSWORD));
        assert!(!debug.contains("admin@example.com"));
        assert!(debug.contains("oculto"));
    }

    /// Verifica o contrato identificado por `accepts_shutdown_on_tab_close_outside_production` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn accepts_shutdown_on_tab_close_outside_production() {
        let pairs = [
            ("SHUTDOWN_ON_TAB_CLOSE", "true"),
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", VALID_SECRET),
        ];

        let config = AppConfig::from_lookup(lookup(&pairs)).expect("configuração válida");

        assert!(config.shutdown_on_tab_close);
    }

    /// Verifica o contrato identificado por `rejects_shutdown_on_tab_close_in_production` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_shutdown_on_tab_close_in_production() {
        let pairs = [
            ("APP_ENV", "production"),
            ("SHUTDOWN_ON_TAB_CLOSE", "true"),
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", VALID_SECRET),
        ];

        let error = AppConfig::from_lookup(lookup(&pairs)).expect_err("opção insegura");

        assert!(matches!(
            error,
            ConfigError::Invalid {
                key: "SHUTDOWN_ON_TAB_CLOSE",
                ..
            }
        ));
    }

    /// Verifica o contrato identificado por `accepts_production_environment` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn accepts_production_environment() {
        let pairs = [
            ("APP_ENV", "production"),
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", VALID_SECRET),
        ];

        let config = AppConfig::from_lookup(lookup(&pairs)).expect("configuração válida");

        assert_eq!(config.environment, Environment::Production);
    }

    /// Verifica o contrato identificado por `rejects_missing_jwt_secret` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_missing_jwt_secret() {
        let pairs = [("DATABASE_URL", "postgres://user:pass@localhost:5432/test")];

        let error = AppConfig::from_lookup(lookup(&pairs)).expect_err("segredo ausente");

        assert_eq!(error, ConfigError::Missing { key: "JWT_SECRET" });
    }

    /// Verifica o contrato identificado por `rejects_short_jwt_secret` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_short_jwt_secret() {
        let pairs = [
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", "curto"),
        ];

        let error = AppConfig::from_lookup(lookup(&pairs)).expect_err("segredo curto");

        assert!(matches!(
            error,
            ConfigError::Invalid {
                key: "JWT_SECRET",
                ..
            }
        ));
    }

    /// Verifica o contrato identificado por `rejects_invalid_port` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_invalid_port() {
        let pairs = [
            ("APP_PORT", "não-é-porta"),
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", VALID_SECRET),
        ];

        let error = AppConfig::from_lookup(lookup(&pairs)).expect_err("porta inválida");

        assert!(matches!(
            error,
            ConfigError::Invalid {
                key: "APP_PORT",
                ..
            }
        ));
    }

    /// Verifica o contrato identificado por `debug_does_not_leak_jwt_secret` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn debug_does_not_leak_jwt_secret() {
        let secret = JwtSecret(VALID_SECRET.to_owned());

        assert_eq!(format!("{secret:?}"), "JwtSecret(***)");
    }

    /// Verifica o contrato identificado por `parses_custom_jwt_access_ttl` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn parses_custom_jwt_access_ttl() {
        let pairs = [
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", VALID_SECRET),
            ("JWT_ACCESS_TTL_SECS", "300"),
        ];

        let config = AppConfig::from_lookup(lookup(&pairs)).expect("configuração válida");

        assert_eq!(config.jwt_access_ttl_secs, 300);
    }

    /// Verifica o contrato identificado por `rejects_non_numeric_jwt_access_ttl` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_non_numeric_jwt_access_ttl() {
        let pairs = [
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", VALID_SECRET),
            ("JWT_ACCESS_TTL_SECS", "quinze-minutos"),
        ];

        let error = AppConfig::from_lookup(lookup(&pairs)).expect_err("TTL inválido");

        assert!(matches!(
            error,
            ConfigError::Invalid {
                key: "JWT_ACCESS_TTL_SECS",
                ..
            }
        ));
    }

    /// Verifica o contrato identificado por `rejects_zero_jwt_access_ttl` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_zero_jwt_access_ttl() {
        let pairs = [
            ("DATABASE_URL", "postgres://user:pass@localhost:5432/test"),
            ("JWT_SECRET", VALID_SECRET),
            ("JWT_ACCESS_TTL_SECS", "0"),
        ];

        let error = AppConfig::from_lookup(lookup(&pairs)).expect_err("TTL zero");

        assert!(matches!(
            error,
            ConfigError::Invalid {
                key: "JWT_ACCESS_TTL_SECS",
                ..
            }
        ));
    }

    /// Verifica o contrato identificado por `accepts_hex_password_in_url` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn accepts_hex_password_in_url() {
        let pairs = [
            (
                "DATABASE_URL",
                "postgres://user:0f1e2d3c4b5a@localhost:5432/app",
            ),
            ("JWT_SECRET", VALID_SECRET),
        ];

        let config = AppConfig::from_lookup(lookup(&pairs)).expect("senha hexadecimal é válida");

        assert!(config.database_url.contains("0f1e2d3c4b5a"));
    }

    /// Verifica o contrato identificado por `rejects_base64_slash_in_database_password` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_base64_slash_in_database_password() {
        // `openssl rand -base64` produz `/`; numa URL ele encerra a autoridade
        // e a conexão iria para o host errado.
        let pairs = [
            (
                "DATABASE_URL",
                "postgres://user:ab/cd+ef=@localhost:5432/app",
            ),
            ("JWT_SECRET", VALID_SECRET),
        ];

        let error = AppConfig::from_lookup(lookup(&pairs)).expect_err("senha com `/` é inválida");

        assert!(matches!(
            error,
            ConfigError::Invalid {
                key: "DATABASE_URL",
                ..
            }
        ));
    }

    /// Verifica o contrato identificado por `rejects_base64_in_redis_password` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_base64_in_redis_password() {
        let pairs = [
            ("DATABASE_URL", "postgres://user:hex@localhost:5432/app"),
            ("JWT_SECRET", VALID_SECRET),
            ("REDIS_URL", "redis://:ab/cd@localhost:6379"),
        ];

        let error = AppConfig::from_lookup(lookup(&pairs)).expect_err("senha com `/` é inválida");

        assert!(matches!(
            error,
            ConfigError::Invalid {
                key: "REDIS_URL",
                ..
            }
        ));
    }

    /// Verifica o contrato identificado por `accepts_url_without_credentials` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn accepts_url_without_credentials() {
        let pairs = [
            ("DATABASE_URL", "postgres://localhost:5432/app"),
            ("JWT_SECRET", VALID_SECRET),
            ("REDIS_URL", "redis://localhost:6379"),
        ];

        AppConfig::from_lookup(lookup(&pairs)).expect("URL sem credencial é válida");
    }

    /// Verifica o contrato identificado por `rejects_sqlite_url_so_the_boot_fails_early` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_sqlite_url_so_the_boot_fails_early() {
        let pairs = [
            ("DATABASE_URL", "sqlite://data/api.db"),
            ("JWT_SECRET", VALID_SECRET),
        ];

        let error = AppConfig::from_lookup(lookup(&pairs)).expect_err("sqlite é recusado");

        assert!(matches!(
            error,
            ConfigError::Invalid {
                key: "DATABASE_URL",
                ..
            }
        ));
    }
}
