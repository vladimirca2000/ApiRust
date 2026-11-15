use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use infrastructure::Database;

/// Banco SQLite temporário, migrado e removido ao final do teste.
pub struct TempDb {
    pub db: Database,
    path: PathBuf,
}

impl TempDb {
    pub async fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("relógio")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("apirust-{name}-{}-{nanos}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let db = Database::connect_path(&path).await.expect("conectar banco");
        db.migrate().await.expect("aplicar migrations");

        Self { db, path }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
