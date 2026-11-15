//! Helper compartilhado pelos testes de integração: banco isolado em PostgreSQL.
//!
//! Cada teste recebe um **schema** próprio, criado no primeiro uso e removido
//! em [`TempDb`] quando o teste termina. Isso permite rodar os testes em
//! paralelo contra o mesmo servidor sem que um veja as linhas do outro — o
//! equivalente ao antigo `:memory:` do SQLite, que não existe no PostgreSQL.
//!
//! A URL vem de `TEST_DATABASE_URL` e, na ausência dela, de `DATABASE_URL` (o
//! `.env` de desenvolvimento). Sem nenhuma das duas, o teste falha com mensagem
//! explícita em vez de passar em silêncio: um teste que "passa" sem tocar no
//! banco esconde justamente o defeito que ele deveria pegar.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use infrastructure::Database;

/// Contador para desambiguar schemas criados no mesmo nanossegundo.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Banco PostgreSQL isolado em um schema exclusivo.
///
/// O schema é removido quando a struct sai de escopo, inclusive em panic, para
/// não deixar resíduo no servidor compartilhado.
pub struct TempDb {
    /// Banco já migrado e restrito ao schema deste teste.
    pub db: Database,
}

impl TempDb {
    /// Cria um schema novo, aplica as migrations e devolve o banco.
    ///
    /// # Panics
    ///
    /// Falha se não houver `TEST_DATABASE_URL`/`DATABASE_URL` ou se a conexão,
    /// criação do schema ou migrations falharem. Um teste de integração sem
    /// banco não tem como validar nada, então a falha é ruidosa de propósito.
    pub async fn new(name: &str) -> Self {
        let url = database_url();
        let schema = schema_name(name);

        let db = Database::connect_isolated(&url, &schema)
            .await
            .unwrap_or_else(|error| panic!("não foi possível preparar o schema {schema}: {error}"));

        db.migrate()
            .await
            .unwrap_or_else(|error| panic!("falha ao aplicar migrations em {schema}: {error}"));

        Self { db }
    }
}

impl Drop for TempDb {
    /// Remove o schema ao fim do teste, inclusive quando o teste falha.
    ///
    /// `Database::close` é `async` e não pode ser chamado em `Drop`, então o
    /// `DROP` roda em uma thread dedicada com um runtime mínimo. Sem isso, cada
    /// execução deixaria um schema órfão no servidor compartilhado — e eles se
    /// acumulam silenciosamente.
    fn drop(&mut self) {
        let Some(schema) = self.db.schema().map(str::to_owned) else {
            return;
        };

        let options = self.db.pool().connect_options();

        let handle = std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    eprintln!("não foi possível criar runtime para limpar {schema}: {error}");
                    return;
                }
            };

            runtime.block_on(async move {
                use sqlx::Connection;

                match sqlx::PgConnection::connect_with(&options).await {
                    Ok(mut connection) => {
                        let statement = format!("DROP SCHEMA IF EXISTS \"{schema}\" CASCADE");
                        if let Err(error) = sqlx::query(&statement).execute(&mut connection).await {
                            eprintln!("falha ao remover schema {schema}: {error}");
                        }
                        let _ = connection.close().await;
                    }
                    Err(error) => {
                        eprintln!("falha ao conectar para remover {schema}: {error}");
                    }
                }
            });
        });

        // Espera a limpeza: o teste não pode terminar antes do `DROP`.
        let _ = handle.join();
    }
}

/// Devolve a URL de teste, priorizando `TEST_DATABASE_URL`.
///
/// # Panics
///
/// Falha quando nenhuma variável está definida.
fn database_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| {
            panic!(
                "defina TEST_DATABASE_URL (ou DATABASE_URL) apontando para um PostgreSQL; \
                 ex.: postgres://usuario:senha@127.0.0.1:5432/banco"
            )
        })
}

/// Monta um nome de schema único, aceito por `[a-z0-9_]`.
///
/// Usa o prefixo do teste, o PID, o instante em nanos e um contador: PID separa
/// execuções concorrentes do `cargo test`, e o contador separa testes do mesmo
/// processo.
fn schema_name(name: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("relógio do sistema")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);

    // O PostgreSQL limita identificadores a 63 bytes; o prefixo entra truncado.
    let prefix: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .take(12)
        .collect();

    format!("t_{prefix}_{}_{nanos}_{sequence}", std::process::id())
}
