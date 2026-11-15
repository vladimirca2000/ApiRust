-- Histórico do catálogo e trilha de auditoria.
--
-- Estas tabelas existem porque `products.stock` e `products.price_cents` são
-- sobrescritos: sem elas, um saldo que caiu não diz se foi venda, perda ou
-- correção, e uma mudança de papel não é atribuível a ninguém.

CREATE TABLE stock_movements (
    id         TEXT PRIMARY KEY NOT NULL,
    product_id TEXT NOT NULL,
    delta      BIGINT NOT NULL CHECK (delta <> 0),
    reason     TEXT NOT NULL CHECK (reason IN ('initial', 'correction', 'sale', 'loss', 'restock')),
    actor_id   TEXT,
    at         TEXT NOT NULL,
    FOREIGN KEY (product_id) REFERENCES products (id) ON DELETE CASCADE,
    FOREIGN KEY (actor_id) REFERENCES users (id) ON DELETE SET NULL
);

CREATE INDEX stock_movements_product_at_idx ON stock_movements (product_id, at);

CREATE TABLE price_changes (
    id         TEXT PRIMARY KEY NOT NULL,
    product_id TEXT NOT NULL,
    old_cents  BIGINT NOT NULL CHECK (old_cents > 0),
    new_cents  BIGINT NOT NULL CHECK (new_cents > 0),
    actor_id   TEXT,
    at         TEXT NOT NULL,
    CHECK (old_cents <> new_cents),
    FOREIGN KEY (product_id) REFERENCES products (id) ON DELETE CASCADE,
    FOREIGN KEY (actor_id) REFERENCES users (id) ON DELETE SET NULL
);

CREATE INDEX price_changes_product_at_idx ON price_changes (product_id, at);

CREATE TABLE audit_log (
    id        TEXT PRIMARY KEY NOT NULL,
    actor_id  TEXT,
    action    TEXT NOT NULL CHECK (action IN ('create', 'update', 'change_role', 'delete')),
    entity    TEXT NOT NULL CHECK (entity IN ('user', 'category', 'product')),
    entity_id TEXT NOT NULL,
    before    TEXT,
    after     TEXT,
    at        TEXT NOT NULL,
    FOREIGN KEY (actor_id) REFERENCES users (id) ON DELETE SET NULL
);

CREATE INDEX audit_log_entity_idx ON audit_log (entity, entity_id, at);
CREATE INDEX audit_log_actor_idx ON audit_log (actor_id, at);
CREATE INDEX audit_log_at_idx ON audit_log (at);
