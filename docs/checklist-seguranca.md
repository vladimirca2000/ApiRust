# Checklist de segurança — Fase 7

Revisão guiada pela skill `security-audit` em **modo orientação**: foco em
autenticação, IDOR, SQL injection e vazamento de segredos. Não é um pentest
completo nem um relatório de auditoria com artefatos; cada item aponta o
controle no código e o teste que o quebra se for removido.

Confiança: só entra `confirmado` o que tem teste ou inspeção de fonte. O que
depende de deploy (TLS no proxy, permissões de `data/`) fica como validação
externa.

## Autenticação

| Controle | Onde | Evidência |
|---|---|---|
| Algoritmo JWT fixo (`HS256`); `alg: none` rejeitado | `infrastructure/src/token.rs` | `crates/infrastructure/tests/token.rs` |
| Claims `sub`, `exp`, `iat`, `jti`, `iss`, `aud` validadas | idem | testes de token expirado/assinatura inválida |
| Access curto (padrão 15 min) | `JWT_ACCESS_TTL_SECS` | config rejeita TTL `0` |
| Refresh opaco, hash SHA-256, rotação, reuse revoga a família | `infrastructure/src/refresh.rs` | `crates/infrastructure/tests/auth.rs` |
| Rate limit **antes** do Argon2, por IP | `api/src/auth_routes.rs` | `crates/api/tests/auth_flow.rs` (429) |
| Login não enumera contas (mesmo 401) | `application/src/authenticate.rs` | teste de senha errada / e-mail inválido |
| `GET /users/me` relê o usuário; conta inativa → 403 | `auth_routes.rs` | `auth_flow.rs` |
| Documentação interativa só em desenvolvimento | `config.rs` + `router()` | `api_docs_environment.rs`; produção → 404 |

Residual consciente (Fase 8.1): o access token continua stateless até expirar.
Logout não o invalida. Rate limit é por processo.

## IDOR e autorização

| Controle | Onde | Evidência |
|---|---|---|
| Política única em `UserAction::authorize` | `application/src/policy.rs` | testes unitários da política |
| Ator relido do banco a cada request (`resolve_actor`) | `api/src/problem.rs` | RBAC: manager rebaixado perde acesso com token antigo |
| Usuário comum não lê/altera outro `{id}` | `user_routes.rs` | `crates/api/tests/rbac.rs` (403) |
| Usuário comum não escala papel | `ChangeUserRole` | 3 testes de escalonamento |
| Admin não rebaixa/exclui a si mesmo | `policy.rs` | teste dedicado |
| Catálogo: User lê, só Admin/Manager escreve | `manage_catalog.rs` | `catalog_flow.rs` (403) |

Remover a checagem de dono em `View`/`UpdateProfile` faz os testes de IDOR
falharem (`200` no lugar de `403`) — validado por mutação na Fase 4.

## SQL injection

| Controle | Onde | Evidência |
|---|---|---|
| Consultas com placeholders `?`; valores só em `.bind()` | repositórios SQLx | inspeção de fonte |
| `ORDER BY` vem do enum `ProductSort`, nunca do texto do cliente | `catalog_ports.rs` | `GET ?sort=price; DROP` → 400 |
| Curingas `%` e `_` da busca são escapados | `products.rs` `escape_like` | teste de catálogo |
| `format!(SELECT {COLUMNS} ...)` interpola **constante** de colunas, não input | `products.rs` | `COLUMNS` é `&'static str` |

Não há concatenação de input de usuário em SQL. A allowlist de `sort` é o
controle que impede injeção em `ORDER BY` (que não aceita parâmetro vinculado).

## Segredos e dados sensíveis

