use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::error::DbError;

/// Número máximo de conexões simultâneas no pool de arquivo.
const DEFAULT_MAX_CONNECTIONS: u32 = 5;
/// Tempo que uma operação espera por conexão/lock antes de desistir.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Conexão com o banco SQLite, já configurada com foreign keys, WAL e timeout.
#[derive(Clone)]
pub struct Database {
    /// Pool compartilhado; clonar a struct clona o handle, não as conexões.
    pool: SqlitePool,
}

impl Database {
    /// Abre o banco a partir de uma URL SQLx (`sqlite://caminho/arquivo.db`).
    ///
    /// # Errors
    ///
    /// Retorna [`DbError`] se a URL for inválida, o diretório não puder ser
    /// criado ou a conexão falhar.
    pub async fn connect(database_url: &str) -> Result<Self, DbError> {
        let options = SqliteConnectOptions::from_str(database_url)?;
        Self::connect_with_options(options).await
    }

    /// Abre o banco a partir de um caminho de arquivo (ou `:memory:` em testes).
    ///
    /// # Errors
    ///
    /// Retorna [`DbError`] se o diretório não puder ser criado ou a conexão falhar.
    pub async fn connect_path(path: impl AsRef<Path>) -> Result<Self, DbError> {
        let options = SqliteConnectOptions::new().filename(path);
        Self::connect_with_options(options).await
    }

    /// Configura as opções de conexão e monta o pool.
    ///
    /// Aplica `foreign_keys=ON`, `busy_timeout` de 5 s e `synchronous=NORMAL` em
    /// qualquer modo; WAL e criação do diretório valem apenas para arquivo
    /// (bancos `:memory:` usam uma única conexão, pois cada conexão veria um
    /// banco diferente).
    ///
    /// # Errors
    ///
    /// Retorna [`DbError::Io`] se o diretório não puder ser criado e
    /// [`DbError::Sqlx`] se a conexão falhar.
    async fn connect_with_options(options: SqliteConnectOptions) -> Result<Self, DbError> {
        let is_memory = options.get_filename() == Path::new(":memory:");

        let options = options
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(BUSY_TIMEOUT)
            .synchronous(SqliteSynchronous::Normal);

        let options = if is_memory {
            options
        } else {
            if let Some(parent) = options.get_filename().parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            options.journal_mode(SqliteJournalMode::Wal)
        };

        let pool = SqlitePoolOptions::new()
            .max_connections(if is_memory {
                1
            } else {
                DEFAULT_MAX_CONNECTIONS
            })
            .min_connections(u32::from(is_memory))
            .acquire_timeout(BUSY_TIMEOUT)
            .connect_with(options)
            .await?;

        Ok(Self { pool })
    }

    /// Aplica as migrations embutidas no binário. É idempotente.
    ///
    /// # Errors
    ///
    /// Retorna [`DbError`] se alguma migration falhar.
    pub async fn migrate(&self) -> Result<(), DbError> {
        sqlx::migrate!("../../migrations").run(&self.pool).await?;
        Ok(())
    }

    /// Verifica se o banco responde.
    ///
    /// # Errors
    ///
    /// Retorna [`DbError`] se a consulta falhar.
    pub async fn ping(&self) -> Result<(), DbError> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    /// Empresta o pool para repositórios e testes executarem consultas diretas.
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Fecha o pool e aguarda as conexões em uso.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}
