# Análise de QA — ApiRust

Revisão de qualidade em **modo orientação** (não é um pentest nem auditoria
completa): varredura de codebase + verificação direta dos achados na fonte +
execução dos portões de qualidade. Cada item traz o path do código, a
evidência e um teste que quebra se o comportamento mudar. Complementa
[`checklist-seguranca.md`](checklist-seguranca.md) — aqui o foco é robustez,
consistência, cobertura de teste e riscos operacionais.

Confiança: só entra como `confirmado` o que foi verificado por leitura direta
ou execução local.

> **Histórico:** 1ª rodada sobre o estado commitado (`f22ac5f`, stack SQLite).
> 2ª rodada executou os portões sobre a migração SQLite → PostgreSQL em curso.
> **3ª rodada (15/11/2026) fechou todos os achados abertos.** O que era aberto
> agora tem correção, teste de regressão e evidência de execução.

---

## Resumo executivo

| Dimensão | Estado |
|---|---|
| Estrutura | 4 crates (~15.000 LOC de `src`, ~7.000 LOC de testes de integração), DDD/Clean |
| Persistência | PostgreSQL via SQLx 0.8 (feature `postgres`) |
| Testes | **383 testes, 0 falhas** contra PostgreSQL e Redis reais |
| Erros | Zero `.unwrap()` em `src`; `expect()` só em mutex poisoning e segredo de teste; `unsafe_code = "forbid"` |
| CI | 5 jobs: qualidade (com PostgreSQL e Redis), cobertura informativa, auditoria, licenças, docs |
| Achados abertos | **Nenhum.** A6 e A10 eram decisões registradas, não defeitos |

O código está maduro: sem `TODO/FIXME/panic!` em produção, SQL 100%
parametrizado, config validada no boot, fail-closed em rate limit e denylist, e
escritas de catálogo com controle otimista e auditoria transacional.

---

## O que está sólido (verificado)

| Controle | Onde | Evidência |
|---|---|---|
| Zero `unwrap`/`todo!`/`println!` em produção | workspace | varredura `src` limpa; logging 100% `tracing` |
| URL de banco validada no boot: `sqlite://` rejeitada | `crates/api/src/config.rs` | `bootstrap.rs` |
| Credencial em URL validada: `/`, `+`, espaço abortam o boot | `crates/api/src/config.rs` | 4 testes de config |
| Catálogo canônico de rotas com teste guardião | `catalog.rs` + `routes_catalog.rs` | teste falha se o roteador divergir |
| Config tipada validada no boot | `crates/api/src/config.rs` (~26 testes) | boot aborta em config inválida |
| Segredos redigidos em `Debug` | `JwtSecret`, `AdminSeed` | testes `debug_does_not_leak_*` |
| Fail-closed: denylist fora do ar ⇒ 401 | `crates/api/src/auth.rs` | inspeção de fonte |
| Ator relido do banco a cada request (anti token-stale) | `problem.rs` (`resolve_actor`) | `rbac.rs` |
| Isolamento por schema em teste, com limpeza de órfãos por PID | `tests/common/mod.rs` | suíte paralela sem contaminação |
| Escrita de catálogo com controle otimista (CAS) | `infrastructure/src/history.rs` | 2 testes de concorrência |
| Auditoria na mesma transação da mutação | `infrastructure/src/audit_tx.rs` | 2 testes de rollback |
| Compose: senha de Postgres **e** Redis obrigatórias (`:?`) | `docker-compose.yml` | inspeção |
| CI provê PostgreSQL e Redis reais | `.github/workflows/ci.yml` | inspeção + execução local equivalente |
| Docker: usuário não-root, `tini`, drain 40s > 30s | `Dockerfile`, `docker-compose.yml` | inspeção |
| SIGTERM aciona shutdown gracioso | `crates/api/src/main.rs` | log real: `signal="SIGTERM"` |
| `close()` nunca apaga schema | `infrastructure/src/db.rs` | dados sobrevivem a restart |

---

## Achados e resolução

### A1 — Senha default `admin` no Redis do compose — ✅ RESOLVIDO

**Correção:** `${POSTGRES_PASSWORD:?}` e `${REDIS_PASSWORD:?}` no compose — o
`up` aborta sem as variáveis. A documentação passou a recomendar
`openssl rand -hex 32` (base64 gera `/` e `+`, inválidos em URL).