| Controle | Onde | Evidência |
|---|---|---|
| `.env` fora do Git; só `.env.example` versionado | `.gitignore` | `crates/api/tests/secrets.rs` |
| Exemplo sem senha/e-mail reais e sem Redis autenticado | `.env.example` | idem |
| `JwtSecret` e `AdminSeed` omitem valores em `Debug` | `config.rs` | testes de config |
| `PasswordHash` / `User` não imprimem o PHC | `domain` | testes de `Debug` |
| Respostas de usuário nunca incluem `password_hash` | DTOs | testes HTTP |
| Logs de boot não registram e-mail/senha/hash | `main.rs` `seed_admin` | inspeção + critério da Fase 2 |
| `JWT_SECRET` mínimo 32 caracteres; senha do Admin na política de domínio | `config.rs` | boot aborta |

Validação externa (não dá para fechar só com o repositório): TLS no reverse
proxy, `data/` gravável só pelo processo da API, e o advisory
`RUSTSEC-2026-0459` (SQLite WAL) mitigado por essa permissão de diretório.

## Risco aceito conscientemente

| Item | Risco | Mitigação atual | Condição para reavaliar |
|---|---|---|---|
| Senha do Redis (`admin`) em texto puro no `.env`, fraca e igual ao usuário | quem lê o arquivo ou adivinha a senha controla rate limit e denylist | instância de laboratório em rede interna; `.env` no `.gitignore`; credencial nunca logada | **qualquer** exposição fora da rede interna ou uso compartilhado do Redis |
| Banco PostgreSQL acessível por `userapp/userapp` | acesso direto ao schema, sem passar pela API | rede interna; schema dedicado | exposição do serviço ou dado real de cliente |

O risco está aceito por decisão explícita do responsável, registrado no próprio
`.env` junto da variável. A senha de serviço nunca deve ser a mesma do usuário:
para ambiente exposto, `openssl rand -base64 32` e `REDIS_PASSWORD` sem default
(o compose já exige a variável via `:?`).

## HTTP e operação

| Controle | Onde | Evidência |
|---|---|---|
| CORS sem `*`; origens explícitas | `hardening.rs` | `hardening_http.rs` |
| Corpo limitado a 1 MiB | idem | 413 no excesso |
| Timeout de request 30 s | idem | constante + boot |
| Teto de concorrência; excesso → 503 | semáforo no middleware HTTP | `excess_concurrent_requests_return_503` |
| Shutdown gracioso + drain com timeout | `main.rs` + `operation.rs` | testes de `force_after_drain` |
| `/ready` → 503 sem detalhe interno quando o banco cai | `routes.rs` | `http.rs` |
| Headers `nosniff`, `DENY`, `no-store` | `hardening.rs` | presentes em sucesso e erro |
| `cargo audit` e `cargo deny` no CI | `.github/workflows/ci.yml` | job `audit` + job `deny` |

## Itens adiados (não são falhas desta fase)

Registrados na [Fase 8](plano-de-implementacao.md#fase-8--dívidas-técnicas-e-itens-fora-do-escopo):
multi-tenancy. Os demais itens daquela fase — rate limit no Redis, denylist de
`jti`, auditoria, métricas Prometheus e Dockerfile — foram entregues.

## Achados da análise de QA

A revisão de qualidade em [`analise-qa.md`](analise-qa.md) registra 10 achados.
Os de prioridade média já estão fechados:

| Achado | Estado |
|---|---|
| A1 — senha default `admin` no compose | fechado: `${REDIS_PASSWORD:?}` obrigatório |
| A2 — rate limit chaveado no IP do socket | **aberto** (decisão de deploy: proxy vs. direto) |
| A3 — CI não executa testes de Redis | **fechado**: `services:` de PostgreSQL e Redis no workflow + passo que falha em `SKIP:` |
| A4 — logout sem Bearer sem teste dedicado | aberto (comportamento documentado) |

A revisão de código em [`revisao-codigo.md`](revisao-codigo.md) fechou seis
achados adicionais, incluindo dois que afetavam dados: o `close()` apagava o
schema da aplicação e a auditoria podia ficar separada da mutação.
