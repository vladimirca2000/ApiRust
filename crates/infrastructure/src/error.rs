use thiserror::Error;

/// Falhas ao preparar ou acessar o banco de dados.
#[derive(Debug, Error)]
pub enum DbError {
    /// Falha devolvida pelo SQLx ao conectar ou executar uma consulta.
    #[error("erro de banco de dados: {0}")]
    Sqlx(#[from] sqlx::Error),

    /// Falha ao aplicar ou validar as migrations embutidas.
    #[error("erro ao aplicar migrations: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    /// Falha de sistema de arquivos ao criar o diretório do banco.
    #[error("erro de I/O ao preparar o banco: {0}")]
    Io(#[from] std::io::Error),
}
