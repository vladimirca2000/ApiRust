# ApiRust

API RESTful em Rust para gerenciamento de usuários, categorias e produtos, seguindo DDD, Clean Architecture e boas práticas de segurança, testes e observabilidade.

O plano completo de implementação está em [`docs/plano-de-implementacao.md`](docs/plano-de-implementacao.md).

## Stack

- **Runtime/HTTP:** Tokio + Axum
- **Persistência:** SQLx + SQLite (migrations versionadas)
- **Autenticação:** JWT (access curto + refresh com rotação) e Argon2id
- **Documentação:** OpenAPI via Utoipa + Swagger UI
- **Erros:** `thiserror` + respostas `application/problem+json` (RFC 7807)
- **Observabilidade:** `tracing` / `tracing-subscriber`

## Arquitetura

Workspace com quatro crates:

```text
crates/
  domain/          # Entidades, Value Objects e regras de negócio
  application/     # Casos de uso e portas
  infrastructure/  # SQLx, Redis, JWT, Argon2id
  api/             # HTTP, DTOs, middlewares, bootstrap
migrations/
```

Direção das dependências: `api -> application -> domain` e `infrastructure -> application/domain`.

## Pré-requisitos

- Rust stable (instalado via [rustup](https://rustup.rs))
- Componentes: `clippy`, `rustfmt` (o `rust-toolchain.toml` já os declara)

## Configuração

```powershell
Copy-Item .env.example .env
# edite .env e defina um JWT_SECRET forte:
# openssl rand -base64 48
```

Variáveis:

| Variável | Obrigatória | Padrão | Descrição |
|---|---|---|---|
| `APP_ENV` | não | `development` | `development` ou `production` |
| `APP_HOST` | não | `127.0.0.1` | IP de bind |
| `APP_PORT` | não | `8080` | Porta HTTP |
| `DATABASE_URL` | sim | — | URL do SQLite (o diretório é criado automaticamente) |
| `RUN_MIGRATIONS` | não | `true` | Aplica as migrations embutidas no boot |
| `REDIS_URL` | não | — | Redis para Fase 3 (use `redis://usuario:senha@127.0.0.1:6379` se houver autenticação) |
| `JWT_SECRET` | sim | — | Segredo com pelo menos 32 caracteres |
| `JWT_ACCESS_TTL_SECS` | não | `900` | TTL do access token |
| `ADMIN_EMAIL` | não | — | E-mail do Admin inicial (exige `ADMIN_PASSWORD`) |
| `ADMIN_PASSWORD` | não | — | Senha do Admin inicial (exige `ADMIN_EMAIL`) |
| `SHUTDOWN_ON_TAB_CLOSE` | não | `false` | Dev: encerra o servidor 10s após a última aba fechar |
| `MAX_CONCURRENT_REQUESTS` | não | `128` | Teto de requisições HTTP simultâneas; o excesso responde `503` |
| `SHUTDOWN_DRAIN_SECS` | não | `30` | Espera das conexões em voo após o sinal de shutdown |
| `RUST_LOG` | não | `info,api=debug,tower_http=info` | Filtro de logs |

Nunca versione `.env` nem use o segredo de exemplo em produção.

### Banco e migrations

As migrations ficam em `migrations/` e são embutidas no binário. Com `RUN_MIGRATIONS=true` (padrão), são aplicadas de forma idempotente no boot. Em ambientes com passo de deploy controlado, defina `false` e aplique com `sqlx migrate run` (requer `sqlx-cli`).

SQLite é configurado com foreign keys, WAL, `busy_timeout` de 5s e pool limitado a 5 conexões. Unicidade de e-mail, SKU e nome de categoria vale apenas para registros ativos (soft delete).

### Primeiro Admin

Defina `ADMIN_EMAIL` e `ADMIN_PASSWORD` juntos. No boot, a API cria um usuário `admin` ativo se o e-mail ainda não existir; se já existir, nada é alterado. As duas variáveis são validadas na inicialização (e-mail normalizado e política de senha) e podem ser removidas depois do primeiro provisionamento.

A senha nunca é armazenada em texto puro nem registrada em logs — apenas o hash Argon2id vai para o banco. Se apenas uma das variáveis estiver definida, o boot falha com erro de configuração.

## Execução

```powershell
cargo run -p api
```

- `GET /health` — processo ativo (não consulta dependências)
- `GET /ready` — dependências prontas (`SELECT 1` no SQLite, timeout de 2 s). Banco
  indisponível responde `503 {"status":"unavailable"}` sem detalhe interno.
- Shutdown gracioso em `Ctrl+C` (e no fechamento da aba, em desenvolvimento), com
  espera configurável das conexões em voo (`SHUTDOWN_DRAIN_SECS`).
- Teto de concorrência HTTP (`MAX_CONCURRENT_REQUESTS`); o excesso responde `503`.

### Autenticação

```text
POST /api/v1/auth/login      { "email", "password" }        → 200 par de tokens
POST /api/v1/auth/refresh    { "refresh_token" }            → 200 par rotacionado
POST /api/v1/auth/logout     { "refresh_token" }            → 204
GET  /api/v1/users/me        Authorization: Bearer <access> → 200 perfil
```

- O access token é um JWT HS256 curto (padrão 15 min) com `sub`, `iat`, `exp`,
  `jti`, `iss` e `aud`. O algoritmo é fixado no código e validado na leitura.
- O refresh token é opaco (256 bits), válido por 30 dias e persistido apenas como
  SHA-256. Cada uso o revoga e emite um sucessor; reapresentar um token já usado
  revoga toda a família daquele usuário.
- Login e refresh têm rate limit em memória: 5 tentativas/min (login) e
  30/min (refresh) por IP, retornando `429` com o tempo restante.
- Erros seguem `application/problem+json` (RFC 7807). Credenciais inválidas,
  e-mail inexistente e conta inativa respondem o mesmo `401`, sem revelar quais
  e-mails existem.

### Documentação interativa

```text
GET /swagger-ui              → Swagger UI (apenas em desenvolvimento)
GET /api-docs/openapi.json   → spec OpenAPI 3 (apenas em desenvolvimento)
```

**Em produção essas rotas não existem** e respondem `404`. O boot rejeita
`APP_ENV=production` combinado com `EXPOSE_API_DOCS=true` em vez de ignorar em silêncio —
do contrário o operador acreditaria que a documentação está no ar.

A regra vale para qualquer console equivalente (Redoc, GraphQL Playground, Scalar,
`/docs`): a API não expõe mapa de rotas fora de desenvolvimento.

### Usuários (RBAC)

```text
GET    /api/v1/users              Admin        lista paginada
POST   /api/v1/users              Admin        cria usuário
GET    /api/v1/users/{id}         Admin|dono   lê o perfil
PATCH  /api/v1/users/{id}         Admin|dono   altera nome, e-mail e/ou senha
PATCH  /api/v1/users/{id}/role    Admin        altera papel (nunca o próprio)
DELETE /api/v1/users/{id}         Admin        soft delete (nunca a própria conta)
```

- O papel e o estado do usuário são **relidos do banco** a cada requisição, não
  confiados das claims: um usuário rebaixado ou desativado perde acesso na hora,
  mesmo com token ainda válido.
- Usuário comum acessa apenas o próprio perfil; qualquer outro `{id}` responde
  `403`, o que fecha a classe de falha IDOR.
- Um Admin não altera o próprio papel nem exclui a própria conta, para o sistema
  nunca ficar sem administrador.
- `DELETE` é soft delete: some das listagens e do `GET`, mas permanece no banco.
- Listagem limita `limit` a 100 e ordena por `created_at` + `id`, garantindo
  paginação estável.

### Catálogo (categorias e produtos)

```text
GET    /api/v1/categories          autenticado   lista paginada
POST   /api/v1/categories          Admin/Manager cria (nome único)
GET    /api/v1/categories/{id}     autenticado   lê
PATCH  /api/v1/categories/{id}     Admin/Manager atualiza nome/descrição
DELETE /api/v1/categories/{id}     Admin/Manager remove (409 se tiver produtos)

GET    /api/v1/products            autenticado   lista com filtros e ordenação
POST   /api/v1/products            Admin/Manager cria (SKU único)
GET    /api/v1/products/{id}       autenticado   lê
PATCH  /api/v1/products/{id}       Admin/Manager atualiza
DELETE /api/v1/products/{id}       Admin/Manager remove (soft delete)
```

- Preço trafega como **inteiro em centavos** (`price_cents`). Nunca `float`:
  ponto flutuante binário não representa `0,10` e acumula erro em somas.
- `sku` é normalizado para maiúsculas, porque `sku-1` e `SKU-1` são o mesmo
  produto comercial e a unicidade no banco é sensível a caixa.
- Estoque `0` é válido (esgotado); negativo é rejeitado com `422`.
- Excluir categoria com produtos responde `409` com a razão; a FK
  `ON DELETE RESTRICT` é a segunda barreira.
- Filtros de produto: `category_id`, `name`, `only_active`.
- Ordenação (`sort`) aceita apenas `newest`, `oldest`, `name`, `cheapest` e
  `most_expensive`; qualquer outro valor responde `400`. O fragmento `ORDER BY`
  vem de um enum no código, nunca do texto do cliente.
- `limit` é truncado em 100 em todas as listagens.

### Servidor vinculado à aba (desenvolvimento)

Com `SHUTDOWN_ON_TAB_CLOSE=true` no `.env`, abra `http://127.0.0.1:8080/`. A página mantém uma sessão SSE; ao fechar a aba, o servidor encerra após 10 segundos de tolerância (um refresh não derruba o processo). A opção é rejeitada quando `APP_ENV=production`.

## Testes e qualidade

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo audit
cargo deny check
cargo doc --workspace --no-deps
```

O `cargo audit` usa `.cargo/audit.toml`. O `cargo deny` usa `deny.toml` (licenças,
advisories, fontes e duplicidade). Exceções temporárias a advisories transitivas
ficam registradas nos dois arquivos, com justificativa, e devem ser reavaliadas a
cada atualização do SQLx.

A revisão de segurança da Fase 7 (auth, IDOR, SQL, segredos) está em
[`docs/checklist-seguranca.md`](docs/checklist-seguranca.md).

### Documentação obrigatória

Todo item de código — módulo, struct, enum, trait, campo, constante, função (inclusive
privada) — precisa de comentário `///` ou `//!` explicando funcionalidade, para que serve
e o que retorna. Isso é imposto por lint, não por convenção: o workspace liga
`missing_docs` e `clippy::missing_docs_in_private_items`, e o CI roda `clippy -D warnings`.
Item sem documentação quebra o build.

Variáveis locais não são cobertas por lint (o Rust não permite doc comment em `let`); para
elas vale comentário `//` quando o nome não bastar.

## Status

- [x] Fase 0 — workspace, configuração, logging, health/ready, CI
- [x] Fase 1 — SQLite, migrations, constraints, pool e readiness com banco
- [x] Fase 2 — domínio de usuários, hashing Argon2id e primeiro Admin
- [x] Fase 3 — login, JWT, refresh, logout
- [x] Fase 4 — RBAC e usuários
- [x] Fase 5 — categorias e produtos
- [x] Fase 6 — OpenAPI e erros padronizados
- [x] Fase 7 — operação e endurecimento
- [ ] Fase 8 — dívidas técnicas e itens fora do escopo (ver [`docs/plano-de-implementacao.md`](docs/plano-de-implementacao.md#fase-8--dívidas-técnicas-e-itens-fora-do-escopo))

## Licença

MIT — veja [`LICENSE`](LICENSE).
