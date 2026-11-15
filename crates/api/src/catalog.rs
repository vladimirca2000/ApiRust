//! Catálogo único dos endpoints da API.
//!
//! É a fonte da verdade usada pela página de desenvolvimento (`GET /`) e pelos
//! testes que verificam a divergência com o roteador real. Toda rota nova deve
//! ser registrada aqui — `crates/api/tests/routes_catalog.rs` falha se o
//! `router()` declarar um caminho que não esteja neste catálogo.

/// Visibilidade de um endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Sem autenticação.
    Public,
    /// Exige `Authorization: Bearer <access token>`.
    Bearer,
    /// Montado apenas com `SHUTDOWN_ON_TAB_CLOSE=true` (desenvolvimento).
    DevOnly,
}

impl Access {
    /// Rótulo curto exibido na página.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Public => "público",
            Self::Bearer => "Bearer",
            Self::DevOnly => "dev",
        }
    }
}

/// Descrição de um endpoint exposto pela API.
#[derive(Debug, Clone, Copy)]
pub struct Endpoint {
    /// Método HTTP (`GET`, `POST`, ...).
    pub method: &'static str,
    /// Caminho completo, começando com `/`.
    pub path: &'static str,
    /// Nível de acesso exigido.
    pub access: Access,
    /// O que o endpoint faz, em uma frase.
    pub description: &'static str,
}

/// Grupo de endpoints, usado para organizar a página por área.
#[derive(Debug, Clone, Copy)]
pub struct Group {
    /// Título da seção.
    pub title: &'static str,
    /// Endpoints do grupo.
    pub endpoints: &'static [Endpoint],
}

/// Endpoints de operação (health/readiness).
pub const OPERATION: &[Endpoint] = &[
    Endpoint {
        method: "GET",
        path: "/health",
        access: Access::Public,
        description: "Processo ativo (não consulta dependências).",
    },
    Endpoint {
        method: "GET",
        path: "/ready",
        access: Access::Public,
        description: "Dependências prontas (executa SELECT 1 no SQLite).",
    },
];

/// Endpoints de autenticação (Fase 3).
pub const AUTH: &[Endpoint] = &[
    Endpoint {
        method: "POST",
        path: "/api/v1/auth/login",
        access: Access::Public,
        description: "Autentica e-mail e senha e devolve access + refresh.",
    },
    Endpoint {
        method: "POST",
        path: "/api/v1/auth/refresh",
        access: Access::Public,
        description: "Rotaciona o refresh token e devolve um par novo.",
    },
    Endpoint {
        method: "POST",
        path: "/api/v1/auth/logout",
        access: Access::Public,
        description: "Revoga o refresh token apresentado (resposta sem corpo).",
    },
    Endpoint {
        method: "GET",
        path: "/api/v1/users/me",
        access: Access::Bearer,
        description: "Perfil do usuário autenticado.",
    },
];

/// Endpoints de gerenciamento de usuários (Fase 4).
pub const USERS: &[Endpoint] = &[
    Endpoint {
        method: "GET",
        path: "/api/v1/users",
        access: Access::Bearer,
        description: "Lista usuários vivos com paginação (apenas Admin).",
    },
    Endpoint {
        method: "POST",
        path: "/api/v1/users",
        access: Access::Bearer,
        description: "Cria um usuário novo (apenas Admin).",
    },
    Endpoint {
        method: "GET",
        path: "/api/v1/users/{id}",
        access: Access::Bearer,
        description: "Lê um usuário (Admin ou o próprio).",
    },
    Endpoint {
        method: "PATCH",
        path: "/api/v1/users/{id}",
        access: Access::Bearer,
        description: "Atualiza nome, e-mail ou senha (Admin ou o próprio).",
    },
    Endpoint {
        method: "DELETE",
        path: "/api/v1/users/{id}",
        access: Access::Bearer,
        description: "Aplica soft delete (apenas Admin, nunca a própria conta).",
    },
    Endpoint {
        method: "PATCH",
        path: "/api/v1/users/{id}/role",
        access: Access::Bearer,
        description: "Altera o papel (apenas Admin, nunca o próprio).",
    },
];

