//! Gravação de auditoria **dentro** da transação de quem chamou.
//!
//! Existe para eliminar a janela em que a mutação confirma e a trilha falha: o
//! `INSERT` em `audit_log` compartilha a transação do produto/usuário/categoria,
//! então ou os dois entram, ou nenhum entra.
//!
//! Gravar a auditoria depois do `commit` — como era antes — permitia exatamente
//! o pior cenário para quem investiga: a alteração existe, o autor não.

use application::AppError;
use application::audit::AuditEntry;
use application::format_timestamp;

/// Insere uma entrada de auditoria na transação corrente.
///
/// # Errors
///
/// Retorna [`AppError::Infrastructure`] se o snapshot não puder ser serializado
/// ou se a inserção falhar. O chamador deve propagar: engolir o erro deixaria a
/// mutação confirmada sem trilha.
pub async fn insert_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    entry: &AuditEntry,
) -> Result<(), AppError> {
    let sql = "INSERT INTO audit_log (id, actor_id, action, entity, entity_id, before, after, at) \
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8)";

    let before = serialize(entry.before.as_ref())?;
    let after = serialize(entry.after.as_ref())?;

    sqlx::query(sql)
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(entry.actor_id.map(|id| id.to_string()))
        .bind(entry.action.as_str())
        .bind(entry.entity.as_str())
        .bind(&entry.entity_id)
        .bind(before)
        .bind(after)
        .bind(format_timestamp(entry.at))
        .execute(&mut **transaction)
        .await
        .map_err(AppError::infrastructure)?;

    Ok(())
}

/// Serializa um snapshot de auditoria para JSON.
///
/// # Errors
///
/// Retorna [`AppError::Infrastructure`] quando a serialização falha.
fn serialize(snapshot: Option<&serde_json::Value>) -> Result<Option<String>, AppError> {
    snapshot
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| AppError::infrastructure(std::io::Error::other(error.to_string())))
}
