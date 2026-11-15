# Revisão de código

**Escopo:** mudanças locais em persistência PostgreSQL, Redis, histórico,
auditoria e empacotamento Docker.

**Estado:** todos os achados corrigidos e verificados. Cada correção tem teste de
regressão ou evidência de execução real.

## Achados e correções

### P1 — O encerramento pode apagar o schema da aplicação — **corrigido**

**Referências:** `crates/infrastructure/src/db.rs`, `crates/api/src/main.rs`

**O que era:** `Database::close()` fechava o pool **e** executava
`DROP SCHEMA ... CASCADE` no schema da instância. Como o `main` usa
`connect_isolated()` quando `DATABASE_SCHEMA` está definido e chama `close()` no
encerramento, um shutdown apagaria tabelas e dados de produção.

**Correção:** `close()` agora **só** fecha o pool. A remoção de schema ficou em
`drop_schema()`, que não tem nenhum caminho a partir do boot ou do shutdown — é
chamada apenas pelo código de teste. A doc de `close()` diz explicitamente que
nunca remove schema, para a regressão não voltar por descuido.

**Verificação:** a API rodou, recebeu SIGTERM, encerrou e subiu de novo; os dados
sobreviveram:

```text
INFO api: sinal de encerramento recebido signal="SIGTERM"
INFO api: servidor encerrado
...
usuários: 1 · produtos: 1 · movimentações: 2   (após o restart)
```

### P1 — O CI não provisiona PostgreSQL para os testes — **corrigido**

**Referências:** `.github/workflows/ci.yml`

**O que era:** o workflow rodava `cargo test --workspace` sem banco. Os helpers
entram em `panic` sem `TEST_DATABASE_URL`, então as suítes de integração nem
iniciavam. Os testes Redis saíam em `SKIP`.

**Correção:**

- `services.postgres` (17-alpine) e `services.redis` (7-alpine) com healthcheck;
- `TEST_DATABASE_URL` e `REDIS_URL` exportados no job;
- passo extra que **falha o build** se a saída dos testes Redis contiver `SKIP:`
  — transforma um verde enganoso em erro visível.

### P1 — Atualizações concorrentes divergem saldo e histórico — **corrigido**

**Referências:** `crates/application/src/history_ports.rs`,
`crates/infrastructure/src/history.rs`

**O que era:** o delta era calculado a partir de uma leitura feita **antes** da
transação. Duas requisições que liam o mesmo saldo gravavam valores absolutos
diferentes e registravam movimentos incompatíveis com o saldo final.

**Correção:** controle otimista no `UPDATE`:

```sql
UPDATE products SET ... WHERE id = $10 AND stock = $11 AND price_cents = $12
```

Os valores esperados são derivados do próprio `ProductChange` por
`expected_values()`: estoque esperado = novo − delta; preço esperado =
`old_cents`. Quando `rows_affected() == 0`, uma consulta distingue
`NotFound` (404) de `Conflict` (409) — no conflito o cliente relê e repete.

**Testes:** `stale_stock_update_is_rejected_and_writes_nothing` e
`concurrent_updates_keep_stock_equal_to_the_sum_of_movements` (três escritas
concorrentes sobre a mesma leitura: uma vence, e o saldo continua igual à soma
das movimentações).

### P2 — Mutação e auditoria não eram atômicas — **corrigido**

**Referências:** `crates/infrastructure/src/audit_tx.rs`,
`manage_catalog.rs`, `manage_users.rs`, `catalog_ports.rs`, `ports.rs`

**O que era:** o caso de uso gravava a mutação e só depois chamava `record()`.
Se a auditoria falhasse, a API devolvia erro com a alteração já confirmada — a
trilha ficava sem o evento.

**Correção:** as portas `insert`/`update` de usuário, categoria e produto passaram
a receber `Option<&AuditEntry>` e gravam a trilha **na mesma transação**. O
`insert_audit` foi extraído para `audit_tx.rs`, compartilhado pelos três
adaptadores.

**Testes:** `audit_is_written_in_the_same_transaction_as_the_product` e
`failing_product_write_rolls_back_the_audit_entry` (SKU duplicado não deixa
entrada de auditoria órfã).

### P2 — Credenciais interpoladas em URL sem escape — **corrigido**

**Referências:** `docker-compose.yml`, `crates/api/src/config.rs`,
`.env.example`

**O que era:** o `.env.example` sugeria `openssl rand -base64 32`. Base64 produz
`/`, `+` e `=`; numa URL, `/` encerra a autoridade e `+` é lido como espaço — uma
senha válida gerava URL inválida ou conexão para o host errado.

**Correção em duas camadas:**

1. `.env.example` e `docker-compose.yml` passam a recomendar `openssl rand -hex 32`
   (hexadecimal é seguro em URL);
2. o boot **rejeita** credencial com `/`, `+`, espaço, `#` ou `?` no trecho
   `user:senha@`, com mensagem que indica o comando correto. Falhar cedo vale mais
   que descobrir "connection refused" sem saber por quê.

**Testes:** `rejects_base64_slash_in_database_password`,
`rejects_base64_in_redis_password`, `accepts_hex_password_in_url`,
`accepts_url_without_credentials`.

### P2 — SIGTERM não acionava o shutdown gracioso — **corrigido**

**Referência:** `crates/api/src/main.rs`

**O que era:** o handler escutava apenas `ctrl_c()` (SIGINT) e a sessão de
desenvolvimento. O `docker stop` envia SIGTERM, então o processo morria sem
executar o drain e cortava requisições em voo — o `stop_grace_period` do compose
não tinha efeito.

**Correção:** `terminate_signal()` registra SIGTERM (via
`tokio::signal::unix::SignalKind::terminate`) e SIGINT, e o primeiro que chegar
vence. Em plataforma não-Unix, mantém apenas `ctrl_c`.

**Verificação real:**

```text
INFO api: servidor iniciado address=0.0.0.0:8080 max_concurrent=128 drain_secs=30
INFO api: sinal de encerramento recebido signal="SIGTERM"
INFO api: servidor encerrado
```

## Verificações executadas

```text
cargo fmt --all --check                                → ok
cargo clippy --workspace --all-targets -- -D warnings  → ok
RUSTDOCFLAGS=-Dwarnings cargo doc --workspace           → ok
scripts/check-local-docs.sh                            → ok
cargo test --workspace (PostgreSQL + Redis)            → 355 testes, 0 falhas
cargo deny check                                       → ok
```

Os testes que antes exigiam `TEST_DATABASE_URL` foram executados contra o
PostgreSQL real (`192.168.2.200:5432`), com schema descartável por teste. Os
testes Redis rodaram contra instância real, sem `SKIP`.
