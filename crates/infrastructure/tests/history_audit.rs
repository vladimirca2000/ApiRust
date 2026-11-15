//! Testes de integração do histórico e da trilha de auditoria contra SQLite real.
//!
//! Cobrem o que os testes HTTP não alcançam: a atomicidade entre produto e
//! histórico, a reconstrução dos tipos de domínio e a ordem estável da listagem.

mod common;

use application::{
    AppError, AuditAction, AuditEntity, AuditEntry, AuditFilter, AuditRepository,
    CatalogHistoryRepository, CategoryRepository, PasswordHasher, ProductChange, ProductRepository,
    UserRepository,
};
use chrono::{DateTime, Duration, Utc};
use domain::{
    Category, EntityStatus, Money, PriceChange, Product, Sku, StockMovement, StockReason, UserId,
};
use infrastructure::{
    Database, SqlxAuditRepository, SqlxCatalogHistoryRepository, SqlxCategoryRepository,
    SqlxProductRepository,
};

/// Instante base dos testes.
fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-15T12:00:00Z")
        .expect("data válida")
        .with_timezone(&Utc)
}

/// Banco isolado; a guarda devolvida remove o schema no fim do teste.
///
/// O chamador precisa manter a guarda viva: `let temp = setup().await;` e usar
/// `&temp.db`. Movê-la para fora descartaria o banco.
async fn setup() -> common::TempDb {
    common::TempDb::new("history-audit").await
}

/// Cria uma categoria viva e devolve seu id.
async fn category(database: &Database) -> UserId {
    let repository = SqlxCategoryRepository::new(database.pool().clone());
    let category = Category::new("Eletrônicos", None, now()).expect("categoria");
    repository.insert(&category, None).await.expect("inserir");
    category.id()
}

/// Monta um produto válido ligado à categoria informada.
fn product(category_id: UserId, sku: &str, cents: i64, stock: i64) -> Product {
    Product::new(
        "Teclado",
        Sku::parse(sku).expect("sku"),
        None,
        Money::from_cents(cents).expect("preço"),
        stock,
        category_id,
        now(),
    )
    .expect("produto")
}

/// Verifica o contrato identificado por `inserting_a_product_with_history_is_atomic` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn inserting_a_product_with_history_is_atomic() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = setup().await;
    let database = &temp.db;
    let category_id = category(database).await;
    let history = SqlxCatalogHistoryRepository::new(database.pool().clone());
    let products = SqlxProductRepository::new(database.pool().clone());

    let product = product(category_id, "ATOM-1", 1000, 5);
    let movement = StockMovement::new(product.id(), 5, StockReason::Initial, None, now())
        .expect("movimentação");

    history
        .insert_product(&product, Some(&movement), None)
        .await
        .expect("inserir com histórico");

    let stored = products
        .find_by_id(product.id())
        .await
        .expect("consultar")
        .expect("produto existe");
    assert_eq!(stored.stock(), 5);

    let movements = history
        .stock_movements(product.id(), 0, 10)
        .await
        .expect("listar");
    assert_eq!(movements.len(), 1);
    assert_eq!(movements[0].delta, 5);
    assert_eq!(movements[0].reason, StockReason::Initial);
}

/// Verifica o contrato identificado por `duplicate_sku_rolls_back_the_whole_transaction` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn duplicate_sku_rolls_back_the_whole_transaction() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = setup().await;
    let database = &temp.db;
    let category_id = category(database).await;
    let history = SqlxCatalogHistoryRepository::new(database.pool().clone());

    let first = product(category_id, "ATOM-2", 1000, 1);
    history
        .insert_product(&first, None, None)
        .await
        .expect("primeiro");

    // Mesmo SKU: a transação inteira precisa falhar, inclusive o histórico.
    let second = product(category_id, "ATOM-2", 2000, 9);
    let movement =
        StockMovement::new(second.id(), 9, StockReason::Initial, None, now()).expect("movimento");

    let result = history.insert_product(&second, Some(&movement), None).await;
    assert!(result.is_err(), "SKU duplicado deve falhar");

    let movements = history
        .stock_movements(second.id(), 0, 10)
        .await
        .expect("listar");
    assert!(
        movements.is_empty(),
        "nada do produto rejeitado pode ter sido gravado"
    );
}