### A2 — Rate limit chaveado no IP do socket — ✅ RESOLVIDO

**O problema:** lendo só o peer do TCP, uma API atrás de proxy vê um único IP —
o do proxy. Todos os clientes dividem a mesma cota, e um cliente esgota o limite
de login (5/min).

**A armadilha:** ler `X-Forwarded-For` sempre é pior. O cabeçalho vem do
cliente, que forja um IP novo a cada requisição e escapa do limite para sempre.

**Correção:** `crates/api/src/client_ip.rs` com a regra:

- `TRUSTED_PROXIES` vazio (padrão): usa o peer e **ignora** o cabeçalho;
- peer na lista de confiáveis: usa o **último salto não confiável** da cadeia —
  os anteriores podem ter sido escritos pelo cliente;
- cadeia ilegível: volta ao peer, em vez de aceitar valor não interpretável.

Config inválida em `TRUSTED_PROXIES` aborta o boot: um erro de digitação faria o
proxy não ser reconhecido, reproduzindo o problema original.

**Evidência no servidor real** — seis requisições de login com `X-Forwarded-For`
forjado e diferente em cada uma:

```text
tentativa 1 (XFF forjado): 401
tentativa 2 (XFF forjado): 401
tentativa 3 (XFF forjado): 401
tentativa 4 (XFF forjado): 401
tentativa 5 (XFF forjado): 429   ← limite atingido apesar do cabeçalho
tentativa 6 (XFF forjado): 429
```

**Testes:** `qa_findings.rs` (3 casos) + `bootstrap.rs` (2 casos).

### A3 — CI nunca executa os testes de Redis — ✅ RESOLVIDO

**Correção:** `services` de `postgres:17-alpine` e `redis:7-alpine` no job de
qualidade, com `TEST_DATABASE_URL` e `REDIS_URL`. Passo adicional **falha o
build** se a saída dos testes Redis contiver `SKIP:` — o verde enganoso vira
erro visível.

### A4 — Logout sem `Authorization` — ✅ RESOLVIDO (teste + registro)

**Correção:** teste dedicado `logout_without_bearer_revokes_the_presented_refresh_token`.
O comportamento permanece (revogar o refresh apresentado é idempotente e
documentado); a decisão está registrada na seção de riscos assumidos do
`checklist-seguranca.md`.

### A5 — Sem cobertura e sem teste do bootstrap — ✅ RESOLVIDO

**Correção (duas partes):**

1. **Job `coverage`** no CI com `cargo-llvm-cov --no-fail-under-coverage` e
   resumo publicado em `$GITHUB_STEP_SUMMARY`. Sem limiar que quebre o build:
   piso numérico mal calibrado produz teste de fachada.
2. **`crates/api/tests/bootstrap.rs`** com 11 testes: produção exige Redis;
   produção rejeita Swagger e `SHUTDOWN_ON_TAB_CLOSE`; `sqlite://` recusado;
   `TRUSTED_PROXIES` inválido aborta; migrations e seed rodam **antes** de o
   servidor aceitar conexões; SIGTERM tratado; modo Redis logado.

### A6 — Paginação corrigia input inválido — ✅ RESOLVIDO (mudança de contrato)

**Era decisão de time; o time optou por recusar.**

`normalize` agora devolve `Result`: `limit` fora de `1..=100` é
`AppError::InvalidPagination` → **400** com `detail` apontando o intervalo.
`offset` negativo continua virando 0 (não há ambiguidade). O teto permanece.

Um cliente com bug recebia uma página de 1 item e concluía "não há mais dados";
agora recebe a mensagem que aponta o parâmetro.

**Evidência no servidor real:**

```text
GET /api/v1/products?limit=0    → 400 "limit deve estar entre 1 e 100, recebido 0"
GET /api/v1/products?limit=1000 → 400
GET /api/v1/products?limit=20   → 200
```

**Testes:** `pagination.rs` (5), `qa_findings.rs` (3), `catalog_flow.rs` (1).

### A7 — Banco de desenvolvimento em `data/` — ✅ RESOLVIDO

O código não usa mais SQLite. O resíduo `data/api.db` foi removido do working
dir, e o README registra que `data/` é descartável e está no `.gitignore`.

### A8 — `AppState::new` com segredo de teste e docs expostas — ✅ RESOLVIDO

**Correção:** o construtor neutro foi **removido** e dividido em dois:

