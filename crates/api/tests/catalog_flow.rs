//! Testes HTTP do catálogo (Fase 5).
//!
//! Cobrem o critério de aceite: invariantes no domínio e no banco, `400` para
//! ordenação inválida, limite máximo de paginação e `409` ao excluir categoria
//! com produtos.

use std::net::SocketAddr;

use api::AppState;
use application::{PasswordHasher, UserRepository};
use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use chrono::Utc;
use domain::{Email, Role, User};
use http_body_util::BodyExt;
use infrastructure::{Argon2PasswordHasher, Database, SqlxUserRepository};
use serde_json::{Value, json};
use tower::ServiceExt;

/// Segredo dos tokens de teste.
const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";
/// Senha dos usuários de teste.
const PASSWORD: &str = "Senha-Forte-2026!";

/// Ambiente com um Admin, um Manager e um User autenticáveis.
struct Env {
    /// Estado da API.
    state: AppState,
    /// Token do Admin.
    admin: String,
    /// Token do Manager.
    manager: String,
    /// Token do User.
    user: String,
}

/// Monta o ambiente e autentica os três papéis.
async fn env() -> Env {
    let database = Database::connect_path(":memory:")
        .await
        .expect("banco em memória");
    database.migrate().await.expect("migrations");

    let repository = SqlxUserRepository::new(database.pool().clone());
    let hasher = Argon2PasswordHasher::new();

    for (name, email, role) in [
        ("Admin", "admin@example.com", Role::Admin),
        ("Manager", "manager@example.com", Role::Manager),
        ("User", "user@example.com", Role::User),
    ] {
        let hash = hasher.hash(PASSWORD).await.expect("hash");
        let user = User::new(
            name,
            Email::parse(email).expect("e-mail"),
            hash,
            role,
            Utc::now(),
        )
        .expect("usuário");
        repository.insert(&user).await.expect("inserir");
    }

    let state = AppState::new(false, database)
        .with_jwt(SECRET, 900)
        .expect("jwt");

    let admin = do_login(&state, "admin@example.com").await;
    let manager = do_login(&state, "manager@example.com").await;
    let user = do_login(&state, "user@example.com").await;

    Env {
        state,
        admin,
        manager,
        user,
    }
}

/// Faz login e devolve o access token.
async fn do_login(state: &AppState, email: &str) -> String {
    let (status, body) = call(
        state,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({"email": email, "password": PASSWORD})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login de {email}");
    body["access_token"]
        .as_str()
        .expect("access_token")
        .to_owned()
}

/// Envia uma requisição com IP e token opcionais.
async fn call(
    state: &AppState,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);

    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }

    let payload = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };

    let mut request = builder.body(payload).expect("requisição");
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 6000))));

    let response = api::router(state.clone())
        .oneshot(request)
        .await
        .expect("resposta");

    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("corpo")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Cria uma categoria e devolve seu id.
async fn create_category(env: &Env, name: &str) -> String {
    let (status, body) = call(
        &env.state,
        "POST",
        "/api/v1/categories",
        Some(&env.admin),
        Some(json!({"name": name, "description": null})),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED, "criar categoria {name}");
    body["id"].as_str().expect("id").to_owned()
}

#[tokio::test]
async fn admin_creates_and_lists_categories() {
    let env = env().await;

    create_category(&env, "Eletrônicos").await;
    create_category(&env, "Livros").await;

    let (status, body) = call(
        &env.state,
        "GET",
        "/api/v1/categories",
        Some(&env.admin),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 2);
}