/// Verifica o contrato identificado por `updating_a_product_records_stock_and_price_history` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn updating_a_product_records_stock_and_price_history() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = setup().await;
    let database = &temp.db;
    let category_id = category(database).await;
    let history = SqlxCatalogHistoryRepository::new(database.pool().clone());

    let mut product = product(category_id, "ATOM-3", 1000, 10);
    history
        .insert_product(&product, None, None)
        .await
        .expect("inserir");

    let later = now() + Duration::minutes(5);
    product.set_stock(6, later).expect("reduzir estoque");
    product
        .change_price(Money::from_cents(1500).expect("preço"), later)
        .expect("mudar preço");

    let movement =
        StockMovement::new(product.id(), -4, StockReason::Sale, None, later).expect("movimento");
    let price = PriceChange::new(product.id(), 1000, 1500, None, later).expect("preço");

    let change = ProductChange {
        movement: Some(&movement),
        price: Some(&price),
        audit: None,
    };
    history
        .update_product(&product, &change)
        .await
        .expect("atualizar com histórico");

    let movements = history
        .stock_movements(product.id(), 0, 10)
        .await
        .expect("listar");
    assert_eq!(movements.len(), 1);
    assert_eq!(movements[0].delta, -4);
    assert_eq!(movements[0].reason, StockReason::Sale);

    let prices = history
        .price_changes(product.id(), 0, 10)
        .await
        .expect("listar");
    assert_eq!(prices.len(), 1);
    assert_eq!(prices[0].old_cents, 1000);
    assert_eq!(prices[0].new_cents, 1500);
}

/// Verifica o contrato identificado por `updating_a_missing_product_reports_not_found_without_history` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn updating_a_missing_product_reports_not_found_without_history() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = setup().await;
    let database = &temp.db;
    let category_id = category(database).await;
    let history = SqlxCatalogHistoryRepository::new(database.pool().clone());

    // Produto nunca inserido.
    let product = product(category_id, "ATOM-4", 1000, 3);
    let movement =
        StockMovement::new(product.id(), 3, StockReason::Initial, None, now()).expect("movimento");
    let change = ProductChange {
        movement: Some(&movement),
        price: None,
        audit: None,
    };

    let result = history.update_product(&product, &change).await;
    assert!(result.is_err(), "produto inexistente deve falhar");

    let movements = history
        .stock_movements(product.id(), 0, 10)
        .await
        .expect("listar");
    assert!(movements.is_empty(), "histórico não pode ficar órfão");
}

/// Verifica o contrato identificado por `price_change_listing_is_newest_first` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn price_change_listing_is_newest_first() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = setup().await;
    let database = &temp.db;
    let category_id = category(database).await;
    let history = SqlxCatalogHistoryRepository::new(database.pool().clone());

    let mut product = product(category_id, "ATOM-5", 1000, 1);
    history
        .insert_product(&product, None, None)
        .await
        .expect("inserir");

    for (index, cents) in [1200, 1400, 1600].iter().enumerate() {
        let later = now() + Duration::minutes(index as i64 + 1);
        let previous = product.price().cents();
        product
            .change_price(Money::from_cents(*cents).expect("preço"), later)
            .expect("mudar preço");

        let price =
            PriceChange::new(product.id(), previous, *cents, None, later).expect("histórico");
        let change = ProductChange {
            movement: None,
            price: Some(&price),
            audit: None,
        };
        history
            .update_product(&product, &change)
            .await
            .expect("gravar");
    }

    let prices = history
        .price_changes(product.id(), 0, 10)
        .await
        .expect("listar");

    let values: Vec<i64> = prices.iter().map(|entry| entry.new_cents).collect();
    assert_eq!(
        values,
        vec![1600, 1400, 1200],
        "a série deve vir da mais recente para a mais antiga"
    );
}

