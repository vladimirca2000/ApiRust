//! Tradução de erros de constraint do PostgreSQL para o vocabulário da aplicação.
//!
//! **Por que não comparar o texto da mensagem.** No SQLite, o driver devolve
//! algo como `UNIQUE constraint failed: users.email`, e o código antigo
//! dependia desse texto. O PostgreSQL responde com o **nome da constraint**
//! (`duplicate key value violates unique constraint "users_email_active_unique"`)
//! e traduz a mensagem conforme o locale do servidor — comparar strings seria
//! frágil em duas frentes.
//!
//! A identificação usa o `SQLSTATE` (código estável, definido pela norma) e,
//! quando é preciso distinguir *qual* constraint falhou, o nome exato que as
//! migrations criam. Nome de constraint no projeto é sempre explícito
//! (`CREATE UNIQUE INDEX users_email_active_unique`), nunca gerado pelo banco.

use sqlx::error::DatabaseError;
use sqlx::{Error, postgres::PgDatabaseError};

/// `SQLSTATE` de violação de unicidade.
const UNIQUE_VIOLATION: &str = "23505";

/// `SQLSTATE` de violação de chave estrangeira.
const FOREIGN_KEY_VIOLATION: &str = "23503";

/// Indica se o erro é uma violação de unicidade na constraint informada.
///
/// Devolve `false` quando a violação é de outra constraint, o que evita
/// confundir e-mail duplicado com SKU duplicado na mesma transação.
#[must_use]
pub fn is_unique_violation_on(error: &Error, constraint: &str) -> bool {
    constraint_name(error, UNIQUE_VIOLATION).is_some_and(|name| name == constraint)
}

/// Indica se o erro é violação de qualquer chave estrangeira.
///
/// Usado onde só existe uma FK possível (produto → categoria), então o nome da
/// constraint não acrescenta informação.
#[must_use]
pub fn is_foreign_key_violation(error: &Error) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.try_downcast_ref::<PgDatabaseError>())
        .is_some_and(|pg| pg.code() == FOREIGN_KEY_VIOLATION)
}

/// Extrai o nome da constraint quando o erro é do `SQLSTATE` esperado.
///
/// Devolve `None` para outro tipo de erro, erro sem constraint associada
/// (por exemplo, `NOT NULL`) ou erro que não vem do PostgreSQL.
fn constraint_name<'a>(error: &'a Error, expected_code: &str) -> Option<&'a str> {
    let database: &dyn DatabaseError = error.as_database_error()?;
    let pg = database.try_downcast_ref::<PgDatabaseError>()?;

    if pg.code() != expected_code {
        return None;
    }

    pg.constraint()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifica o contrato identificado por `non_database_error_is_never_a_violation` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn non_database_error_is_never_a_violation() {
        let error = Error::RowNotFound;
        assert!(!is_unique_violation_on(&error, "users_email_active_unique"));
        assert!(!is_foreign_key_violation(&error));
    }

    /// Verifica o contrato identificado por `sqlstate_constants_match_postgres_documentation` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn sqlstate_constants_match_postgres_documentation() {
        // Valores fixos da norma SQL: trocá-los silenciosamente quebraria a
        // tradução de erro sem nenhum teste falhar.
        assert_eq!(UNIQUE_VIOLATION, "23505");
        assert_eq!(FOREIGN_KEY_VIOLATION, "23503");
    }
}
