//! Binário da API: carrega configuração, prepara banco e serve o roteador HTTP.
//!
//! A ordem do `main` é intencional: nada abre porta antes de a configuração
//! estar validada e as migrations aplicadas.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Context;
use api::config::AppConfig;
use api::operation::force_after_drain;
use api::{AppState, telemetry};
use application::{EnsureAdmin, SeedOutcome};
use chrono::Utc;
use infrastructure::{Argon2PasswordHasher, Database, SqlxUserRepository};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = AppConfig::from_env()?;
    telemetry::init(config.environment)?;

    let database = Database::connect(&config.database_url).await?;
    if config.run_migrations {
        database.migrate().await?;
        tracing::info!("migrations aplicadas");
    }

    seed_admin(&config, &database).await?;

    if config.redis_url.is_some() {
        tracing::info!("Redis configurado; rate limit e revogação distribuídos permanecem adiados");
    }

    let drain = Duration::from_secs(config.shutdown_drain_secs);
    let state = AppState::new(config.shutdown_on_tab_close, database.clone())
        .with_jwt(config.jwt_secret.expose(), config.jwt_access_ttl_secs)?
        .with_api_docs(config.expose_api_docs)
        .with_allowed_origins(config.allowed_origins.clone())
        .with_max_concurrent_requests(config.max_concurrent_requests);
    if state.sessions.is_enabled() {
        tracing::info!("modo desenvolvimento: fechar a aba encerra o servidor");
    }
    if state.expose_api_docs {
        tracing::info!("documentação interativa habilitada em /swagger-ui");
    } else {
        tracing::info!("documentação interativa desabilitada");
    }
    if state.allowed_origins.is_empty() {
        tracing::info!("CORS: nenhuma origem cruzada permitida");
    } else {
        tracing::info!(origins = ?state.allowed_origins, "CORS: origens permitidas");
    }

    let address = SocketAddr::new(config.host, config.port);
    let listener = TcpListener::bind(address).await?;
    tracing::info!(
        %address,
        max_concurrent = config.max_concurrent_requests,
        drain_secs = config.shutdown_drain_secs,
        "servidor iniciado"
    );

    // Um único sinal alimenta o shutdown gracioso e o timeout de drain: o
    // `SessionTracker` só entrega a notificação para um waiter.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let state_for_signal = state.clone();
    tokio::spawn(async move {
        shutdown_signal(state_for_signal).await;
        let _ = shutdown_tx.send(true);
    });

    let mut graceful_rx = shutdown_rx.clone();
    let graceful = async move {
        let _ = graceful_rx.wait_for(|flag| *flag).await;
    };

    let force = force_after_drain(
        async move {
            let mut rx = shutdown_rx;
            let _ = rx.wait_for(|flag| *flag).await;
        },
        drain,
    );

    // `into_make_service_with_connect_info` expõe o IP da conexão, usado pelo
    // rate limit de login/refresh para separar tentativas por origem.
    tokio::select! {
        result = axum::serve(
            listener,
            api::router(state.clone()).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(graceful) => result?,
        () = force => {
            tracing::warn!(
                drain_secs = config.shutdown_drain_secs,
                "timeout de drain; encerrando conexões restantes"
            );
        }
    }

    database.close().await;
    tracing::info!("servidor encerrado");
    Ok(())
}

/// Provisiona o Admin inicial quando `ADMIN_EMAIL` e `ADMIN_PASSWORD` existem.
///
/// Nunca registra e-mail, senha ou hash; apenas o resultado da operação.
async fn seed_admin(config: &AppConfig, database: &Database) -> anyhow::Result<()> {
    let Some(admin) = &config.admin_seed else {
        return Ok(());
    };

    let repository = SqlxUserRepository::new(database.pool().clone());
    let hasher = Argon2PasswordHasher::new();

    let outcome = EnsureAdmin::new(&repository, &hasher)
        .execute(admin.email(), admin.password(), Utc::now())
        .await
        .context("falha ao provisionar o admin inicial")?;

    match outcome {
        SeedOutcome::Created(_) => tracing::info!("admin inicial criado"),
        SeedOutcome::AlreadyExists(_) => {
            tracing::info!("admin inicial já existe; nenhuma alteração feita");
        }
    }

    Ok(())
}

/// Aguarda o primeiro sinal de encerramento: `Ctrl+C` ou o fechamento da aba.
///
/// Quando o modo de sessão está desabilitado, `wait_for_shutdown` nunca
/// resolve, então o `select!` fica efetivamente escutando apenas o `Ctrl+C`.
async fn shutdown_signal(state: AppState) {
    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            if let Err(error) = result {
                tracing::error!(%error, "falha ao registrar handler de shutdown");
            }
            tracing::info!("sinal de shutdown recebido");
        }
        () = state.sessions.wait_for_shutdown() => {
            tracing::info!("aba do navegador fechada; encerrando servidor");
        }
    }
}