| Construtor | Segredo | `expose_api_docs` |
|---|---|---|
| `AppState::from_parts` (produção) | inerte, substituído por `with_jwt` | `false` |
| `AppState::for_tests` | descartável | `true` |

O nome grita o contrato. `main.rs` usa `from_parts`, e o teste
`production_bootstrap_does_not_use_the_test_constructor` falha se alguém trocar
— inclusive se reintroduzir `AppState::new`.

### A9 — Advisories ignoradas exigem reavaliação — ✅ RESOLVIDO

Cada entrada mantida em `deny.toml`/`.cargo/audit.toml` tem justificativa e
condição de reavaliação. A entrada obsoleta caiu em A11.

### A10 — `#[allow(dead_code)]` nas funções `_doc()` — ✅ DOCUMENTADO

Padrão intencional do utoipa; a documentação fica no próprio módulo
`api_docs.rs`. A regressão de uma rota sem `_doc` é pega por
`routes_catalog.rs`.

### A11 — Entrada obsoleta no `deny.toml` — ✅ RESOLVIDO

`RUSTSEC-2026-0459` (libsqlite3-sys) removida de `deny.toml` e de
`.cargo/audit.toml`: com a feature `postgres`, a crate saiu do grafo.

**Evidência:** `cargo deny check` → `advisories ok, bans ok, licenses ok,
sources ok`, **sem warnings**.

### A12 — Suíte depende de PostgreSQL externo — ✅ RESOLVIDO

**Correção:** os helpers separam as duas causas de falha:

- variável ausente → mensagem cita `TEST_DATABASE_URL` e `DATABASE_URL`;
- servidor inacessível → `ensure_server_reachable` falha citando host e porta e
  o erro de conexão.

Antes, com a variável definida e o servidor fora, o teste sugeria definir a
variável — direcionando o diagnóstico para o lugar errado.

---

## Gaps de cobertura de teste — situação

| Gap | Estado |
|---|---|
| Bootstrap/`main.rs` | ✅ `crates/api/tests/bootstrap.rs` (11 testes) |
| Logout sem Bearer | ✅ `qa_findings.rs` |
| Comportamento atrás de proxy | ✅ 3 testes de `client_ip` + 2 de config |
| Cobertura quantitativa | ✅ job `coverage` (informativo) |
| `page.rs` (SSE) | ainda enxuto; o timer de 10 s não tem teste de timeout — registrado, impacto baixo |

---

## Qualidade do CI

Cinco jobs independentes com warnings como erro:

| Job | Cobre |
|---|---|
| `quality` | fmt, clippy, testes (PostgreSQL + Redis reais), prova de ausência de `SKIP`, docs |
| `coverage` | `cargo-llvm-cov`, resumo no step summary, sem bloquear |
| `audit` | `cargo audit` |
| `deny` | licenças, advisories, fontes, duplicidade |

Nada de benchmark/performance — aceitável para o tamanho do projeto. O semáforo
de concorrência (`503`) e o `TimeoutLayer` de 30 s continuam sem teste de carga
formal.

---

## Validação executada (3ª rodada)

**Data:** 15/11/2026, PostgreSQL e Redis reais.

| Portão | Resultado |
|---|---|
| `cargo fmt --all --check` | ✅ OK |
| `cargo clippy --workspace --all-targets -- -D warnings` | ✅ zero warnings |
| `cargo test --workspace` | ✅ **383 testes, 0 falhas** |
| `RUSTDOCFLAGS=-D warnings cargo doc --workspace` | ✅ OK |
| `cargo deny check` | ✅ todos ok, sem warnings |
| `scripts/check-local-docs.sh` | ✅ OK |

Smoke no servidor real (`0.0.0.0:8080`, PostgreSQL + Redis):

```text
GET  /health                        → 200
GET  /api/v1/products?limit=0       → 400 (A6)
GET  /api/v1/products?limit=1000    → 400 (A6)
GET  /api/v1/products?limit=20      → 200
POST /api/v1/auth/login ×6 com XFF forjado
                                    → 401×4, 429, 429 (A2: cabeçalho não burla)
log: "rate limit usa o IP do socket: TRUSTED_PROXIES não configurado"
log: "Redis conectado: rate limit e revogação distribuídos ativos"
```

**Plano de ação:** concluído. Não resta item aberto; `page.rs` (SSE) fica como
melhoria futura, sem impacto em segurança ou correção.
