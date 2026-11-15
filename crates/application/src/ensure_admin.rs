//! Caso de uso e dublês de teste para provisionar o administrador inicial.
//!
//! O provisionamento é idempotente: não duplica contas existentes e converte
//! corridas de criação em um resultado previsível.

use chrono::{DateTime, Utc};
use domain::{Email, Role, User, UserId, validate_password};

use crate::error::AppError;
use crate::ports::{PasswordHasher, UserRepository};

/// Nome atribuído ao Admin criado pelo seed.
pub const ADMIN_NAME: &str = "Administrador";

/// Resultado do provisionamento do Admin inicial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedOutcome {
    /// O Admin não existia e foi criado.
    Created(UserId),
    /// Já existia um usuário ativo com o e-mail informado.
    AlreadyExists(UserId),
}

/// Provisiona o primeiro Admin de forma idempotente.
///
/// A senha em texto puro só existe em memória durante o hashing; o usuário é
/// persistido apenas com o hash Argon2id.
pub struct EnsureAdmin<'a> {
    /// Repositório consultado antes de inserir e usado para persistir o Admin.
    users: &'a dyn UserRepository,
    /// Responsável por transformar a senha em hash Argon2id.
    hasher: &'a dyn PasswordHasher,
}

impl<'a> EnsureAdmin<'a> {
    /// Monta o caso de uso com as dependências injetadas.
    ///
    /// Recebe referências (não ownership) para que o bootstrap reuse as mesmas
    /// instâncias em outros casos de uso.
    #[must_use]
    pub fn new(users: &'a dyn UserRepository, hasher: &'a dyn PasswordHasher) -> Self {
        Self { users, hasher }
    }

