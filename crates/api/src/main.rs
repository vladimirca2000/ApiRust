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
use infrastructure::{
    Argon2PasswordHasher, Database, RedisAccessTokenDenylist, RedisRateLimitStore, RedisState,
    SqlxUserRepository,
};
use tokio::net::TcpListener;
/// Carrega e valida a configuração, inicializa banco e dependências e inicia o servidor; retorna erro se qualquer etapa de inicialização ou execução falhar.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = AppConfig::from_env()?;
    telemetry::init(config.environment)?;

    let database = match config.database_schema.as_deref() {
        Some(schema) => {
            tracing::info!(schema, "usando schema dedicado do PostgreSQL");
            Database::connect_isolated(&config.database_url, schema).await?
        }
        None => Database::connect(&config.database_url).await?,
    };
    if config.run_migrations {
        database.migrate().await?;
        tracing::info!("migrations aplicadas");
    }

    seed_admin(&config, &database).await?;

    let drain = Duration::from_secs(config.shutdown_drain_secs);
    let mut state = AppState::from_parts(config.shutdown_on_tab_close, database.clone())
        .with_jwt(config.jwt_secret.expose(), config.jwt_access_ttl_secs)?
        .with_api_docs(config.expose_api_docs)
        .with_allowed_origins(config.allowed_origins.clone())
        .with_max_concurrent_requests(config.max_concurrent_requests)
        .with_metrics(config.metrics_token.clone())
        .map_err(|error| anyhow::anyhow!("falha ao criar métricas: {error}"))?
        .with_trusted_proxies(config.trusted_proxies.clone());

    if config.trusted_proxies.is_empty() {
        tracing::info!("rate limit usa o IP do socket: TRUSTED_PROXIES não configurado");
    } else {
        tracing::info!(
            proxies = config.trusted_proxies.len(),
            "rate limit resolve o IP do cliente via X-Forwarded-For de proxies confiáveis"
        );
    }

    // Redis é o estado compartilhado de rate limit e revogação. Em produção é
    // obrigatório: subir sem ele significaria rate limit por réplica (limite
    // multiplicado) e logout que não invalida token nas outras instâncias.
    match &config.redis_url {
        Some(url) => {
            let redis = RedisState::connect(url)
                .await
                .context("falha ao conectar ao Redis configurado")?;
            redis
                .ping()
                .await
                .context("Redis configurado não respondeu ao PING")?;
            state.rate_limiter = std::sync::Arc::new(RedisRateLimitStore::new(redis.clone()));
            state.denylist = std::sync::Arc::new(RedisAccessTokenDenylist::new(redis));
            tracing::info!("Redis conectado: rate limit e revogação distribuídos ativos");
        }
        None => {
            if config.environment == api::config::Environment::Production {
                // Em produção, subir sem Redis significaria rate limit por
                // réplica e logout que não invalida token nas outras instâncias.
                anyhow::bail!(
                    "REDIS_URL é obrigatório em produção: sem ele, rate limit e revogação \
                     ficam por instância e o estado não é compartilhado"
                );
            }
            tracing::warn!(
                "Redis não configurado; rate limit e revogação ficam em memória (apenas uma instância)"
            );
        }
    }

    if state.session_metrics_enabled() {
        tracing::info!("métricas Prometheus habilitadas em /metrics");
    } else {
        tracing::info!("métricas desabilitadas: defina METRICS_TOKEN para habilitar /metrics");
    }
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

/// Aguarda o primeiro sinal de encerramento: `Ctrl+C`, `SIGTERM` ou fechamento da aba.
///
/// **SIGTERM é obrigatório em container.** O `docker stop` envia SIGTERM, não
/// SIGINT; sem registrá-lo, o processo morreria sem executar o drain e
/// requisições em voo seriam cortadas. Tratar os dois sinais pelo mesmo fluxo é
/// o que faz o `stop_grace_period` do compose ter efeito.
///
/// Quando o modo de sessão está desabilitado, `wait_for_shutdown` nunca
/// resolve, então o `select!` fica efetivamente escutando apenas os sinais.
async fn shutdown_signal(state: AppState) {
    tokio::select! {
        signal = terminate_signal() => {
            tracing::info!(signal, "sinal de encerramento recebido");
        }
        () = state.sessions.wait_for_shutdown() => {
            tracing::info!("aba do navegador fechada; encerrando servidor");
        }
    }
}

/// Resolve no primeiro sinal de encerramento, devolvendo o nome do sinal.
///
/// `ctrl_c` cobre SIGINT e, em Unix, também SIGTERM? Não: são sinais distintos.
/// Por isso os dois são registrados e o primeiro que chegar vence.
async fn terminate_signal() -> &'static str {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        // Registro pode falhar em ambiente exótico; nesse caso o SIGINT ainda
        // funciona, e o erro fica no log em vez de derrubar o servidor.
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(stream) => stream,
            Err(error) => {
                tracing::error!(%error, "falha ao registrar handler de SIGTERM");
                return match tokio::signal::ctrl_c().await {
                    Ok(()) => "SIGINT",
                    Err(error) => {
                        tracing::error!(%error, "falha no handler de Ctrl+C");
                        "desconhecido"
                    }
                };
            }
        };

        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    tracing::error!(%error, "falha no handler de Ctrl+C");
                }
                "SIGINT"
            }
            _ = terminate.recv() => "SIGTERM",
        }
    }

    #[cfg(not(unix))]
    {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "falha no handler de Ctrl+C");
        }
        "SIGINT"
    }
}