/// Verifica o contrato identificado por `audit_records_and_lists_by_entity` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn audit_records_and_lists_by_entity() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = setup().await;
    let database = &temp.db;
    let audit = SqlxAuditRepository::new(database.pool().clone());
    let entity_id = uuid::Uuid::new_v4().to_string();

    let created = AuditEntry::created(
        None,
        AuditEntity::Product,
        entity_id.clone(),
        serde_json::json!({"stock": 1}),
        now(),
    );
    audit.record(&created).await.expect("gravar criação");

    let updated = AuditEntry::updated(
        None,
        AuditAction::Update,
        AuditEntity::Product,
        entity_id.clone(),
        serde_json::json!({"stock": 1}),
        serde_json::json!({"stock": 3}),
        now() + Duration::seconds(30),
    );
    audit.record(&updated).await.expect("gravar atualização");

    let filter = AuditFilter {
        entity: Some(AuditEntity::Product),
        entity_id: Some(entity_id),
        actor_id: None,
    };
    let page = audit.list(&filter, 0, 10).await.expect("listar");

    assert_eq!(page.total, 2);
    assert_eq!(page.items.len(), 2);
    assert_eq!(
        page.items[0].action,
        AuditAction::Update,
        "mais recente primeiro"
    );
    assert_eq!(page.items[1].action, AuditAction::Create);
}

/// Verifica o contrato identificado por `audit_filters_by_actor` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn audit_filters_by_actor() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = setup().await;
    let database = &temp.db;

    // A FK exige um autor real: o ator é uma linha de `users`, não um UUID solto.
    let users = infrastructure::SqlxUserRepository::new(database.pool().clone());
    let hasher = infrastructure::Argon2PasswordHasher::new();
    let hash = hasher.hash("Senha-Forte-2026!").await.expect("hash");
    let actor_user = domain::User::new(
        "Autor",
        domain::Email::parse("autor@example.com").expect("e-mail"),
        hash,
        domain::Role::Admin,
        now(),
    )
    .expect("usuário");
    users
        .insert(&actor_user, None)
        .await
        .expect("inserir autor");
    let actor = actor_user.id();

    let audit = SqlxAuditRepository::new(database.pool().clone());

    let entry = AuditEntry::created(
        Some(actor),
        AuditEntity::Category,
        uuid::Uuid::new_v4().to_string(),
        serde_json::json!({"name": "x"}),
        now(),
    );
    audit.record(&entry).await.expect("gravar");

    let matching = audit
        .list(
            &AuditFilter {
                entity: None,
                entity_id: None,
                actor_id: Some(actor),
            },
            0,
            10,
        )
        .await
        .expect("listar");
    assert_eq!(matching.total, 1);

    let other = audit
        .list(
            &AuditFilter {
                entity: None,
                entity_id: None,
                actor_id: Some(UserId::new_v4()),
            },
            0,
            10,
        )
        .await
        .expect("listar");
    assert_eq!(other.total, 0);
}

/// Verifica o contrato identificado por `audit_rejects_unknown_actor_by_foreign_key` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn audit_rejects_unknown_actor_by_foreign_key() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = setup().await;
    let database = &temp.db;
    let audit = SqlxAuditRepository::new(database.pool().clone());

    // `actor_id` inexistente viola a FK: a trilha nunca aponta para um fantasma.
    let entry = AuditEntry::created(
        Some(UserId::new_v4()),
        AuditEntity::User,
        uuid::Uuid::new_v4().to_string(),
        serde_json::json!({"name": "fantasma"}),
        now(),
    );

    assert!(
        audit.record(&entry).await.is_err(),
        "a FK de auditoria deve exigir um autor existente"
    );
}

/// Verifica o contrato identificado por `audit_rejects_unknown_entity_at_the_database_level` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn audit_rejects_unknown_entity_at_the_database_level() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = setup().await;
    let database = &temp.db;

    // Segunda barreira: mesmo que o código aceitasse um texto novo, o `CHECK`
    // do banco impede que a linha entre. É defesa em profundidade, e é por isso
    // que o adaptador pode ignorar linhas desconhecidas sem risco real.
    let result = sqlx::query(
        "INSERT INTO audit_log (id, actor_id, action, entity, entity_id, before, after, at) \
         VALUES ($1, NULL, 'create', 'planilha', $2, NULL, NULL, $3)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(now().to_rfc3339())
    .execute(database.pool())
    .await;

    assert!(
        result.is_err(),
        "CHECK de `entity` deve recusar valor fora do vocabulário"
    );
}

