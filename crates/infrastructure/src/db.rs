//! Conexão com o PostgreSQL e aplicação das migrations.
//!
//! O banco é único e compartilhado, então o isolamento entre testes **não** vem
//! de um banco por processo (como no `:memory:` do SQLite), mas de um `schema`
//! próprio por execução. Cada instância ajusta o `search_path` na conexão em vez
//! de qualificar nome a nome: o SQL dos repositórios permanece sem prefixo.
//!
//! O `schema` é criado no primeiro uso e removido por [`Database::close`]. Um
//! teste que panic antes de fechar deixa resíduo — por isso os nomes são únicos
//! e há [`Database::drop_schema`] para limpeza explícita.

use std::time::Duration;

use sqlx::Executor;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use sqlx::{Connection, PgConnection};

use crate::error::DbError;

/// Número máximo de conexões simultâneas no pool.
///
/// O PostgreSQL lida com concorrência muito melhor que o SQLite, mas um teto
/// explícito evita que uma réplica sozinha esgote o `max_connections` do
/// servidor compartilhado.
const DEFAULT_MAX_CONNECTIONS: u32 = 10;

/// Tempo que uma operação espera por conexão antes de desistir.
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

/// Conexão com o PostgreSQL, opcionalmente restrita a um schema.
#[derive(Clone)]
pub struct Database {
    /// Pool compartilhado; clonar a struct clona o handle, não as conexões.
    pool: PgPool,
    /// Schema usado por esta instância, quando aplicável.
    schema: Option<String>,
}

impl Database {
    /// Abre o pool a partir de uma URL (`postgres://usuario:senha@host:porta/banco`).
    ///
    /// Usa o `search_path` padrão do usuário (normalmente `public`).
    ///
    /// # Errors
    ///
    /// Retorna [`DbError`] se a URL for inválida ou a conexão falhar.
    pub async fn connect(database_url: &str) -> Result<Self, DbError> {
        let options = database_url.parse::<PgConnectOptions>()?;
        Self::connect_with_options(options, None).await
    }

    /// Abre o pool em um schema dedicado, criando-o se necessário.
    ///
    /// É o modo usado pelos testes: cada execução ganha um schema próprio, com
    /// as migrations aplicadas e descartado no fim. Isso evita que dois testes
    /// concorrentes vejam as linhas um do outro.
    ///
    /// # Errors
    ///
    /// Retorna [`DbError`] se a conexão, a criação do schema ou a troca de
    /// `search_path` falharem.
    pub async fn connect_isolated(database_url: &str, schema: &str) -> Result<Self, DbError> {
        validate_schema_name(schema)?;

        let options = database_url.parse::<PgConnectOptions>()?;
        Self::connect_with_options(options, Some(schema.to_owned())).await
    }

