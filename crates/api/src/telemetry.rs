//! Configuração do `tracing` para logs legíveis em desenvolvimento e JSON em produção.
//!
//! O filtro de níveis é obtido de `RUST_LOG`, com defaults seguros para a API.

use anyhow::Result;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;

use crate::config::Environment;

/// Filtro de log usado quando `RUST_LOG` não está definido.
const DEFAULT_FILTER: &str = "info,api=debug,tower_http=info";

/// Inicializa o `tracing` com saída JSON em produção e legível em desenvolvimento.
///
/// O nível é controlado por `RUST_LOG`.
///
/// # Errors
///
/// Retorna erro se o filtro de log for inválido ou se o subscriber já tiver
/// sido inicializado.
pub fn init(environment: Environment) -> Result<()> {
    let filter =
        EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new(DEFAULT_FILTER))?;

    let builder = fmt().with_env_filter(filter).with_target(true);

    let init_result = match environment {
        Environment::Production => builder.json().try_init(),
        Environment::Development => builder.try_init(),
    };

    init_result.map_err(|error| anyhow::anyhow!("falha ao inicializar o tracing: {error}"))
}