/// Endpoints do catálogo: categorias e produtos (Fase 5).
pub const CATALOG: &[Endpoint] = &[
    Endpoint {
        method: "GET",
        path: "/api/v1/categories",
        access: Access::Bearer,
        description: "Lista categorias com paginação.",
    },
    Endpoint {
        method: "POST",
        path: "/api/v1/categories",
        access: Access::Bearer,
        description: "Cria categoria (Admin/Manager), nome único.",
    },
    Endpoint {
        method: "GET",
        path: "/api/v1/categories/{id}",
        access: Access::Bearer,
        description: "Lê uma categoria.",
    },
    Endpoint {
        method: "PATCH",
        path: "/api/v1/categories/{id}",
        access: Access::Bearer,
        description: "Atualiza nome ou descrição (Admin/Manager).",
    },
    Endpoint {
        method: "DELETE",
        path: "/api/v1/categories/{id}",
        access: Access::Bearer,
        description: "Remove categoria; responde conflito se houver produtos vinculados.",
    },
    Endpoint {
        method: "GET",
        path: "/api/v1/products",
        access: Access::Bearer,
        description: "Lista produtos com filtros e ordenação por allowlist.",
    },
    Endpoint {
        method: "POST",
        path: "/api/v1/products",
        access: Access::Bearer,
        description: "Cria produto (Admin/Manager), SKU único e preço em centavos.",
    },
    Endpoint {
        method: "GET",
        path: "/api/v1/products/{id}",
        access: Access::Bearer,
        description: "Lê um produto.",
    },
    Endpoint {
        method: "PATCH",
        path: "/api/v1/products/{id}",
        access: Access::Bearer,
        description: "Atualiza produto (Admin/Manager).",
    },
    Endpoint {
        method: "DELETE",
        path: "/api/v1/products/{id}",
        access: Access::Bearer,
        description: "Remove produto via soft delete (Admin/Manager).",
    },
];

/// Endpoints de histórico do catálogo e trilha de auditoria (Fase 8).
pub const HISTORY: &[Endpoint] = &[
    Endpoint {
        method: "GET",
        path: "/api/v1/products/{id}/stock-movements",
        access: Access::Bearer,
        description: "Movimentações de estoque do produto (Admin/Manager).",
    },
    Endpoint {
        method: "GET",
        path: "/api/v1/products/{id}/price-changes",
        access: Access::Bearer,
        description: "Alterações de preço do produto (Admin/Manager).",
    },
    Endpoint {
        method: "GET",
        path: "/api/v1/audit",
        access: Access::Bearer,
        description: "Trilha de auditoria: quem alterou o quê (apenas Admin).",
    },
];

/// Endpoints auxiliares de desenvolvimento.
pub const DEVELOPMENT: &[Endpoint] = &[
    Endpoint {
        method: "GET",
        path: "/",
        access: Access::DevOnly,
        description: "Esta página; mantém a sessão que sustenta o servidor.",
    },
    Endpoint {
        method: "GET",
        path: "/api/dev/session",
        access: Access::DevOnly,
        description: "Canal SSE que mantém a aba vinculada ao processo.",
    },
];

/// Todos os grupos, na ordem em que aparecem na página.
pub const GROUPS: &[Group] = &[
    Group {
        title: "Operação",
        endpoints: OPERATION,
    },
    Group {
        title: "Autenticação",
        endpoints: AUTH,
    },
    Group {
        title: "Usuários",
        endpoints: USERS,
    },
    Group {
        title: "Catálogo",
        endpoints: CATALOG,
    },
    Group {
        title: "Histórico e auditoria",
        endpoints: HISTORY,
    },
    Group {
        title: "Desenvolvimento",
        endpoints: DEVELOPMENT,
    },
];

/// Itera sobre todos os endpoints de todos os grupos.
pub fn all_endpoints() -> impl Iterator<Item = &'static Endpoint> {
    GROUPS.iter().flat_map(|group| group.endpoints.iter())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifica o contrato identificado por `no_duplicate_method_and_path_pairs` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn no_duplicate_method_and_path_pairs() {
        let mut seen = Vec::new();
        for endpoint in all_endpoints() {
            let key = (endpoint.method, endpoint.path);
            assert!(
                !seen.contains(&key),
                "endpoint duplicado no catálogo: {} {}",
                endpoint.method,
                endpoint.path
            );
            seen.push(key);
        }
    }

    /// Verifica o contrato identificado por `every_path_starts_with_slash_and_has_description` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn every_path_starts_with_slash_and_has_description() {
        for endpoint in all_endpoints() {
            assert!(
                endpoint.path.starts_with('/'),
                "caminho deve começar com '/': {}",
                endpoint.path
            );
            assert!(
                !endpoint.description.trim().is_empty(),
                "endpoint sem descrição: {} {}",
                endpoint.method,
                endpoint.path
            );
        }
    }
}