/// Verifica o contrato identificado por `soft_deleting_a_product_keeps_its_history` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn soft_deleting_a_product_keeps_its_history() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = setup().await;
    let database = &temp.db;
    let category_id = category(database).await;
    let history = SqlxCatalogHistoryRepository::new(database.pool().clone());

    let mut product = product(category_id, "ATOM-6", 1000, 2);
    history
        .insert_product(&product, None, None)
        .await
        .expect("inserir");

    let later = now() + Duration::minutes(1);
    product.soft_delete(later);
    let change = ProductChange {
        movement: None,
        price: None,
        audit: None,
    };
    history
        .update_product(&product, &change)
        .await
        .expect("soft delete");

    // O histórico continua consultável: ele é o registro de que o produto existiu.
    let movements = history
        .stock_movements(product.id(), 0, 10)
        .await
        .expect("listar");
    assert!(movements.is_empty(), "não houve movimentação neste produto");

    // Confirma que o produto foi para soft delete.
    let products = SqlxProductRepository::new(database.pool().clone());
    assert!(
        products
            .find_by_id(product.id())
            .await
            .expect("consultar")
            .is_none()
    );
}

/// Garante que o status restaurado de um produto excluído continua inativo.
#[tokio::test]
async fn deleted_product_status_is_inactive() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = setup().await;
    let database = &temp.db;
    let category_id = category(database).await;
    let history = SqlxCatalogHistoryRepository::new(database.pool().clone());

    let mut product = product(category_id, "ATOM-7", 1000, 1);
    history
        .insert_product(&product, None, None)
        .await
        .expect("inserir");

    let later = now() + Duration::minutes(1);
    product.soft_delete(later);
    history
        .update_product(
            &product,
            &ProductChange {
                movement: None,
                price: None,
                audit: None,
            },
        )
        .await
        .expect("soft delete");

    let raw_status: String = sqlx::query_scalar("SELECT status FROM products WHERE id = $1")
        .bind(product.id().to_string())
        .fetch_one(database.pool())
        .await
        .expect("consultar status");

    assert_eq!(raw_status, EntityStatus::Inactive.as_str());
}

/// Verifica o contrato identificado por `stale_stock_update_is_rejected_and_writes_nothing` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn stale_stock_update_is_rejected_and_writes_nothing() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = setup().await;
    let database = &temp.db;
    let category_id = category(database).await;
    let history = SqlxCatalogHistoryRepository::new(database.pool().clone());

    let product = product(category_id, "CAS-1", 1000, 10);
    history
        .insert_product(&product, None, None)
        .await
        .expect("inserir");

    // Primeira escrita vence: 10 → 7 (delta -3).
    let mut first = product.clone();
    first.set_stock(7, now()).expect("reduzir");
    let winner =
        StockMovement::new(first.id(), -3, StockReason::Sale, None, now()).expect("movimento");
    history
        .update_product(
            &first,
            &ProductChange {
                movement: Some(&winner),
                price: None,
                audit: None,
            },
        )
        .await
        .expect("primeira escrita");

    // Segunda escrita partiu da mesma leitura (10) e tenta 5 (delta -5).
    // Sem controle otimista ela gravaria 5 e o histórico somaria 2 movimentos
    // que não fecham com o saldo.
    let mut stale = product.clone();
    stale.set_stock(5, now()).expect("reduzir");
    let loser =
        StockMovement::new(stale.id(), -5, StockReason::Sale, None, now()).expect("movimento");

    let result = history
        .update_product(
            &stale,
            &ProductChange {
                movement: Some(&loser),
                price: None,
                audit: None,
            },
        )
        .await;

    assert!(
        matches!(result, Err(AppError::Conflict(_))),
        "escrita sobre leitura obsoleta deve virar conflito, veio {result:?}"
    );

    // Nem o saldo nem o histórico mudaram.
    let stored = SqlxProductRepository::new(database.pool().clone())
        .find_by_id(product.id())
        .await
        .expect("consultar")
        .expect("produto existe");
    assert_eq!(stored.stock(), 7, "saldo deve ser o da escrita vencedora");

    let movements = history
        .stock_movements(product.id(), 0, 10)
        .await
        .expect("listar");
    assert_eq!(movements.len(), 1, "a escrita perdedora não pode gravar");
    assert_eq!(movements[0].delta, -3);
}