    /// Configura o pool e, quando houver schema, o cria e o fixa no `search_path`.
    ///
    /// O `search_path` entra nas **opções de conexão**, não em um `SET` avulso:
    /// um `SET` valeria apenas para a conexão que o executou, e o pool abriria
    /// conexões novas apontando para o schema padrão.
    ///
    /// # Errors
    ///
    /// Retorna [`DbError::Sqlx`] quando a conexão falha e
    /// [`DbError::InvalidSchema`] para nome fora do formato aceito.
    async fn connect_with_options(
        options: PgConnectOptions,
        schema: Option<String>,
    ) -> Result<Self, DbError> {
        if let Some(name) = schema.as_deref() {
            ensure_schema(options.clone(), name).await?;
        }

        let options = match schema.as_deref() {
            Some(name) => options.options([("search_path", name)]),
            None => options,
        };

        let pool = PgPoolOptions::new()
            .max_connections(DEFAULT_MAX_CONNECTIONS)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .connect_with(options)
            .await?;

        Ok(Self { pool, schema })
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
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Devolve o schema usado por esta instância, quando isolada.
    #[must_use]
    pub fn schema(&self) -> Option<&str> {
        self.schema.as_deref()
    }

    /// Fecha o pool, sem tocar em nenhum schema.
    ///
    /// **Nunca remove schema.** Um `DROP SCHEMA ... CASCADE` aqui apagaria as
    /// tabelas da aplicação a cada encerramento — inclusive em produção, onde
    /// `DATABASE_SCHEMA` aponta para o schema real. A remoção de schema de
    /// teste é responsabilidade explícita de [`Database::drop_schema`].
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Remove um schema pelo nome, usando uma conexão dedicada.
    ///
    /// Chamada **apenas** pelo código de teste, que cria schemas descartáveis.
    /// Não existe caminho no boot ou no shutdown da aplicação que chegue aqui:
    /// a limpeza é explícita para não confundir "encerrar" com "apagar dados".
    ///
    /// # Errors
    ///
    /// Retorna [`DbError`] se a conexão ou o `DROP` falharem.
    pub async fn drop_schema(&self, schema: &str) -> Result<(), DbError> {
        validate_schema_name(schema)?;

        let mut connection = PgConnection::connect_with(&self.pool.connect_options()).await?;
        let statement = format!("DROP SCHEMA IF EXISTS \"{schema}\" CASCADE");
        connection.execute(statement.as_str()).await?;
        connection.close().await?;

        Ok(())
    }
}

/// Garante que o schema existe antes de o pool apontar para ele.
///
/// # Errors
///
/// Retorna [`DbError::Sqlx`] se a criação falhar.
async fn ensure_schema(options: PgConnectOptions, schema: &str) -> Result<(), DbError> {
    validate_schema_name(schema)?;

    let mut connection = PgConnection::connect_with(&options).await?;
    let statement = format!("CREATE SCHEMA IF NOT EXISTS \"{schema}\"");
    connection.execute(statement.as_str()).await?;
    connection.close().await?;

    Ok(())
}

/// Aceita apenas nomes de schema seguros para interpolação.
///
/// O nome vai dentro de aspas duplas em `CREATE`/`DROP SCHEMA`, então a
/// validação impede que um valor com aspas ou ponto e vírgula escape do
/// comando. Como o nome vem do código de teste, não do cliente, a checagem é
/// defesa em profundidade — mas `DROP SCHEMA ... CASCADE` não é o lugar de
/// confiar em disciplina.
///
/// # Errors
///
/// Retorna [`DbError::InvalidSchema`] para vazio, longo demais ou com caractere
/// fora de `[a-z0-9_]`.
fn validate_schema_name(schema: &str) -> Result<(), DbError> {
    /// Comprimento máximo aceito para um nome de schema.
    ///
    /// O PostgreSQL limita identificadores a 63 bytes; a margem cobre o prefixo
    /// gerado pelos testes e mantém folga para uso manual.
    const MAX_LEN: usize = 63;

    let valid = !schema.is_empty()
        && schema.len() <= MAX_LEN
        && schema.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
        && schema
            .starts_with(|character: char| character.is_ascii_alphabetic() || character == '_');

    if valid {
        Ok(())
    } else {
        Err(DbError::InvalidSchema(schema.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifica o contrato identificado por `accepts_safe_schema_names` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn accepts_safe_schema_names() {
        assert!(validate_schema_name("api_test_1").is_ok());
        assert!(validate_schema_name("_privado").is_ok());
    }

    /// Verifica o contrato identificado por `rejects_unsafe_schema_names` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_unsafe_schema_names() {
        for name in [
            "",
            "Public",
            "a-b",
            "a b",
            "a;b",
            "a\"b",
            "1abc",
            "schema.public",
        ] {
            assert!(
                validate_schema_name(name).is_err(),
                "nome {name:?} deveria ser recusado"
            );
        }
    }

    /// Verifica o contrato identificado por `rejects_schema_name_that_is_too_long` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_schema_name_that_is_too_long() {
        assert!(validate_schema_name(&"a".repeat(64)).is_err());
        assert!(validate_schema_name(&"a".repeat(63)).is_ok());
    }
}
