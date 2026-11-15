//! Matriz de permissões da Fase 4, exercitada pela API HTTP.
//!
//! Cobre os dois riscos centrais: **IDOR** (acessar/altear dados de terceiros)
//! e **escalonamento de privilégio** (conceder-se um papel maior). Cada teste
//! autentica um usuário real com o papel indicado e observa o status HTTP.

use std::net::SocketAddr;

use api::AppState;
use application::{PasswordHasher, UserRepository};
use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use chrono::Utc;
use domain::{Email, Role, User, UserId};
use http_body_util::BodyExt;
use infrastructure::{Argon2PasswordHasher, Database, SqlxUserRepository};
use serde_json::{Value, json};
use tower::ServiceExt;

/// Segredo usado para assinar os tokens dos testes.
const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

/// Senha em texto puro de todos os usuários de teste.
const PASSWORD: &str = "Senha-Forte-2026!";

/// Usuário semeado no banco, com as credenciais para autenticar.
#[derive(Debug, Clone)]
struct Seeded {
    /// Identificador do usuário.
    id: UserId,
    /// E-mail usado no login.
    email: String,
    /// Papel efetivo.
    role: Role,
}

/// Ambiente de teste: estado da API e usuários semeados.
struct Env {
    /// Estado compartilhado pelos handlers.
    state: AppState,
    /// Administrador.
    admin: Seeded,
    /// Gerente de catálogo (sem poderes de usuários).
    manager: Seeded,
    /// Usuário comum.
    user: Seeded,
}

/// Monta o ambiente com um usuário de cada papel.
async fn env() -> Env {
    let database = Database::connect_path(":memory:")
        .await
        .expect("banco em memória");
    database.migrate().await.expect("migrations");

    let repository = SqlxUserRepository::new(database.pool().clone());
    let hasher = Argon2PasswordHasher::new();

    let admin = seed_user(
        &repository,
        &hasher,
        "Admin",
        "admin@example.com",
        Role::Admin,
    )
    .await;
    let manager = seed_user(
        &repository,
        &hasher,
        "Manager",
        "manager@example.com",
        Role::Manager,
    )
    .await;
    let user = seed_user(&repository, &hasher, "User", "user@example.com", Role::User).await;

    let state = AppState::new(false, database)
        .with_jwt(SECRET, 900)
        .expect("jwt");

    Env {
        state,
        admin,
        manager,
        user,
    }
}

/// Cria e persiste um usuário de teste, devolvendo suas credenciais.
async fn seed_user(
    repository: &SqlxUserRepository,
    hasher: &Argon2PasswordHasher,
    name: &str,
    email: &str,
    role: Role,
) -> Seeded {
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

    Seeded {
        id: user.id(),
        email: email.to_owned(),
        role,
    }
}

/// Envia uma requisição com IP de origem e token opcionais.
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
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5000))));

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

/// Faz login e devolve o access token.
async fn login(env: &Env, seeded: &Seeded) -> String {
    let (status, body) = call(
        &env.state,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({"email": seeded.email, "password": PASSWORD})),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "login deveria funcionar");
    body["access_token"]
        .as_str()
        .expect("access_token")
        .to_owned()
}

#[tokio::test]
async fn all_roles_can_read_their_own_profile() {
    let env = env().await;

    for seeded in [&env.admin, &env.manager, &env.user] {
        let token = login(&env, seeded).await;
        let (status, body) = call(
            &env.state,
            "GET",
            &format!("/api/v1/users/{}", seeded.id),
            Some(&token),
            None,
        )
        .await;

        assert_eq!(status, StatusCode::OK, "papel {:?}", seeded.role);
        assert_eq!(body["email"], seeded.email);
        assert!(
            body.get("password_hash").is_none(),
            "a resposta nunca pode expor o hash"
        );
    }
}

#[tokio::test]
async fn non_admin_cannot_list_users() {
    let env = env().await;

    for seeded in [&env.manager, &env.user] {
        let token = login(&env, seeded).await;
        let (status, _) = call(&env.state, "GET", "/api/v1/users", Some(&token), None).await;

        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "papel {:?} não pode listar usuários",
            seeded.role
        );
    }
}