    /// Cria o Admin quando o e-mail ainda não está em uso.
    ///
    /// # Errors
    ///
    /// Retorna [`AppError::Domain`] quando a senha não atende à política de
    /// domínio e [`AppError::Infrastructure`] quando hashing ou persistência
    /// falham.
    pub async fn execute(
        &self,
        email: &Email,
        password: &str,
        now: DateTime<Utc>,
    ) -> Result<SeedOutcome, AppError> {
        validate_password(password)?;

        if let Some(existing) = self.users.find_by_email(email).await? {
            return Ok(SeedOutcome::AlreadyExists(existing.id()));
        }

        let password_hash = self.hasher.hash(password).await?;
        let admin = User::new(ADMIN_NAME, email.clone(), password_hash, Role::Admin, now)?;

        match self.users.insert(&admin, None).await {
            Ok(()) => Ok(SeedOutcome::Created(admin.id())),
            // Corrida entre a consulta e o insert: o índice parcial garante que
            // apenas um usuário ativo fique com o e-mail.
            Err(AppError::EmailAlreadyInUse) => {
                let existing = self
                    .users
                    .find_by_email(email)
                    .await?
                    .ok_or(AppError::EmailAlreadyInUse)?;

                Ok(SeedOutcome::AlreadyExists(existing.id()))
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::audit::AuditEntry;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use async_trait::async_trait;
    use domain::{EntityStatus, PasswordHash};

    use super::*;

    /// Hash Argon2id fixo e válido usado pelos dublês dos testes de provisionamento.
    const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdGUtZGV0ZXN0ZQ$aGFzaC1kZS10ZXN0ZQ";
    /// Retorna um instante UTC fixo para tornar as verificações determinísticas.
    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-14T12:00:00Z")
            .expect("data válida")
            .with_timezone(&Utc)
    }
    /// Cria um e-mail válido e fixo para os cenários de teste; retorna o value object correspondente.
    fn email() -> Email {
        Email::parse("admin@example.com").expect("e-mail válido")
    }
    /// Cria um hash Argon2id de fixture para os cenários de teste e retorna `PasswordHash`.
    fn password_hash() -> PasswordHash {
        PasswordHash::parse(HASH).expect("hash válido")
    }
    /// Monta e retorna um usuário Admin existente para exercitar o provisionamento idempotente.
    fn existing_admin() -> User {
        User::new(
            "Administrador",
            email(),
            password_hash(),
            Role::Admin,
            now(),
        )
        .expect("usuário válido")
    }

    #[derive(Default)]
    /// Repositório em memória para testar o provisionamento sem banco de dados.
    struct FakeUsers {
        /// Usuários mantidos pelo dublê durante cada cenário.
        users: Mutex<Vec<User>>,
    }

    #[async_trait]
    impl UserRepository for FakeUsers {
        /// Busca no estado em memória do dublê e retorna `Some` quando o ID coincide, `None` caso contrário.
        async fn find_by_id(&self, id: UserId) -> Result<Option<User>, AppError> {
            Ok(self
                .users
                .lock()
                .expect("lock")
                .iter()
                .find(|user| user.id() == id)
                .cloned())
        }
        /// Busca no estado em memória do dublê e retorna `Some` quando o e-mail coincide, `None` caso contrário.
        async fn find_by_email(&self, email: &Email) -> Result<Option<User>, AppError> {
            Ok(self
                .users
                .lock()
                .expect("lock")
                .iter()
                .find(|user| user.email() == email)
                .cloned())
        }
        /// Adiciona o registro ao estado em memória do dublê e retorna sucesso; os dublês de corrida podem devolver o conflito configurado.
        async fn insert(&self, user: &User, _audit: Option<&AuditEntry>) -> Result<(), AppError> {
            self.users.lock().expect("lock").push(user.clone());
            Ok(())
        }
        /// Retorna do estado em memória a página solicitada pelo offset e limite.
        async fn list(&self, offset: i64, limit: i64) -> Result<Vec<User>, AppError> {
            Ok(self
                .users
                .lock()
                .expect("lock")
                .iter()
                .skip(offset as usize)
                .take(limit as usize)
                .cloned()
                .collect())
        }
        /// Retorna a quantidade de registros mantidos pelo dublê em memória.
        async fn count(&self) -> Result<i64, AppError> {
            Ok(self.users.lock().expect("lock").len() as i64)
        }
        /// Substitui o registro no estado em memória e retorna sucesso ou o erro de ausência definido pelo cenário.
        async fn update(&self, user: &User, _audit: Option<&AuditEntry>) -> Result<(), AppError> {
            let mut guard = self.users.lock().expect("lock");
            if let Some(slot) = guard.iter_mut().find(|existing| existing.id() == user.id()) {
                *slot = user.clone();
                return Ok(());
            }
            Err(AppError::UserNotFound)
        }
    }

    /// Hasher determinístico que conta chamadas e devolve o hash de fixture.
    struct FakeHasher {
        /// Quantidade de vezes que o caso de uso solicitou hashing.
        calls: AtomicUsize,
    }

    impl FakeHasher {
        /// Cria o dublê de hashing com contador de chamadas zerado e retorna a instância pronta para o teste.
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl PasswordHasher for FakeHasher {
        /// Retorna o hash de senha válido usado como fixture nos testes.
        async fn hash(&self, _password: &str) -> Result<PasswordHash, AppError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(password_hash())
        }
        /// Devolve o resultado de verificação configurado pelo dublê, sem executar hashing real.
        async fn verify(&self, _password: &str, _hash: &PasswordHash) -> Result<bool, AppError> {
            Ok(false)
        }
    }

    /// Hasher que simula falha de infraestrutura para verificar propagação de erro.
    struct FailingHasher;

    #[async_trait]
    impl PasswordHasher for FailingHasher {
        /// Simula uma falha ao gerar o hash e retorna erro de infraestrutura.
        async fn hash(&self, _password: &str) -> Result<PasswordHash, AppError> {
            Err(AppError::infrastructure(std::io::Error::other(
                "falha simulada no hashing",
            )))
        }
        /// Devolve o resultado de verificação configurado pelo dublê, sem executar hashing real.
        async fn verify(&self, _password: &str, _hash: &PasswordHash) -> Result<bool, AppError> {
            Ok(false)
        }
    }

    /// Repositório que simula uma corrida: a primeira consulta não encontra
    /// nada, o insert falha por e-mail duplicado e a segunda consulta encontra
    /// o usuário criado concorrentemente.
    struct RaceUsers {
        /// Usuário que representa a conta criada por outra execução concorrente.
        existing: User,
        /// Indica se a tentativa concorrente de inserção já ocorreu.
        attempted: AtomicBool,
    }

    #[async_trait]
    impl UserRepository for RaceUsers {
        /// Busca no estado em memória do dublê e retorna `Some` quando o ID coincide, `None` caso contrário.
        async fn find_by_id(&self, id: UserId) -> Result<Option<User>, AppError> {
            Ok((self.existing.id() == id).then(|| self.existing.clone()))
        }
        /// Busca no estado em memória do dublê e retorna `Some` quando o e-mail coincide, `None` caso contrário.
        async fn find_by_email(&self, email: &Email) -> Result<Option<User>, AppError> {
            if self.attempted.load(Ordering::SeqCst) && self.existing.email() == email {
                Ok(Some(self.existing.clone()))
            } else {
                Ok(None)
            }
        }
        /// Adiciona o registro ao estado em memória do dublê e retorna sucesso; os dublês de corrida podem devolver o conflito configurado.
        async fn insert(&self, _user: &User, _audit: Option<&AuditEntry>) -> Result<(), AppError> {
            self.attempted.store(true, Ordering::SeqCst);
            Err(AppError::EmailAlreadyInUse)
        }
        /// Retorna do estado em memória a página solicitada pelo offset e limite.
        async fn list(&self, _offset: i64, _limit: i64) -> Result<Vec<User>, AppError> {
            Ok(vec![self.existing.clone()])
        }
        /// Retorna a quantidade de registros mantidos pelo dublê em memória.
        async fn count(&self) -> Result<i64, AppError> {
            Ok(1)
        }
        /// Substitui o registro no estado em memória e retorna sucesso ou o erro de ausência definido pelo cenário.
        async fn update(&self, _user: &User, _audit: Option<&AuditEntry>) -> Result<(), AppError> {
            Ok(())
        }
    }

    /// Verifica o contrato identificado por `creates_admin_when_email_is_free` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[tokio::test]
    async fn creates_admin_when_email_is_free() {
        let users = FakeUsers::default();
        let hasher = FakeHasher::new();
        let use_case = EnsureAdmin::new(&users, &hasher);

        let outcome = use_case
            .execute(&email(), "Senha-Forte-2026!", now())
            .await
            .expect("criar admin");

        let created_id = match outcome {
            SeedOutcome::Created(id) => id,
            other => panic!("esperado Created, recebido {other:?}"),
        };
        assert_eq!(hasher.calls.load(Ordering::SeqCst), 1);

        let stored = users
            .find_by_id(created_id)
            .await
            .expect("consultar")
            .expect("admin persistido");
        assert_eq!(stored.email(), &email());
        assert_eq!(stored.role(), Role::Admin);
        assert_eq!(stored.status(), EntityStatus::Active);
        assert_eq!(stored.name(), ADMIN_NAME);
        assert_eq!(stored.password_hash(), &password_hash());
        assert_eq!(stored.created_at(), now());
    }

    /// Verifica o contrato identificado por `does_not_duplicate_existing_admin` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[tokio::test]
    async fn does_not_duplicate_existing_admin() {
        let users = FakeUsers::default();
        let existing = existing_admin();
        users.users.lock().expect("lock").push(existing.clone());

        let hasher = FakeHasher::new();
        let outcome = EnsureAdmin::new(&users, &hasher)
            .execute(&email(), "Senha-Forte-2026!", now())
            .await
            .expect("provisionar admin");

        assert_eq!(outcome, SeedOutcome::AlreadyExists(existing.id()));
        assert_eq!(hasher.calls.load(Ordering::SeqCst), 0);
        assert_eq!(users.users.lock().expect("lock").len(), 1);
    }

    /// Verifica o contrato identificado por `rejects_weak_password_before_hashing` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[tokio::test]
    async fn rejects_weak_password_before_hashing() {
        let users = FakeUsers::default();
        let hasher = FakeHasher::new();

        let error = EnsureAdmin::new(&users, &hasher)
            .execute(&email(), "fraca", now())
            .await
            .expect_err("senha fraca");

        assert!(matches!(
            error,
            AppError::Domain(domain::DomainError::InvalidPassword(_))
        ));
        assert_eq!(hasher.calls.load(Ordering::SeqCst), 0);
        assert!(users.users.lock().expect("lock").is_empty());
    }

    /// Verifica o contrato identificado por `converts_concurrent_insert_into_already_exists` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[tokio::test]
    async fn converts_concurrent_insert_into_already_exists() {
        let existing = existing_admin();
        let users = RaceUsers {
            existing: existing.clone(),
            attempted: AtomicBool::new(false),
        };
        let hasher = FakeHasher::new();

        let outcome = EnsureAdmin::new(&users, &hasher)
            .execute(&email(), "Senha-Forte-2026!", now())
            .await
            .expect("resolver corrida");

        assert_eq!(outcome, SeedOutcome::AlreadyExists(existing.id()));
    }

    /// Verifica o contrato identificado por `propagates_hasher_failure` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[tokio::test]
    async fn propagates_hasher_failure() {
        let users = FakeUsers::default();

        let error = EnsureAdmin::new(&users, &FailingHasher)
            .execute(&email(), "Senha-Forte-2026!", now())
            .await
            .expect_err("falha de hashing");

        assert!(matches!(error, AppError::Infrastructure(_)));
        assert!(users.users.lock().expect("lock").is_empty());
    }
}
