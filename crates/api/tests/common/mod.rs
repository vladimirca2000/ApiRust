//! Helper dos testes HTTP: banco PostgreSQL isolado por schema.
//!
//! Substitui o `:memory:` do SQLite, que não existe no PostgreSQL. Cada teste
//! ganha um schema próprio, então a suíte roda em paralelo contra o mesmo
//! servidor sem que um teste veja as linhas do outro.
//!
//! **Por que não limpar no `Drop`.** O `Database` é movido para dentro do
//! `AppState` e os testes constroem o estado em uma função auxiliar, então a
//! guarda morreria no fim dessa função — removendo o schema que o teste ainda
//! vai usar. A limpeza é feita por varredura: ao iniciar, o processo remove os
//! schemas cujo PID **não existe mais**, o que é seguro mesmo com várias
//! execuções simultâneas.

use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use infrastructure::Database;

/// Contador para desambiguar schemas criados no mesmo instante.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Garante que a varredura de órfãos rode uma única vez por processo.
static CLEANUP: Once = Once::new();

/// Garante que a checagem de conectividade rode uma única vez por processo.
static REACHABLE: Once = Once::new();

/// Prefixo de todo schema criado por testes.
const SCHEMA_PREFIX: &str = "t_";

/// Cria um banco isolado, já migrado.
///
/// Na primeira chamada do processo, remove schemas órfãos de execuções
/// anteriores.
///
/// # Panics
///
/// Falha quando não há `TEST_DATABASE_URL`/`DATABASE_URL` ou quando a conexão,
/// a criação do schema ou as migrations falham. Um teste HTTP sem banco não
/// valida nada, então a falha é ruidosa de propósito.
pub async fn isolated_database(test_name: &str) -> Database {
    let url = database_url();

    // Checagem única por processo: distingue variável ausente de servidor fora.
    if !REACHABLE.is_completed() {
        let mut marked = false;
        REACHABLE.call_once(|| marked = true);
        if marked {
            ensure_server_reachable(&url).await;
        }
    }

    // Limpeza única por processo: roda dentro do runtime de teste.
    if !CLEANUP.is_completed() {
        let mut marked = false;
        CLEANUP.call_once(|| marked = true);
        if marked {
            cleanup_orphan_schemas(&url).await;
        }
    }

    let schema = schema_name(test_name);

    let database = Database::connect_isolated(&url, &schema)
        .await
        .unwrap_or_else(|error| panic!("não foi possível preparar o schema {schema}: {error}"));

    database
        .migrate()
        .await
        .unwrap_or_else(|error| panic!("falha ao aplicar migrations em {schema}: {error}"));

    database
}

/// Devolve a URL de teste, priorizando `TEST_DATABASE_URL`.
///
/// # Panics
///
/// Falha quando nenhuma variável está definida. A mensagem cita a variável —
/// não a conexão — porque a causa é configuração ausente, não servidor fora.
fn database_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| {
            panic!(
                "nenhuma URL de banco para os testes: defina TEST_DATABASE_URL \
                 (ou DATABASE_URL) apontando para um PostgreSQL acessível; \
                 ex.: postgres://usuario:senha@127.0.0.1:5432/banco"
            )
        })
}

/// Confirma que o servidor responde antes de tentar criar schema.
///
/// Separa **configuração ausente** de **servidor inacessível**: sem esta
/// checagem, a falha de conexão aparecia como um erro genérico de schema, e quem
/// já tinha definido `TEST_DATABASE_URL` era induzido a procurar o problema no
/// lugar errado.
///
/// # Panics
///
/// Falha com mensagem de conexão quando o servidor não responde, citando host e
/// porta para o diagnóstico.
async fn ensure_server_reachable(url: &str) {
    use sqlx::Connection;

    match sqlx::PgConnection::connect(url).await {
        Ok(connection) => {
            let _ = connection.close().await;
        }
        Err(error) => {
            let target = url
                .rsplit_once('@')
                .map_or(url, |(_, host)| host)
                .to_owned();

            panic!(
                "PostgreSQL inacessível em {target}: {error}. \
                 Verifique se o servidor está no ar e se a URL em \
                 TEST_DATABASE_URL/DATABASE_URL aponta para ele."
            );
        }
    }
}

/// Remove schemas de testes cujo processo dono já não existe.
///
/// O nome carrega o PID (`t_<prefixo>_<pid>_<nanos>_<seq>`). Consultar
/// `/proc/<pid>` diz se a execução ainda está viva; remover só os mortos evita
/// apagar o schema de uma suíte concorrente. Falhas aqui são registradas no
/// stderr e não interrompem os testes — limpeza é higiene, não pré-requisito.
async fn cleanup_orphan_schemas(url: &str) {
    use sqlx::{Connection, Row};

    let Ok(connection) = sqlx::PgConnection::connect(url).await else {
        return;
    };
    let mut connection = connection;

    let rows = sqlx::query(
        "SELECT schema_name FROM information_schema.schemata WHERE schema_name LIKE 't\\_%'",
    )
    .fetch_all(&mut connection)
    .await;

    let Ok(rows) = rows else {
        let _ = connection.close().await;
        return;
    };

    for row in rows {
        let Ok(schema): Result<String, _> = row.try_get("schema_name") else {
            continue;
        };

        let Some(pid) = owner_pid(&schema) else {
            continue;
        };

        // `/proc/<pid>` existe enquanto o processo vive; ausência = órfão.
        if std::path::Path::new(&format!("/proc/{pid}")).exists() {
            continue;
        }

        let statement = format!("DROP SCHEMA IF EXISTS \"{schema}\" CASCADE");
        if let Err(error) = sqlx::query(&statement).execute(&mut connection).await {
            eprintln!("falha ao remover schema órfão {schema}: {error}");
        }
    }

    let _ = connection.close().await;
}

/// Extrai o PID do nome do schema, quando o formato é reconhecido.
fn owner_pid(schema: &str) -> Option<u32> {
    // `t_<prefixo>_<pid>_<nanos>_<seq>`: o PID é o penúltimo segmento antes de
    // nanos e seq, então basta ler de trás para frente.
    let parts: Vec<&str> = schema.split('_').collect();
    if parts.len() < 4 {
        return None;
    }

    parts.get(parts.len() - 3)?.parse().ok()
}

/// Monta um nome de schema único e aceito por `[a-z0-9_]`.
fn schema_name(test_name: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("relógio do sistema")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);

    // Identificador do PostgreSQL tem limite de 63 bytes: o prefixo é truncado.
    let prefix: String = test_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .take(16)
        .collect();

    format!(
        "{SCHEMA_PREFIX}{prefix}_{}_{nanos}_{sequence}",
        std::process::id()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifica o contrato identificado por `extracts_pid_from_schema_name` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn extracts_pid_from_schema_name() {
        assert_eq!(owner_pid("t_users_1234_99_0"), Some(1234));
        assert_eq!(owner_pid("t_a_1_2_3"), Some(1));
    }

    /// Verifica o contrato identificado por `rejects_names_without_pid` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn rejects_names_without_pid() {
        assert_eq!(owner_pid("public"), None);
        assert_eq!(owner_pid("t_abc"), None);
    }
}