/// Verifica o contrato identificado por `concurrent_updates_keep_stock_equal_to_the_sum_of_movements` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn concurrent_updates_keep_stock_equal_to_the_sum_of_movements() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = setup().await;
    let database = &temp.db;
    let category_id = category(database).await;
    let history = SqlxCatalogHistoryRepository::new(database.pool().clone());

    let product = product(category_id, "CAS-2", 1000, 20);
    let initial =
        StockMovement::new(product.id(), 20, StockReason::Initial, None, now()).expect("inicial");
    history
        .insert_product(&product, Some(&initial), None)
        .await
        .expect("inserir");

    // Três tentativas concorrentes, todas partindo do mesmo saldo lido (20).
    let mut tasks = Vec::new();
    for target in [18_i64, 15, 12] {
        let history = history.clone();
        let mut candidate = product.clone();
        candidate.set_stock(target, now()).expect("reduzir");
        let delta = target - product.stock();
        tasks.push(tokio::spawn(async move {
            let movement =
                StockMovement::new(candidate.id(), delta, StockReason::Sale, None, now())
                    .expect("movimento");
            let outcome = history
                .update_product(
                    &candidate,
                    &ProductChange {
                        movement: Some(&movement),
                        price: None,
                        audit: None,
                    },
                )
                .await;
            outcome.is_ok()
        }));
    }

    let mut successes = 0;
    for task in tasks {
        if task.await.expect("tarefa") {
            successes += 1;
        }
    }

    assert_eq!(
        successes, 1,
        "apenas uma escrita sobre a mesma leitura pode vencer"
    );

    let stored = SqlxProductRepository::new(database.pool().clone())
        .find_by_id(product.id())
        .await
        .expect("consultar")
        .expect("produto existe");

    let movements = history
        .stock_movements(product.id(), 0, 100)
        .await
        .expect("listar");
    let sum: i64 = movements.iter().map(|movement| movement.delta).sum();

    assert_eq!(
        stored.stock(),
        sum,
        "o saldo precisa ser a soma das movimentações ({movements:?})"
    );
}

/// Verifica o contrato identificado por `audit_is_written_in_the_same_transaction_as_the_product` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn audit_is_written_in_the_same_transaction_as_the_product() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = setup().await;
    let database = &temp.db;
    let category_id = category(database).await;
    let history = SqlxCatalogHistoryRepository::new(database.pool().clone());

    let product = product(category_id, "CAS-3", 1000, 3);
    let entry = AuditEntry::created(
        None,
        AuditEntity::Product,
        product.id().to_string(),
        serde_json::json!({"sku": "CAS-3"}),
        now(),
    );

    history
        .insert_product(&product, None, Some(&entry))
        .await
        .expect("inserir com auditoria");

    let audit = SqlxAuditRepository::new(database.pool().clone());
    let page = audit
        .list(&AuditFilter::default(), 0, 10)
        .await
        .expect("listar auditoria");

    assert_eq!(
        page.total, 1,
        "a trilha entra na mesma transação do produto"
    );
}

/// Verifica o contrato identificado por `failing_product_write_rolls_back_the_audit_entry` e protege o comportamento coberto pelas asserções.
/// O teste retorna `()`; uma asserção incompatível reprova o cenário.
#[tokio::test]
async fn failing_product_write_rolls_back_the_audit_entry() {
    // Banco temporário isolado, usado para não compartilhar dados entre cenários de teste.
    let temp = setup().await;
    let database = &temp.db;
    let category_id = category(database).await;
    let history = SqlxCatalogHistoryRepository::new(database.pool().clone());

    let existing = product(category_id, "CAS-4", 1000, 1);
    history
        .insert_product(&existing, None, None)
        .await
        .expect("inserir original");

    // Mesmo SKU: a transação falha inteira, então a auditoria não pode sobrar.
    let conflicting = product(category_id, "CAS-4", 2000, 5);
    let entry = AuditEntry::created(
        None,
        AuditEntity::Product,
        conflicting.id().to_string(),
        serde_json::json!({"sku": "CAS-4"}),
        now(),
    );

    assert!(
        history
            .insert_product(&conflicting, None, Some(&entry))
            .await
            .is_err(),
        "SKU duplicado deve falhar"
    );

    let audit = SqlxAuditRepository::new(database.pool().clone());
    let page = audit
        .list(&AuditFilter::default(), 0, 10)
        .await
        .expect("listar auditoria");

    assert_eq!(
        page.total, 0,
        "auditoria de uma escrita que falhou não pode existir"
    );
}