#[tokio::test]
async fn duplicate_category_name_returns_409() {
    let env = env().await;
    create_category(&env, "Única").await;

    let (status, _) = call(
        &env.state,
        "POST",
        "/api/v1/categories",
        Some(&env.manager),
        Some(json!({"name": "Única"})),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn plain_user_cannot_create_category() {
    let env = env().await;

    let (status, _) = call(
        &env.state,
        "POST",
        "/api/v1/categories",
        Some(&env.user),
        Some(json!({"name": "Proibida"})),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn plain_user_can_read_categories() {
    let env = env().await;
    create_category(&env, "Pública").await;

    let (status, body) = call(
        &env.state,
        "GET",
        "/api/v1/categories",
        Some(&env.user),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "leitura do catálogo é para todos");
    assert_eq!(body["total"], 1);
}

#[tokio::test]
async fn deleting_category_with_products_returns_409() {
    let env = env().await;
    let category_id = create_category(&env, "Com produtos").await;

    let (status, _) = call(
        &env.state,
        "POST",
        "/api/v1/products",
        Some(&env.admin),
        Some(json!({
            "name": "Teclado",
            "sku": "TEC-1",
            "price_cents": 19990,
            "stock": 5,
            "category_id": category_id,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, body) = call(
        &env.state,
        "DELETE",
        &format!("/api/v1/categories/{category_id}"),
        Some(&env.admin),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        body["detail"].as_str().unwrap_or("").contains("produtos"),
        "a mensagem deve explicar o conflito: {body}"
    );
}

#[tokio::test]
async fn deleting_empty_category_returns_204() {
    let env = env().await;
    let category_id = create_category(&env, "Vazia").await;

    let (status, _) = call(
        &env.state,
        "DELETE",
        &format!("/api/v1/categories/{category_id}"),
        Some(&env.manager),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn creating_product_with_invalid_price_returns_422() {
    let env = env().await;
    let category_id = create_category(&env, "C").await;

    for price in [0, -100] {
        let (status, _) = call(
            &env.state,
            "POST",
            "/api/v1/products",
            Some(&env.admin),
            Some(json!({
                "name": "Inválido",
                "sku": format!("INV-{price}"),
                "price_cents": price,
                "stock": 1,
                "category_id": category_id,
            })),
        )
        .await;

        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "preço {price} deveria ser rejeitado"
        );
    }
}

#[tokio::test]
async fn creating_product_with_negative_stock_returns_422() {
    let env = env().await;
    let category_id = create_category(&env, "C").await;

    let (status, _) = call(
        &env.state,
        "POST",
        "/api/v1/products",
        Some(&env.admin),
        Some(json!({
            "name": "Negativo",
            "sku": "NEG-1",
            "price_cents": 1000,
            "stock": -1,
            "category_id": category_id,
        })),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn creating_product_with_invalid_sku_returns_422() {
    let env = env().await;
    let category_id = create_category(&env, "C").await;

    let (status, _) = call(
        &env.state,
        "POST",
        "/api/v1/products",
        Some(&env.admin),
        Some(json!({
            "name": "SKU inválido",
            "sku": "ab",
            "price_cents": 1000,
            "stock": 1,
            "category_id": category_id,
        })),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn creating_product_with_unknown_category_returns_404() {
    let env = env().await;

    let (status, _) = call(
        &env.state,
        "POST",
        "/api/v1/products",
        Some(&env.admin),
        Some(json!({
            "name": "Órfão",
            "sku": "ORF-1",
            "price_cents": 1000,
            "stock": 1,
            "category_id": "00000000-0000-4000-8000-000000000000",
        })),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn duplicate_sku_returns_409() {
    let env = env().await;
    let category_id = create_category(&env, "C").await;

    let product = json!({
        "name": "P1",
        "sku": "DUP-1",
        "price_cents": 1000,
        "stock": 1,
        "category_id": category_id,
    });

    let (status, _) = call(
        &env.state,
        "POST",
        "/api/v1/products",
        Some(&env.admin),
        Some(product.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = call(
        &env.state,
        "POST",
        "/api/v1/products",
        Some(&env.admin),
        Some(product),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn invalid_sort_returns_400() {
    let env = env().await;

    let (status, body) = call(
        &env.state,
        "GET",
        "/api/v1/products?sort=price;%20DROP%20TABLE%20products",
        Some(&env.admin),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "ordenação inválida = 400");
    assert_eq!(body["title"], "invalid_request");
}

#[tokio::test]
async fn every_allowlisted_sort_is_accepted() {
    let env = env().await;

    for sort in ["newest", "oldest", "name", "cheapest", "most_expensive"] {
        let (status, _) = call(
            &env.state,
            "GET",
            &format!("/api/v1/products?sort={sort}"),
            Some(&env.admin),
            None,
        )
        .await;

        assert_eq!(status, StatusCode::OK, "sort={sort} deveria ser aceito");
    }
}

#[tokio::test]
async fn page_size_is_capped_at_maximum() {
    let env = env().await;

    let (status, body) = call(
        &env.state,
        "GET",
        "/api/v1/products?limit=100000",
        Some(&env.admin),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["limit"], 100,
        "o limite deve ser truncado em 100, nunca obedecer 100000"
    );
}

#[tokio::test]
async fn product_filters_by_category_and_name() {
    let env = env().await;
    let tech = create_category(&env, "Tech").await;
    let books = create_category(&env, "Books").await;

    for (name, sku, category) in [
        ("Notebook", "NB-1", &tech),
        ("Mouse", "MS-1", &tech),
        ("Livro", "LV-1", &books),
    ] {
        let (status, _) = call(
            &env.state,
            "POST",
            "/api/v1/products",
            Some(&env.admin),
            Some(json!({
                "name": name,
                "sku": sku,
                "price_cents": 1000,
                "stock": 1,
                "category_id": category,
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let (_, body) = call(
        &env.state,
        "GET",
        &format!("/api/v1/products?category_id={tech}"),
        Some(&env.admin),
        None,
    )
    .await;
    assert_eq!(body["total"], 2);

    let (_, body) = call(
        &env.state,
        "GET",
        "/api/v1/products?name=livro",
        Some(&env.admin),
        None,
    )
    .await;
    assert_eq!(body["total"], 1, "busca por nome é case-insensitive");
}

#[tokio::test]
async fn product_price_is_returned_in_cents() {
    let env = env().await;
    let category = create_category(&env, "C").await;

    let (_, body) = call(
        &env.state,
        "POST",
        "/api/v1/products",
        Some(&env.admin),
        Some(json!({
            "name": "Preço",
            "sku": "PRC-1",
            "price_cents": 129990,
            "stock": 1,
            "category_id": category,
        })),
    )
    .await;

    assert_eq!(body["price_cents"], 129990, "preço trafega em centavos");
}

#[tokio::test]
async fn updating_product_stock_and_price_works() {
    let env = env().await;
    let category = create_category(&env, "C").await;

    let (_, created) = call(
        &env.state,
        "POST",
        "/api/v1/products",
        Some(&env.admin),
        Some(json!({
            "name": "P",
            "sku": "UPD-1",
            "price_cents": 1000,
            "stock": 5,
            "category_id": category,
        })),
    )
    .await;
    let id = created["id"].as_str().expect("id");

    let (status, body) = call(
        &env.state,
        "PATCH",
        &format!("/api/v1/products/{id}"),
        Some(&env.manager),
        Some(json!({"price_cents": 2500, "stock": 0})),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["price_cents"], 2500);
    assert_eq!(body["stock"], 0, "estoque zero é válido");
}

#[tokio::test]
async fn deleting_product_removes_it_from_listing() {
    let env = env().await;
    let category = create_category(&env, "C").await;

    let (_, created) = call(
        &env.state,
        "POST",
        "/api/v1/products",
        Some(&env.admin),
        Some(json!({
            "name": "Some",
            "sku": "DEL-1",
            "price_cents": 1000,
            "stock": 1,
            "category_id": category,
        })),
    )
    .await;
    let id = created["id"].as_str().expect("id");

    let (status, _) = call(
        &env.state,
        "DELETE",
        &format!("/api/v1/products/{id}"),
        Some(&env.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, body) = call(
        &env.state,
        "GET",
        "/api/v1/products",
        Some(&env.admin),
        None,
    )
    .await;
    assert_eq!(body["total"], 0);
}

#[tokio::test]
async fn plain_user_cannot_write_products() {
    let env = env().await;
    let category = create_category(&env, "C").await;

    let (status, _) = call(
        &env.state,
        "POST",
        "/api/v1/products",
        Some(&env.user),
        Some(json!({
            "name": "Proibido",
            "sku": "PRB-1",
            "price_cents": 1000,
            "stock": 1,
            "category_id": category,
        })),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn catalog_requires_authentication() {
    let env = env().await;

    for (method, path) in [
        ("GET", "/api/v1/categories"),
        ("POST", "/api/v1/categories"),
        ("GET", "/api/v1/products"),
        ("POST", "/api/v1/products"),
    ] {
        let (status, _) = call(&env.state, method, path, None, None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} deveria exigir autenticação"
        );
    }
}