#[tokio::test]
async fn admin_can_list_users() {
    let env = env().await;
    let token = login(&env, &env.admin).await;

    let (status, body) = call(&env.state, "GET", "/api/v1/users", Some(&token), None).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 3);
    assert_eq!(body["items"].as_array().expect("itens").len(), 3);
}

#[tokio::test]
async fn non_admin_cannot_create_users() {
    let env = env().await;

    for seeded in [&env.manager, &env.user] {
        let token = login(&env, seeded).await;
        let (status, _) = call(
            &env.state,
            "POST",
            "/api/v1/users",
            Some(&token),
            Some(json!({
                "name": "Intruso",
                "email": "intruso@example.com",
                "password": PASSWORD,
                "role": "admin",
            })),
        )
        .await;

        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "papel {:?} não pode criar usuários",
            seeded.role
        );
    }
}

#[tokio::test]
async fn admin_can_create_a_user() {
    let env = env().await;
    let token = login(&env, &env.admin).await;

    let (status, body) = call(
        &env.state,
        "POST",
        "/api/v1/users",
        Some(&token),
        Some(json!({
            "name": "Novo Usuário",
            "email": "novo@example.com",
            "password": PASSWORD,
            "role": "manager",
        })),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["email"], "novo@example.com");
    assert_eq!(body["role"], "manager");
    assert_eq!(body["status"], "active");
}

