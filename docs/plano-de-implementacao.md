# Plano de Implementação — API Rust

Plano derivado das diretrizes de arquitetura do projeto. Cada fase termina com código compilando sem warnings, testes verdes e documentação atualizada.

## 1. Escopo do MVP

API REST versionada (`/api/v1`) com:

- Autenticação JWT (access curto + refresh com rotação) e Argon2id.
- RBAC: `Admin`, `Manager`, `User`.
- CRUD de usuários, categorias e produtos, com paginação, filtros e ordenação.
- Persistência SQLite via SQLx com migrations versionadas.
- OpenAPI/Swagger (Utoipa) e erros `application/problem+json` (RFC 7807).
- `/health`, `/ready`, logs estruturados e shutdown gracioso.

Fora do escopo inicial (avaliar depois): Redis obrigatório, métricas Prometheus, Dockerfile/compose, multi-tenancy. Consolidados na [Fase 8](#fase-8--dívidas-técnicas-e-itens-fora-do-escopo).

## 2. Stack e versões

| Camada | Crates |
|---|---|
| Runtime/HTTP | `tokio`, `axum`, `tower`, `tower-http` |
| Persistência | `sqlx` (runtime-tokio, postgres, migrate) |
| Auth | `jsonwebtoken`, `argon2`, `rand`/`password-hash` |
| Domínio | `uuid`, `chrono` (UTC), `thiserror`, `serde` |
| Validação | `validator` ou validação manual na borda |
| Docs | `utoipa`, `utoipa-swagger-ui` |
| Observabilidade | `tracing`, `tracing-subscriber`, `tracing-opentelemetry` (opcional) |
| Config | `dotenvy`, `config` ou `envy` |
| Testes | `tokio::test`, `tower::ServiceExt`, `sqlx::test`, `mockall`/fakes |

Regras:

- Dependências centralizadas em `[workspace.dependencies]` no `Cargo.toml` raiz.
- Versões resolvidas no `Cargo.lock`; não atualizar crates principais sem rodar a suíte.
- MSRV declarada (`rust-version`) e `resolver = "3"` se o MSRV permitir.
- Lints no workspace: `unsafe_code = "forbid"` e `clippy::correctness = "deny"`.

## 3. Arquitetura alvo

```text
Cargo.toml                  # workspace + deps compartilhadas + lints
crates/
  domain/                   # entidades, VOs, erros de domínio, políticas de papel
  application/              # casos de uso + portas (traits)
  infrastructure/           # SQLx, Redis, JWT, Argon2id, relógio
  api/                      # Axum, DTOs, middlewares, OpenAPI, bootstrap
migrations/
docs/
tests/                      # apenas se houver testes E2E fora dos crates
```

Fluxo de dependências: `api -> application -> domain`; `infrastructure -> application/domain`.

**Domain (sem I/O):** `User`, `Email`, `PasswordHash`, `Role`, `Category`, `Product`, `Sku`, `Money`, `EntityStatus`, `DomainError`.
**Application:** traits `UserRepository`, `CategoryRepository`, `ProductRepository`, `PasswordHasher`, `TokenService`, `Clock`, `RefreshTokenStore`; casos de uso pequenos (`CreateProduct`, `AuthenticateUser`, `ChangeUserRole`, ...).
**Infrastructure:** `SqlxUserRepository`, `SqlxCategoryRepository`, `SqlxProductRepository`, `Argon2PasswordHasher`, `JwtTokenService`, `RedisRefreshTokenStore` (atrás de feature/`Option`).
**API:** handlers finos, extractors de auth, middleware de request ID, mapeamento de erros, composição do `AppState`.

## 4. Fases

### Fase 0 — Fundação e CI (concluída)

- [x] `cargo init` + workspace com os 4 crates e `lib.rs` mínimos.
- [x] `rust-toolchain.toml` (stable) e `rust-version` no workspace.
- [x] `AppConfig` tipada e validada no boot (`APP_HOST`, `APP_PORT`, `DATABASE_URL`, `JWT_SECRET`, `JWT_ACCESS_TTL_SECS`, `RUST_LOG`).
- [x] `tracing-subscriber` com JSON em produção e pretty em dev.
- [x] `GET /health` e `GET /ready`.
- [x] `.env.example` (sem segredos), `.gitignore` revisado.
- [x] CI (GitHub Actions): `fmt --check`, `clippy -D warnings`, `test`, `cargo audit`.
- [x] `README` inicial: pré-requisitos, configuração, execução.

**Aceite:** `cargo run` sobe o servidor, `/health` = 200 e `/ready` = 200 (a checagem do banco entra na Fase 1); CI verde.

### Fase 1 — Persistência base (concluída)

- [x] Migration inicial `migrations/20261114000001_init.sql` (users, categories, products, refresh_tokens) aplicada por `sqlx::migrate!` embutido — não depende de `sqlx-cli`.
- [x] Constraints: `UNIQUE` parcial em e-mail/SKU/nome de categoria (somente ativos), FK `products.category_id` com `ON DELETE RESTRICT`, `CHECK(stock >= 0)`, `CHECK(price_cents > 0)`.
- [x] Índices para categoria, nome de produto, usuário e expiração de refresh tokens.
- [x] Pool SQLite com `foreign_keys=ON`, `journal_mode=WAL`, `busy_timeout=5s`, `synchronous=NORMAL`.
- [x] Migrations embutidas no binário, idempotentes, com `RUN_MIGRATIONS` configurável.
- [x] Testes de schema contra SQLite isolado: unicidade parcial, soft delete, constraints de produto e cascade de refresh tokens.
- [ ] Repositórios SQLx — entram junto das entidades: `User` na Fase 2 e `Category`/`Product` na Fase 5 (evita tipos descartáveis e respeita a direção das dependências).
- [x] `/ready` verifica o banco e retorna `503` quando indisponível.

**Aceite:** migrations aplicam do zero e em banco já migrado; constraints e foreign keys cobertas por testes; `/ready` reflete o banco.

### Fase 2 — Domínio de usuários e hashing (concluída)

- [x] VOs `Email` (trim + minúsculas, formato `local@dominio`), `PasswordHash` (PHC Argon2id), `Role` (`admin`/`manager`/`user`) e `EntityStatus` (`active`/`inactive`).
- [x] Invariantes de `User` (nome, e-mail, hash, papel e estado) via `User::new`/`User::restore`; mutações atualizam `updated_at` sem retroceder e soft delete é idempotente.
- [x] Política de senha no domínio: 12–128 caracteres, minúscula, maiúscula, número e símbolo.
- [x] `Argon2PasswordHasher` como adapter da porta `PasswordHasher`, em `spawn_blocking` (Argon2id v19, parâmetros padrão).
- [x] `SqlxUserRepository` atendendo à porta `UserRepository`; consultas filtram `deleted_at IS NULL` e violação do índice parcial vira `EmailAlreadyInUse`.
- [x] Provisionamento idempotente do primeiro Admin via `ADMIN_EMAIL`/`ADMIN_PASSWORD`, sem senha padrão e sem logar e-mail/senha/hash.
- [x] Testes unitários de invariantes e do hasher, testes de caso de uso com fakes e testes de repositório contra SQLite isolado.

**Aceite:** nenhuma senha em texto puro ou hash em DTO/log; admin inicial criado por rotina explícita e documentada. O segundo boot registra `admin inicial já existe` e não altera a linha.

**Evidência de verificação (fases 0–2):**

```text
cargo fmt --all --check                                  → ok
cargo clippy --workspace --all-targets -- -D warnings     → ok
cargo test --workspace                                    → 91 testes, 0 falhas
cargo audit                                               → exit 0, 242 deps, sem vulnerabilidades

Boot real (banco descartável, ambiente sem variáveis residuais):
  GET /health  → 200 {"status":"ok"}
  GET /ready   → 200 {"status":"ready"}
  cabeçalho x-request-id presente
  rota inexistente → 404
  tabelas criadas: _sqlx_migrations, categories, products, refresh_tokens, users

Seed do Admin em dois boots consecutivos:
  boot 1 → "admin inicial criado"
  boot 2 → "admin inicial já existe; nenhuma alteração feita"
  banco após boot 1: 1 usuário admin/active, hash `$argon2id`, updated_at == created_at

Logs:
  APP_ENV=production  → JSON estruturado
  APP_ENV=development → formato legível
  RUST_LOG=debug      → statements SQL aparecem com placeholders `?`, nunca os valores
  nenhuma ocorrência de `argon2id`, senha ou segredo no log

Configuração inválida falha no boot, antes de abrir a porta:
  sem JWT_SECRET              → "configuração obrigatória ausente: JWT_SECRET"
  JWT_SECRET com 5 caracteres → "deve ter pelo menos 32 caracteres"
  ADMIN_EMAIL sem senha       → "configuração obrigatória ausente: ADMIN_PASSWORD"
  senha fraca                 → "deve ter pelo menos 12 caracteres"
  produção + tab close        → "não pode ser habilitado em produção"
  porta ocupada               → "Address already in use (os error 98)"
```

Testes criados para fechar lacunas de aceite:

- `crates/infrastructure/tests/ensure_admin.rs` — aceite da Fase 2 contra SQLite real: hash Argon2id persistido (e não a senha), `updated_at` e `password_hash` intactos no segundo boot, senha do segundo boot ignorada, reprovisionamento após soft delete.
- `migrations.rs` — unicidade de **nome de categoria** entre registros ativos (com liberação após soft delete), `PRAGMA busy_timeout = 5000` e `synchronous = 1 (NORMAL)`.
- `domain` — `Debug` de `PasswordHash` e de `User` não expõem o hash (correção de vazamento real: `User` derivava `Debug` e imprimia o hash PHC completo).

Todas as asserções novas foram validadas por mutação: alterar o timeout para 1 s, trocar `NORMAL` por `FULL`, remover o `Debug` manual ou regravar a linha do Admin faz cada teste correspondente falhar.



### Fase 3 — Autenticação (concluída)

- [x] Claims mínimas (`sub`, `exp`, `iat`, `jti`, `iss`, `aud`), algoritmo fixado e validação completa.
- [x] Access token curto; refresh token opaco, armazenado com hash, rotação e detecção de reutilização.
- [x] `POST /auth/login`, `/auth/refresh`, `/auth/logout`.
- [x] Rate limiting em login/refresh (em memória; Redis quando configurado).
- [x] `GET /users/me`.
- [x] Testes: senha inválida, token expirado/assinatura inválida, usuário inativo, refresh reutilizado.

**Aceite:** fluxo completo login → acesso → refresh → logout coberto por testes E2E; nenhum token em logs.

**Evidência de verificação:**

```text
cargo test --workspace                                → 124 testes, 0 falhas
  crates/api/tests/auth_flow.rs                       → 10 testes E2E
  crates/infrastructure/tests/auth.rs                 → 11 testes de rotação/reuso
  crates/infrastructure/tests/token.rs                → 7 testes de JWT
  crates/api/src/rate_limit.rs (unitários)            → 5 testes de janela

Servidor real (banco descartável):
  POST /api/v1/auth/login      → 200, Bearer, expires_in 900, refresh 64 hex
  GET  /api/v1/users/me        → 200 {"email":"admin@example.com","role":"admin"}
  POST /api/v1/auth/refresh    → 200, refresh rotacionado
  POST /api/v1/auth/logout     → 204
  POST /api/v1/auth/refresh    → 401 (token revogado)

  senha errada                 → 401 application/problem+json
  sem token / token inválido   → 401 application/problem+json
  tentativas 1–5               → 401
  tentativa 6 e 7              → 429 "tente novamente em 57 segundos"

Logs com RUST_LOG=debug: 0 ocorrências de `eyJ`, `argon2id` ou senha.
```

**Decisões de segurança tomadas:**

- Algoritmo `HS256` fixo no código; `Validation::new` + lista de algoritmos impede `alg: none` e troca de chave. Teste dedicado forja um token `alg: none` e confirma a rejeição.
- `iss` e `aud` obrigatórios, com 30 s de tolerância de relógio.
- Refresh token com 256 bits de entropia (`OsRng`), persistido apenas como SHA-256. SHA-256 e não Argon2 porque o token já é inadivinhável; Argon2 só atrasaria cada renovação.
- Reutilização de refresh revoga **toda a família** do usuário — validado por mutação: desativar a checagem faz o teste falhar.
- Login não distingue e-mail inexistente, senha errada e conta inativa na resposta (`401` genérico), evitando enumeração de contas.
- `GET /users/me` relê o usuário do banco: token válido de conta desativada após a emissão recebe `403`.
- Rate limit aplicado **antes** do Argon2, para que tentativas repetidas não consumam CPU.

**Fora do escopo desta fase:** rate limit e revogação distribuídos via Redis (hoje em memória/SQLite) e `Redis` apenas registrado no boot — ver [Fase 8 — 8.1 e 8.4](#fase-8--dívidas-técnicas-e-itens-fora-do-escopo).


### Fase 4 — RBAC e usuários (concluída)

- [x] Middleware/extractor de autenticação e políticas centralizadas por operação.
- [x] `GET/POST/PATCH/DELETE /users` com regras Admin vs. próprio usuário (anti-IDOR).
- [x] `ChangeUserRole` com validação de escalonamento de privilégio.
- [x] Testes de matriz de permissões por papel e por proprietário.

**Aceite:** 401/403 corretos em todos os casos; usuário comum não altera papel nem dados de terceiros.

**Evidência de verificação:**

```text
cargo test --workspace                                → 166 testes, 0 falhas
  crates/api/tests/rbac.rs                            → 22 testes de matriz de permissões
  crates/application/src/policy.rs (unitários)        → 8 testes de política

Servidor real, com três usuários (admin/manager/user):
  usuário comum  listar todos              → 403
  usuário comum  ler perfil do admin (IDOR)→ 403
  usuário comum  ler o próprio perfil      → 200
  usuário comum  se promover a admin       → 403
  usuário comum  criar usuário             → 403
  manager        listar todos              → 403
  admin          se rebaixar               → 403
  admin          criar gerente e comum     → 201
  admin          listar                   → 200 (total 3)
```

**Decisões de segurança:**

- Política única em `application::policy`. Handlers não decidem permissão; delegam para `UserAction::authorize`.
- O **ator é relido do banco** a cada requisição (`resolve_actor`), nunca inferido apenas das claims. Teste dedicado prova que um Manager rebaixado perde acesso imediatamente, mesmo com token emitido antes.
- Admin não pode alterar o próprio papel nem excluir a própria conta — impede que o sistema fique sem administrador.
- `DELETE` é soft delete: o registro sai das listagens e do `GET`, mas permanece no banco.
- ID malformado na URL responde `404` (não `422`): o recurso não existe.
- Paginação com `limit` limitado a 100 e ordenação estável (`created_at`, `id`), para não repetir ou pular linhas entre páginas.
- A resposta de usuário **nunca** inclui `password_hash`.

**Validação por mutação** (a proteção precisa falhar quando removida):

- Remover a checagem de dono em `View`/`UpdateProfile` → 2 testes de IDOR falham (`left: 200, right: 403`).
- Remover `require_admin` de `ChangeRole` → 3 testes de escalonamento falham.

**Fora do escopo:** rate limit nas rotas de usuários e auditoria de quem alterou qual usuário — ver [Fase 8 — 8.1 e 8.2](#fase-8--dívidas-técnicas-e-itens-fora-do-escopo).


### Fase 5 — Categorias e produtos (concluída)

- [x] CRUD de categorias com nome único e regra explícita ao excluir categoria com produtos (conflito 409).
- [x] CRUD de produtos com `Sku` único, `Money` em centavos e estoque não negativo.
- [x] Listagens com paginação limitada, filtros e ordenação por allowlist.
- [x] Transações onde houver múltiplas gravações atômicas.
- [x] Testes de domínio, casos de uso, repositório e HTTP.

**Aceite:** invariantes garantidas no domínio e no banco; ordenação inválida retorna 400; paginação tem limite máximo.

**Evidência de verificação:**

```text
cargo test --workspace                                → 246 testes, 0 falhas
  domain (money, sku, category, product)              → 70 testes de invariantes
  application (manage_catalog)                        → testes de permissão e conflito
  infrastructure/tests/catalog.rs                     → 12 testes contra SQLite real
  api/tests/catalog_flow.rs                           → 20 testes HTTP

Servidor real:
  POST /api/v1/categories  {"name":"Periféricos"}     → 201
  POST /api/v1/products    sku "tec-001"              → 201, sku normalizado "TEC-001",
                                                        price_cents 34990, stock 12
  DELETE /api/v1/categories/{id} (com produto)        → 409
      "categoria com produtos ativos não pode ser removida; remova ou mova os produtos antes"
  GET /api/v1/products?sort=price; DROP TABLE         → 400
  GET /api/v1/products?limit=100000                   → limit retornado: 100
  POST /api/v1/products com price_cents: 0            → 422
  POST /api/v1/products como usuário comum            → 403
```

**Decisões de domínio:**

- `Money` é `i64` em centavos. Nunca `f32`/`f64`: ponto flutuante binário não representa `0.10` e acumula erro em somas. A API transporta `price_cents` e não converte para decimal na saída.
- `Money::from_cents` rejeita zero e negativo, espelhando `CHECK (price_cents > 0)` — domínio e banco concordam.
- `Sku` normaliza para maiúsculas: `sku-1` e `SKU-1` são o mesmo produto comercial e a unicidade no banco é sensível a caixa.
- Estoque zero é **válido** (produto esgotado); negativo não.
- Excluir categoria com produtos é `409`, com a FK `ON DELETE RESTRICT` como segunda barreira. Teste confirma que o `DELETE` direto no SQL também falha.

**Segurança da listagem:**

- Filtros usam placeholders; só valores são vinculados.
- `ORDER BY` vem de `ProductSort`, um enum. A allowlist é o que impede injeção, já que `ORDER BY` não aceita parâmetro vinculado.
- Curingas `%` e `_` na busca são escapados, então buscar `100%` não casa com qualquer coisa começando em `100`.
- `limit` é truncado em 100.

**Validação por mutação:**

- Remover a checagem de produtos antes de excluir categoria → teste falha (`left: 204, right: 409`).
- Aceitar ordenação arbitrária em `ProductSort::parse` → teste falha (`left: 200, right: 400`).

**Fora do escopo:** estoque como histórico de movimentações, histórico de preço e transações explícitas — ver [Fase 8 — 8.3](#fase-8--dívidas-técnicas-e-itens-fora-do-escopo).


### Fase 6 — OpenAPI e erros padronizados (concluída)

- [x] `utoipa` em todos os DTOs/rotas + Swagger UI em `/swagger-ui`.
- [x] Erros RFC 7807 (`type`, `title`, `status`, `detail`, `instance`) sem detalhes internos.
- [x] Request ID correlacionado em logs e respostas de erro.
- [x] CORS com origens explícitas, limite de corpo e timeouts.

**Aceite:** spec gerada cobre 100% das rotas; testes conferem formato de erro e headers de segurança.

**Evidência de verificação:**

```text
cargo test --workspace                                → 271 testes, 0 falhas
  crates/api/tests/api_docs_environment.rs            → 7 testes (dev vs produção)
  crates/api/tests/hardening_http.rs                  → 8 testes (CORS, limite, headers)
  crates/api/src/api_docs.rs (unitários)              → 3 testes de cobertura da spec

Desenvolvimento:
  /swagger-ui               → 303 (redirect para o index da UI)
  /api-docs/openapi.json    → 200, 13 rotas, 20 schemas, 4 tags
  headers                   → nosniff, X-Frame-Options: DENY, no-referrer,
                              permissions-policy, cache-control: no-store
  erro 401                  → {"type","title","status","detail","instance":"<request-id>"}

Produção (ambiente 100% limpo, via `env -i`):
  /swagger-ui               → 404
  /api-docs/openapi.json    → 404
  /health                   → 200
  boot com EXPOSE_API_DOCS=true
    → "configuração inválida em EXPOSE_API_DOCS: documentação interativa
       não pode ser habilitada em produção" (processo aborta)
  CORS_ALLOWED_ORIGINS=*
    → "configuração inválida em CORS_ALLOWED_ORIGINS: `*` não é permitido"
```

**Regra do projeto:** Swagger UI, OpenAPI, Redoc, GraphQL Playground, Scalar e qualquer
console interativo existem **apenas em desenvolvimento**. Em produção as rotas não são
montadas e respondem `404` — não `403`, que confirmaria a existência do endpoint. A regra
está registrada em `.cursor/rules/docs-api-so-em-dev.mdc`.

**Decisões de segurança:**

- `expose_api_docs` é derivado do ambiente. `EXPOSE_API_DOCS` pode **desabilitar** em
  desenvolvimento (para simular produção), mas tentar **habilitar** em produção aborta o
  boot. Ignorar em silêncio deixaria o operador acreditando que a docs está no ar.
- CORS sem `CorsLayer::permissive()`: sem origens configuradas, nenhuma origem cruzada é
  aceita. `*` é rejeitado no boot.
- `instance` carrega o request ID via `task_local`, porque handlers devolvem `Response`
  já formatada e não têm mais acesso à requisição naquele ponto.
- Limite de corpo de 1 MiB e timeout de 30 s aplicados **antes** do handler, com o `413`
  e o `408` corretos.

**Validação por mutação:** montar o Swagger ignorando a flag faz o teste falhar
(`left: 303, right: 404`).

**Fora do escopo:** versionamento da spec, geração de clientes e `instance` como URL
absoluta — ver [Fase 8](#fase-8--dívidas-técnicas-e-itens-fora-do-escopo).


### Fase 7 — Operação e endurecimento (concluída)

- [x] Shutdown gracioso (`Ctrl+C` e fechamento da aba), teto de
      requisições simultâneas (`MAX_CONCURRENT_REQUESTS`, excesso → 503) e
      timeout de drain (`SHUTDOWN_DRAIN_SECS`) após o sinal.
- [x] Timeouts de request (30 s) e de `/ready` (2 s no ping do banco) já
      aplicados na borda HTTP.
- [x] `cargo audit` e `cargo deny` no CI (`deny.toml` cobre licenças, advisories,
      fontes e wildcards).
- [x] Revisão de segurança guiada por `security-audit` (modo orientação):
      [`docs/checklist-seguranca.md`](checklist-seguranca.md), foco em auth,
      IDOR, SQL e segredos.
- [ ] Opcional adiado: métricas Prometheus, Dockerfile multi-stage e
      `docker-compose` — ver [Fase 8.4](#fase-8--dívidas-técnicas-e-itens-fora-do-escopo).

**Aceite:** checklist de segurança revisado; nenhum segredo versionado; `/ready` falha de forma segura sem dependências.

**Evidência de verificação:**

```text
cargo fmt --all --check                               → ok
cargo clippy --workspace --all-targets -- -D warnings → ok
cargo test --workspace                                → 280 testes, 0 falhas
cargo audit                                           → exit 0
cargo deny check                                      → advisories/bans/licenses/sources ok
scripts/check-local-docs.sh                           → ok
cargo doc --workspace --no-deps                       → ok (-D warnings)

Novos testes da fase:
  crates/api/src/operation.rs                         → drain só após o sinal
  crates/api/tests/hardening_http.rs                  → excesso de concorrência → 503
  crates/api/tests/http.rs                            → /ready 503 sem detalhe interno
  crates/api/tests/secrets.rs                         → .env fora do Git, exemplo sem senha

Servidor real (0.0.0.0:8080, banco data/api.db):
  GET  /health              → 200 {"status":"ok"}
  GET  /ready               → 200 {"status":"ready"}
  GET  /api/v1/users/me     → 401 sem token
  GET  /swagger-ui          → 303
  POST /api/v1/auth/login   → 200 (access + refresh)
  GET  /api/v1/users/me     → 200 perfil admin (com Bearer)
  log: max_concurrent=128 drain_secs=30
```

**Decisões de operação:**

- Excesso de concorrência é **recusado** (semáforo no middleware → 503), não
  enfileirado: enfileirar esconderia a sobrecarga até estourar memória. O
  `ConcurrencyLimitLayer` do tower não serve no Axum porque o router devolve
  `Ready` ao spawnar o handler.
- `/ready` nunca inclui a mensagem do SQLx no corpo: o orquestrador só precisa
  de `unavailable`. O detalhe vai para o log.
- Segredos versionados são regressão coberta por `crates/api/tests/secrets.rs`
  (`.gitignore`, `.env.example` e `git ls-files .env`).

### Fase 8 — Dívidas técnicas e itens fora do escopo (concluída)

**Inclui a migração de SQLite para PostgreSQL**, solicitada durante a fase. Ver
[Migração para PostgreSQL](#fase-8--migração-para-postgresql).

Itens conscientemente adiados nas fases anteriores. Ordenados por risco.

#### 8.1 Estado distribuído (risco alto em múltiplas réplicas)

- [x] **Rate limit no Redis.** `infrastructure/src/redis_state.rs` executa
      `INCR` + `PEXPIRE` no mesmo script Lua: dois comandos separados deixariam
      a chave sem TTL se a conexão caísse no meio, bloqueando o IP para sempre.
      A porta `RateLimitStore` troca Redis/memória sem tocar nos casos de uso.
- [x] **Revogação de access token distribuída.** `AccessTokenDenylist` grava o
      `jti` com `SET ... EXAT <exp>`, então a entrada expira junto com o token e
      a denylist não cresce sem limite. O logout revoga o access token corrente,
      e o extractor consulta a denylist **falhando fechado** quando ela está fora.
- [x] **Rate limit nas rotas de usuários e catálogo.** Middleware
      `read_limit::middleware` no grupo autenticado: 300 req/min por IP + rota
      normalizada. Antes, `GET /api/v1/products` era scraping gratuito.

#### 8.2 Auditoria e rastreabilidade

- [x] **Trilha de auditoria.** `audit_log` (`actor_id`, `action`, `entity`,
      `entity_id`, `before`, `after`, `at`) escrita pelos casos de uso de
      usuários, categorias e produtos; consultável em `GET /api/v1/audit`
      (Admin). FK em `actor_id` impede autor fantasma e `CHECK` em
      `entity`/`action` é a segunda barreira.
- [x] **`instance` no RFC 7807.** Entregue na Fase 6 (request ID no corpo).

#### 8.3 Modelo de dados que perdeu histórico

- [x] **Histórico de movimentação de estoque.** `stock_movements` com `delta` e
      `reason` (`initial`, `correction`, `sale`, `loss`, `restock`). Delta zero é
      recusado no domínio e pelo `CHECK`.
- [x] **Histórico de preço.** `price_changes` com `old_cents`/`new_cents`; valor
      igual ao atual é recusado — histórico sem mudança é ruído.
- [x] **Transações explícitas onde houver múltiplas gravações.**
      `insert_product`/`update_product` gravam produto **e** histórico na mesma
      transação. Teste de rollback prova que SKU duplicado não deixa movimento
      órfão.

#### 8.4 Plataforma e operação

- [x] **Redis obrigatório em produção.** O boot aborta sem `REDIS_URL` quando
      `APP_ENV=production`; em desenvolvimento avisa e usa memória.
- [x] **Métricas Prometheus** em `/metrics`, protegidas por `METRICS_TOKEN`
      (cabeçalho `x-metrics-token`). Sem token, a rota não é montada (`404`).
      Rótulo `route` normalizado (`/api/v1/users/{id}`) evita explosão de
      cardinalidade.
- [x] **Dockerfile multi-stage** e **`docker-compose`** com Redis local
      (`Dockerfile`, `docker-compose.yml`, `.dockerignore`). Imagem roda como
      usuário sem privilégios e usa `tini` para repassar `SIGTERM`.
- [x] **`cargo deny`** além do `cargo audit` — entregue na Fase 7.
- [ ] **Multi-tenancy.** Fora do escopo: o produto atende um único cliente.
      Mudar isso é uma fase própria (isolamento toca schema, autorização e todas
      as consultas).

**Aceite: todos os itens de 8.1 foram verificados contra o Redis real**
(`192.168.2.200:6379`). Evidência em
[Verificação do Redis distribuído](#verificação-do-redis-distribuído).

**Evidência de verificação:**

```text
cargo fmt --all --check                               → ok
cargo clippy --workspace --all-targets -- -D warnings → ok
cargo test --workspace                                → 324 testes, 0 falhas
RUSTDOCFLAGS=-Dwarnings cargo doc --workspace          → ok
scripts/check-local-docs.sh                           → ok
cargo audit / cargo deny check                        → advisories/bans/licenses/sources ok

Novos testes da fase:
  domain/src/history.rs                               → 5 (motivos, delta zero, preço igual)
  infrastructure/tests/history_audit.rs               → 11 (atomicidade, rollback, ordem)
  infrastructure/tests/redis.rs                       → 5 contra Redis real (0 SKIP)
  api/tests/fase8.rs                                  → 11 (histórico, auditoria, logout, métricas)
  api/tests/config_fase8.rs                           → 6 (config de Redis e métricas)
  api/src/metrics.rs (unitários)                      → 4 (normalização de rota, token, encoder)

Servidor real (0.0.0.0:8080, banco data/api.db, **com Redis**):
  GET  /health                       → 200
  GET  /ready                        → 200
  GET  /metrics (sem token)          → 404
  GET  /metrics (com token)          → 200 text/plain, api_http_requests_total
  POST /api/v1/auth/login            → 200
  POST /api/v1/auth/login ×6         → 401,401,401,401,401,429 (limite no Redis)
  POST /api/v1/categories            → 201
  POST /api/v1/products (stock 10)   → 201
  PATCH /api/v1/products/{id}        → 200 (stock 4, price 12000)
  GET  .../stock-movements           → 200: delta -6 "correction", delta 10 "initial"
  GET  .../price-changes             → 200: 9900 → 12000
  GET  /api/v1/audit?entity=product  → 200: "create" e "update", com actor_id
  GET  /api/v1/products ×3           → 200,200,200 (contador no Redis: 3, TTL 60 s)
  logout com Bearer                  → 204; jti na denylist com TTL 899 s
  GET  /api/v1/users/me (após)       → 401 (access revogado via Redis)
  GET  /api/v1/products (sem token)  → 401
  /swagger-ui                        → 303
  log: "Redis conectado: rate limit e revogação distribuídos ativos"
```

#### Verificação do Redis distribuído

Executada contra o Redis real de `REDIS_URL` (`redis://admin:***@192.168.2.200:6379`).

**Testes de integração** — `crates/infrastructure/tests/redis.rs`:

```text
REDIS_URL=<do .env> cargo test -p infrastructure --test redis -- --nocapture
  rate_limit_counts_and_blocks_after_the_limit ... ok
  rate_limit_is_shared_across_connections      ... ok
  denylist_revokes_and_expires                 ... ok
  denylist_entry_expires_with_the_token        ... ok
  ping_reports_healthy_connection              ... ok
  test result: ok. 5 passed; 0 failed   (0.08s, nenhum SKIP)
```

**Controle negativo** (porta fechada, `redis://127.0.0.1:6399`): cada teste
imprime `SKIP: não foi possível conectar ao Redis (conexão não completou em 5 s)`.
Isso prova que a execução acima foi real, e não um skip silencioso.

**Rate limit de login pela API** (`POST /api/v1/auth/login`, IP 127.0.0.1):

```text
tentativa 1..5 → 401   (credencial inválida, contabilizada)
tentativa 6    → 429   (limite de 5/min atingido)
tentativa 7    → 429
```

Chave observada no Redis:

```text
ratelimit:login:127.0.0.1   contador=6   TTL≈59876 ms
```

O contador é a **única** fonte de verdade: reiniciar a API não reabre a janela.

**Logout distribuído** (`POST /api/v1/auth/logout` com `Authorization`):

```text
GET /api/v1/users/me  antes   → 200
POST /auth/logout             → 204
GET /api/v1/users/me  depois  → 401
```

Chave criada no Redis, com TTL igual ao `exp` do token (900 s de TTL
configurado, observados 899 s):

```text
denylist:jti:9c262055-16f7-4ebe-9fc8-ce3ce4c83a3d   EXISTS=1   TTL=899 s
```

**Rate limit de leitura** (`GET /api/v1/products`, rotas autenticadas):

```text
3 requisições → 200, 200, 200
ratelimit:read:/api/v1/products:127.0.0.1   contador=3   TTL=60 s
```

**Log de boot** com Redis presente:

```text
INFO api: migrations aplicadas
INFO api: Redis conectado: rate limit e revogação distribuídos ativos
INFO api: métricas Prometheus habilitadas em /metrics
INFO api: servidor iniciado address=0.0.0.0:8080 max_concurrent=128 drain_secs=30
```

#### Correção encontrada durante a verificação

O teste de conexão sem Redis **travava indefinidamente**: a configuração padrão
do `ConnectionManager` não tem timeout de conexão nem de resposta, e o laço
interno de retentativa pode demorar bem mais que o esperado. Consequência real
em produção: um Redis que aceita TCP e não responde deixaria o **boot
pendurado**, sem log de erro.

Corrigido em `infrastructure/src/redis_state.rs` com três limites:

| Limite | Valor | Motivo |
|---|---|---|
| `CONNECT_TIMEOUT` | 2 s | handshake TCP não pode pendurar o boot |
| `RESPONSE_TIMEOUT` | 2 s | Redis lento não segura a requisição HTTP |
| `TOTAL_CONNECT_BUDGET` | 5 s | orçamento externo (`tokio::time::timeout`) sobre as retentativas |

Depois da correção, o cenário sem Redis falha em 5 s com mensagem clara, em vez
de travar. É a diferença entre "o Redis caiu e a API não sobe" e "a API não sobe
e ninguém sabe por quê".

**Decisões da fase:**

- Rate limit usa janela **fixa** com `INCR`+`PEXPIRE` atômico, e não janela
  deslizante com sorted set. A janela fixa permite um pico de até 2× o limite na
  virada; em troca, custa uma chave e um comando por tentativa. Para conter força
  bruta, o pior caso de 2× é aceitável; para cobrança por uso, não seria.
- Denylist usa `EXAT` (expiração absoluta) e não TTL relativo: o `exp` do JWT é
  absoluto, e um TTL relativo renovaria a vida da entrada a cada consulta.
- Denylist indisponível = **recusa** (`401` no extractor, `429` no rate limit).
  Aceitar sem verificar reabriria exatamente o buraco que a denylist fecha.
- Histórico e produto compartilham a transação. Um saldo alterado sem linha de
  histórico é pior que uma requisição que falha, porque o erro é silencioso.
- Auditoria nunca guarda `password_hash`: a trilha é legível por Admin e não deve
  virar um segundo caminho para material de autenticação.
- `/metrics` sem token responde `404`, não `401`/`403`: negar com um status que
  confirma a existência da rota entrega uma pista de reconhecimento.


### Fase 8 — Migração para PostgreSQL

Solicitada durante a fase: trocar SQLite por PostgreSQL (`192.168.2.200`, porta
**5432** — a 5435 informada está fechada), com PostgreSQL como único backend e
isolamento de teste por schema.

O que mudou no código:

| Item | Antes | Depois |
|---|---|---|
| Feature do `sqlx` | `sqlite` | `postgres` |
| Pool | `SqlitePool` | `PgPool` com `search_path` por schema |
| Placeholders | 227 ocorrências de `?` | `$1..$n` (dinâmico em `products`/`audit`) |
| `price_cents`, `stock`, `delta`, `*_cents` | `INTEGER` | `BIGINT` |
| Erro de unicidade | texto `users.email` | `SQLSTATE 23505` + nome da constraint |
| Teste | `:memory:` por processo | schema descartável por teste |

**Bugs encontrados durante a migração** (nenhum seria pego só compilando):

1. **`INTEGER` é `INT4` no PostgreSQL.** Um `SELECT` de `price_cents` em `i64`
   falhava com `mismatched types`, o que virava `500` na listagem de produtos.
   Corrigido trocando as colunas para `BIGINT`.
2. **Placeholders multi-linha reiniciavam a numeração.** A conversão mecânica
   gerou `updated_at = $1, deleted_at = $2 WHERE id = $3` num `UPDATE` de seis
   campos; o PostgreSQL recusa placeholder repetido. Detecção: auditoria que
   valida, para cada literal SQL, se os `$n` formam `1..N` sem repetição.
3. **`ConnectionManager` sem timeout travava o boot.** Testado de propósito com
   Redis inacessível: a chamada não retornava e o teste pendurava. Corrigido com
   `CONNECT_TIMEOUT` (2 s), `RESPONSE_TIMEOUT` (2 s) e um orçamento externo de
   5 s via `tokio::time::timeout`.
4. **Schemas de teste vazavam.** 121 schemas órfãos acumulados no servidor. A
   limpeza por `Drop` não servia (o `Database` é movido para o `AppState` dentro
   de uma função auxiliar, e a guarda morria antes do teste usar o banco).
   Solução: varredura que remove schemas cujo PID dono já não existe.

**Evidência:**

```text
cargo test --workspace (TEST_DATABASE_URL=<postgres>)   → 346 testes, 0 falhas
cargo clippy --workspace --all-targets -- -D warnings    → ok
RUSTDOCFLAGS=-Dwarnings cargo doc --workspace            → ok
scripts/check-local-docs.sh                              → ok
cargo deny check                  → advisories/bans/licenses/sources ok

Servidor real (0.0.0.0:8080, PostgreSQL + schema apirust_dev + Redis):
  GET  /health                        → 200
  GET  /ready                         → 200
  POST /api/v1/auth/login             → 200 (296 chars de token)
  POST /api/v1/categories             → 201
  POST /api/v1/products (stock 10)    → 201
  PATCH /api/v1/products/{id}         → 200 (stock 4, price 12000)
  GET  .../stock-movements            → 200: delta -6 "correction", delta 10 "initial"
  GET  /api/v1/audit?entity=product   → 200: create + update, com actor_id
  GET  /api/v1/products?category_id=… → 200: total 1 (antes: 500)
  POST /auth/logout                   → 204; depois: /users/me → 401
  log: "usando schema dedicado do PostgreSQL schema=apirust_dev"
  log: "Redis conectado: rate limit e revogação distribuídos ativos"

Banco: schema apirust_dev com 8 tabelas (_sqlx_migrations + 7 do domínio),
       1 usuário, 2 movimentações; schema public intacto (0 tabelas).
```

## 5. Estratégia de testes

1. **Unitários (`domain`, `application`)** — invariantes, VOs, políticas de papel, casos de uso com fakes.
2. **Integração (`infrastructure`)** — cada repositório contra SQLite temporário; migrations reais.
3. **HTTP/E2E (`api`)** — `Router` em memória com `tower::ServiceExt`, auth real via JWT de teste.
4. **Segurança** — expiração, assinatura inválida, refresh reuse, IDOR, papel insuficiente, rate limit.

Comandos de verificação antes de concluir qualquer fase:

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo audit
```

## 6. Riscos e mitigação

| Risco | Mitigação |
|---|---|
| Acoplamento do domínio a SQLx/Axum | CI proíbe deps de framework em `domain` (revisão de `Cargo.toml` + `cargo tree`) |
| Divergência entre validação de app e constraints | Constraints no banco + testes que esperam erro de constraint |
| Refresh token mal implementado | Rotação com hash, uso único, teste de reutilização |
| SQLite em concorrência | WAL, pool limitado, transações curtas; SQLx + `busy_timeout` |
| Migrations já aplicadas serem reescritas | Política no README + revisão em PR |
| Vazamento de segredos | `.env.example`, `.gitignore`, `cargo audit`, revisão de logs |

## 7. Definição de pronto (por fase)

- Compila sem warnings; `fmt` e `clippy -D warnings` limpos.
- Testes novos e existentes passam (`cargo test --workspace`).
- OpenAPI e README atualizados quando rotas/DTOs mudam.
- Sem segredos, hashes ou tokens em logs/respostas.
- Limitações e verificações não executadas reportadas explicitamente.

## 8. Decisões tomadas

| Decisão | Escolha | Impacto |
|---|---|---|
| Redis | Memória primeiro; adapter Redis atrás de feature flag | Rate limit e refresh store em memória na Fase 3; portas permitem trocar depois |
| Primeiro Admin | Seed por variáveis `ADMIN_EMAIL`/`ADMIN_PASSWORD` no primeiro boot | Seed idempotente, desabilitado quando as variáveis não existem |
| Política de senha | 12–128 caracteres com minúscula, maiúscula, número e símbolo | Validada no domínio e no boot do Admin |
| Exclusão | Soft delete (`deleted_at`) desde o início | Todas as consultas filtram `deleted_at IS NULL`; unicidade via índice parcial |
| Migrations | Automáticas no boot por padrão (`RUN_MIGRATIONS=true`) | `false` delega ao deploy (`sqlx migrate run`) |
| Escopo | Fases 0–8 entregues; multi-tenancy permanece fora do escopo | — |

### Ambiente Redis (observação)

Redis 7 está em container Docker dentro do WSL (`redis`), publicando `0.0.0.0:6379` e com autenticação por ACL (usuário `Admin`). A porta está acessível a partir do Windows e responde `PONG` autenticado em `redis://Admin:***@127.0.0.1:6379`. A credencial fica apenas em `.env`; a integração efetiva (rate limit e refresh store) acontece na Fase 3.
