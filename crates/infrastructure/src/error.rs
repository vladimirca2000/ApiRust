//! Erros de baixo nível de banco de dados expostos pelos adaptadores.
//!
//! Preserva a distinção entre falhas SQL e regras de domínio para tradução posterior.

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

    /// Nome de schema recusado pela validação (vazio, longo demais ou com
    /// caractere fora de `[a-z0-9_]`).
    #[error("nome de schema inválido: {0}")]
    InvalidSchema(String),
}