#[tokio::test]
async fn creating_user_with_duplicate_email_returns_409() {
    let env = env().await;
    let token = login(&env, &env.admin).await;

    let (status, _) = call(
        &env.state,
        "POST",
        "/api/v1/users",
        Some(&token),
        Some(json!({
            "name": "Duplicado",
            "email": "user@example.com",
            "password": PASSWORD,
            "role": "user",
        })),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn creating_user_with_weak_password_returns_422() {
    let env = env().await;
    let token = login(&env, &env.admin).await;

    let (status, _) = call(
        &env.state,
        "POST",
        "/api/v1/users",
        Some(&token),
        Some(json!({
            "name": "Fraco",
            "email": "fraco@example.com",
            "password": "fraca",
            "role": "user",
        })),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn user_cannot_read_another_users_profile_idor() {
    let env = env().await;
    let token = login(&env, &env.user).await;

    // `user` tenta ler o perfil do admin pelo id — o ataque IDOR clássico.
    let (status, _) = call(
        &env.state,
        "GET",
        &format!("/api/v1/users/{}", env.admin.id),
        Some(&token),
        None,
    )
    .await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "usuário comum não pode ler perfil de terceiros"
    );
}

#[tokio::test]
async fn user_cannot_update_another_users_profile_idor() {
    let env = env().await;
    let token = login(&env, &env.user).await;

    let (status, _) = call(
        &env.state,
        "PATCH",
        &format!("/api/v1/users/{}", env.admin.id),
        Some(&token),
        Some(json!({"name": "Sequestrado"})),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "usuário comum não pode alterar perfil de terceiros"
    );
}

#[tokio::test]
async fn user_can_update_own_profile() {
    let env = env().await;
    let token = login(&env, &env.user).await;

    let (status, body) = call(
        &env.state,
        "PATCH",
        &format!("/api/v1/users/{}", env.user.id),
        Some(&token),
        Some(json!({"name": "Nome Alterado"})),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "Nome Alterado");
}

#[tokio::test]
async fn user_cannot_change_own_role_escalation() {
    let env = env().await;
    let token = login(&env, &env.user).await;

    // Auto-promoção para admin: o escalonamento de privilégio mais óbvio.
    let (status, _) = call(
        &env.state,
        "PATCH",
        &format!("/api/v1/users/{}/role", env.user.id),
        Some(&token),
        Some(json!({"role": "admin"})),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "usuário comum não pode alterar papéis"
    );
}

#[tokio::test]
async fn manager_cannot_change_roles() {
    let env = env().await;
    let token = login(&env, &env.manager).await;

    let (status, _) = call(
        &env.state,
        "PATCH",
        &format!("/api/v1/users/{}/role", env.user.id),
        Some(&token),
        Some(json!({"role": "admin"})),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "Manager não gerencia usuários"
    );
}

#[tokio::test]
async fn admin_can_change_another_users_role() {
    let env = env().await;
    let token = login(&env, &env.admin).await;

    let (status, body) = call(
        &env.state,
        "PATCH",
        &format!("/api/v1/users/{}/role", env.user.id),
        Some(&token),
        Some(json!({"role": "manager"})),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["role"], "manager");
}

#[tokio::test]
async fn admin_cannot_change_own_role() {
    let env = env().await;
    let token = login(&env, &env.admin).await;

    let (status, _) = call(
        &env.state,
        "PATCH",
        &format!("/api/v1/users/{}/role", env.admin.id),
        Some(&token),
        Some(json!({"role": "user"})),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "Admin não pode se rebaixar e deixar o sistema sem administrador"
    );
}

#[tokio::test]
async fn admin_cannot_delete_own_account() {
    let env = env().await;
    let token = login(&env, &env.admin).await;

    let (status, _) = call(
        &env.state,
        "DELETE",
        &format!("/api/v1/users/{}", env.admin.id),
        Some(&token),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn user_cannot_delete_anyone() {
    let env = env().await;
    let token = login(&env, &env.user).await;

    let (status, _) = call(
        &env.state,
        "DELETE",
        &format!("/api/v1/users/{}", env.admin.id),
        Some(&token),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_can_delete_another_user_and_it_disappears_from_listing() {
    let env = env().await;
    let token = login(&env, &env.admin).await;

    let (status, _) = call(
        &env.state,
        "DELETE",
        &format!("/api/v1/users/{}", env.user.id),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // A listagem deve ocultar o registro com soft delete.
    let (_, body) = call(&env.state, "GET", "/api/v1/users", Some(&token), None).await;
    assert_eq!(body["total"], 2);

    // E o perfil dele responde 404.
    let (status, _) = call(
        &env.state,
        "GET",
        &format!("/api/v1/users/{}", env.user.id),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_user_returns_404() {
    let env = env().await;
    let token = login(&env, &env.admin).await;
    let missing = UserId::new_v4();

    let (status, _) = call(
        &env.state,
        "GET",
        &format!("/api/v1/users/{missing}"),
        Some(&token),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn malformed_id_returns_404_not_500() {
    let env = env().await;
    let token = login(&env, &env.admin).await;

    let (status, _) = call(
        &env.state,
        "GET",
        "/api/v1/users/nao-e-uuid",
        Some(&token),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn invalid_role_value_returns_422() {
    let env = env().await;
    let token = login(&env, &env.admin).await;

    let (status, _) = call(
        &env.state,
        "PATCH",
        &format!("/api/v1/users/{}/role", env.user.id),
        Some(&token),
        Some(json!({"role": "superadmin"})),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn unauthenticated_requests_get_401_everywhere() {
    let env = env().await;
    let id = env.user.id;

    let cases = [
        ("GET", "/api/v1/users".to_owned()),
        ("POST", "/api/v1/users".to_owned()),
        ("GET", format!("/api/v1/users/{id}")),
        ("PATCH", format!("/api/v1/users/{id}")),
        ("DELETE", format!("/api/v1/users/{id}")),
        ("PATCH", format!("/api/v1/users/{id}/role")),
    ];

    for (method, path) in cases {
        let (status, _) = call(&env.state, method, &path, None, None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} deveria exigir autenticação"
        );
    }
}

#[tokio::test]
async fn demoted_admin_loses_privileges_immediately() {
    let env = env().await;

    // Admin rebaixa o manager para user.
    let admin_token = login(&env, &env.admin).await;
    let (status, _) = call(
        &env.state,
        "PATCH",
        &format!("/api/v1/users/{}/role", env.manager.id),
        Some(&admin_token),
        Some(json!({"role": "user"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // O token do manager foi emitido quando ele ainda era Manager; ainda assim,
    // o papel é relido do banco, então a listagem deve negar.
    let manager_token = login(&env, &env.manager).await;
    let (status, _) = call(
        &env.state,
        "GET",
        "/api/v1/users",
        Some(&manager_token),
        None,
    )
    .await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "o papel deve ser relido do banco, não confiado da claim"
    );
}
