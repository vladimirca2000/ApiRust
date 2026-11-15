# ApiRust

API RESTful em Rust para gerenciamento de usuários, categorias e produtos, seguindo DDD, Clean Architecture e boas práticas de segurança, testes e observabilidade.

O plano completo de implementação está em [`docs/plano-de-implementacao.md`](docs/plano-de-implementacao.md).

## Stack

- **Runtime/HTTP:** Tokio + Axum
- **Persistência:** SQLx + PostgreSQL (migrations versionadas)
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
| `DATABASE_URL` | sim | — | URL do PostgreSQL (`postgres://usuario:senha@host:porta/banco`) |
| `RUN_MIGRATIONS` | não | `true` | Aplica as migrations embutidas no boot |
| `REDIS_URL` | produção: sim | — | Estado compartilhado de rate limit e denylist. Sem ele em produção, o boot aborta |
| `JWT_SECRET` | sim | — | Segredo com pelo menos 32 caracteres |
| `JWT_ACCESS_TTL_SECS` | não | `900` | TTL do access token |
| `ADMIN_EMAIL` | não | — | E-mail do Admin inicial (exige `ADMIN_PASSWORD`) |
| `ADMIN_PASSWORD` | não | — | Senha do Admin inicial (exige `ADMIN_EMAIL`) |
| `SHUTDOWN_ON_TAB_CLOSE` | não | `false` | Dev: encerra o servidor 10s após a última aba fechar |
| `MAX_CONCURRENT_REQUESTS` | não | `128` | Teto de requisições HTTP simultâneas; o excesso responde `503` |
| `SHUTDOWN_DRAIN_SECS` | não | `30` | Espera das conexões em voo após o sinal de shutdown |
| `METRICS_TOKEN` | não | — | Token de `/metrics`; sem ele a rota não é montada |
| `TRUSTED_PROXIES` | não | — | IPs de proxy/LB separados por vírgula. Vazio = API exposta direto (`X-Forwarded-For` ignorado) |
| `RUST_LOG` | não | `info,api=debug,tower_http=info` | Filtro de logs |

Nunca versione `.env` nem use o segredo de exemplo em produção.

### Banco e migrations

As migrations ficam em `migrations/` e são embutidas no binário. Com `RUN_MIGRATIONS=true` (padrão), são aplicadas de forma idempotente no boot. Em ambientes com passo de deploy controlado, defina `false` e aplique com `sqlx migrate run` (requer `sqlx-cli`).

O pool de PostgreSQL usa no máximo 10 conexões e `acquire_timeout` de 5 s. Unicidade de e-mail, SKU e nome de categoria vale apenas para registros ativos (índices parciais com `WHERE deleted_at IS NULL`), o que permite recriar um registro excluído. `DATABASE_SCHEMA` restringe a API a um schema próprio — útil quando o banco é compartilhado; sem ele vale o `search_path` do usuário.

O diretório `data/` é resíduo da época do SQLite: nenhum código o usa e ele está no `.gitignore`. Pode ser removido.

### Primeiro Admin

Defina `ADMIN_EMAIL` e `ADMIN_PASSWORD` juntos. No boot, a API cria um usuário `admin` ativo se o e-mail ainda não existir; se já existir, nada é alterado. As duas variáveis são validadas na inicialização (e-mail normalizado e política de senha) e podem ser removidas depois do primeiro provisionamento.

A senha nunca é armazenada em texto puro nem registrada em logs — apenas o hash Argon2id vai para o banco. Se apenas uma das variáveis estiver definida, o boot falha com erro de configuração.

## Execução

```powershell
cargo run -p api
```

- `GET /health` — processo ativo (não consulta dependências)
- `GET /ready` — dependências prontas (`SELECT 1` no PostgreSQL, timeout de 2 s). Banco
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

### IP do cliente e rate limit atrás de proxy

A chave do rate limit é o IP do cliente. Lendo só o peer do socket, uma API
atrás de reverse proxy vê **um único IP** — o do proxy — e todos os clientes
dividem a mesma cota: um cliente sozinho pode esgotar o limite de login (5/min).

**Ler `X-Forwarded-For` sempre seria pior**: o cabeçalho vem do cliente e
qualquer um forja um IP novo a cada requisição, escapando do limite.

A regra adotada:

| `TRUSTED_PROXIES` | Comportamento |
|---|---|
| vazio (padrão) | usa o peer do socket; `X-Forwarded-For` é **ignorado** |
| preenchido | se o peer está na lista, usa o **último** salto não confiável do cabeçalho; senão, usa o peer |

`TRUSTED_PROXIES` aceita IPs separados por vírgula (`10.0.0.2,10.0.0.3`); valor
inválido aborta o boot, porque um erro de digitação faria o proxy não ser
reconhecido — exatamente o problema que a variável resolve.

Escolha uma das linhas e registre no runbook:

- **API exposta direto ao cliente**: não defina `TRUSTED_PROXIES`.
- **Atrás de proxy/LB**: defina `TRUSTED_PROXIES` com os IPs do proxy.

### Estado distribuído (Redis)
Com mais de uma réplica, o estado **precisa** sair do processo:

| Item | Sem Redis | Com Redis |
|---|---|---|
| Rate limit | contador por réplica; o limite efetivo é `limite × réplicas` | contador único (`INCR` + `PEXPIRE` em script Lua) |
| Logout | access token continua válido até expirar | `jti` entra na denylist até o `exp` |

Em produção o boot **aborta** sem `REDIS_URL`: subir com o estado por instância
seria um controle de segurança que não controla nada. Em desenvolvimento, sem
Redis, tudo funciona em memória e o log avisa explicitamente.

Os testes de integração do Redis ficam em
`crates/infrastructure/tests/redis.rs` e exigem `REDIS_URL`; quando o servidor
não responde, eles **não passam em silêncio** — imprimem `SKIP` com o motivo.

### Histórico e auditoria

```text
GET /api/v1/products/{id}/stock-movements   Admin/Manager
GET /api/v1/products/{id}/price-changes     Admin/Manager
GET /api/v1/audit                           Admin
```

- `stock` e `price_cents` eram sobrescritos: um saldo que caía não dizia se foi
  venda, perda ou correção. Cada alteração agora grava uma linha em
  `stock_movements` (com `delta` e `reason`) ou `price_changes` (com valor
  anterior e novo).
- Produto e histórico são gravados **na mesma transação**. Se o histórico falhar,
  o produto não muda — um saldo sem explicação é pior que um erro.
- `audit_log` registra `actor_id`, ação, entidade e snapshots `before`/`after`.
  Nunca inclui `password_hash`.
- Mudança sem efeito (mesmo preço, mesmo estoque) **não** gera linha: histórico
  com ruído não serve para relatório.

### Métricas

Com `METRICS_TOKEN` definido, `GET /metrics` publica o formato texto do
Prometheus (`api_http_requests_total`, `api_http_request_duration_seconds`,
`api_http_in_flight`, `api_rate_limited_total`). O token vai no cabeçalho
`x-metrics-token`.

Sem `METRICS_TOKEN`, a rota **não existe** e responde `404` — métricas abertas
entregam o mapa de rotas e a taxa de erro para qualquer visitante. O rótulo
`route` é normalizado (`/api/v1/users/{id}`), para não criar uma série temporal
por usuário.

### PostgreSQL

O projeto usa PostgreSQL (testado com 17.12). Diferenças em relação ao SQLite que
moldaram o código:

| Tema | SQLite | PostgreSQL |
|---|---|---|
| Placeholder | `?` | `$1, $2…` |
| Inteiro de 64 bits | `INTEGER` já é 64 bits | `INTEGER` é `INT4`; usar `BIGINT` |
| Erro de unicidade | texto `UNIQUE constraint failed: users.email` | `SQLSTATE 23505` + nome da constraint |
| Isolamento de teste | `:memory:` por processo | schema próprio por teste |

Os três primeiros explicam mudanças reais no adaptador:

- **227 placeholders** foram convertidos. Em SQL dinâmico (`products`, `audit`) a
  numeração é calculada: os filtros ocupam `$1..$n` e `LIMIT`/`OFFSET` continuam
  a partir de `n+1`. Placeholder repetido ou fora de ordem é erro de execução no
  PostgreSQL, então a contagem precisa casar exatamente com os `bind`.
- **`BIGINT` em `price_cents`, `stock`, `delta`, `old_cents`, `new_cents`**: com
  `INTEGER` (`INT4`), a leitura de um `i64` falha com `mismatched types`. Isso
  aparecia como `500` na listagem de produtos.
- **Erros por `SQLSTATE` + nome da constraint** (`pg_errors.rs`): comparar o texto
  da mensagem seria frágil, porque o PostgreSQL traduz a mensagem conforme o
  locale do servidor.

**Schema dedicado.** O banco pode ser compartilhado com outras aplicações, então
`DATABASE_SCHEMA` limita a API a um schema próprio (`apirust_dev` no `.env`
local). Sem a variável, vale o `search_path` do usuário. O pool fixa o
`search_path` nas **opções de conexão** — um `SET` avulso valeria só para uma
conexão do pool.

**Testes.** Cada teste cria um schema `t_<nome>_<pid>_<nanos>_<seq>` e aplica as
migrations nele. A suíte roda em paralelo contra o mesmo servidor sem que um
teste veja as linhas do outro. A limpeza remove schemas cujo PID já morreu — a
varredura por PID evita apagar o schema de uma execução concorrente.

```bash
export TEST_DATABASE_URL="postgres://usuario:senha@host:5432/banco"
cargo test --workspace
```

Sem `TEST_DATABASE_URL` (ou `DATABASE_URL`), os testes de integração **falham com
mensagem explícita** em vez de passar em silêncio.

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
- [x] Fase 1 — banco, migrations, constraints, pool e readiness (SQLite na época; migrado para PostgreSQL na Fase 8)
- [x] Fase 2 — domínio de usuários, hashing Argon2id e primeiro Admin
- [x] Fase 3 — login, JWT, refresh, logout
- [x] Fase 4 — RBAC e usuários
- [x] Fase 5 — categorias e produtos
- [x] Fase 6 — OpenAPI e erros padronizados
- [x] Fase 7 — operação e endurecimento
- [x] Fase 8 — dívidas técnicas: Redis distribuído, auditoria, histórico e métricas

## Licença

MIT — veja [`LICENSE`](LICENSE).
