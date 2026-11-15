-- Schema inicial da API: usuários, categorias, produtos e refresh tokens.
-- Datas em UTC, formato ISO-8601 (TEXT). Soft delete via deleted_at.
-- Valores monetários e saldo usam BIGINT (INT8): o domínio trabalha com
-- `i64` e `INTEGER` no PostgreSQL é `INT4`, o que faria a decodificação
-- falhar com "mismatched types" na leitura.

CREATE TABLE users (
    id            TEXT PRIMARY KEY NOT NULL,
    name          TEXT NOT NULL CHECK (length(trim(name)) > 0),
    email         TEXT NOT NULL CHECK (length(trim(email)) > 0),
    password_hash TEXT NOT NULL CHECK (length(password_hash) > 0),
    role          TEXT NOT NULL CHECK (role IN ('admin', 'manager', 'user')),
    status        TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'inactive')),
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    deleted_at    TEXT
);

-- Unicidade apenas entre registros ativos: permite recriar um e-mail após soft delete.
CREATE UNIQUE INDEX users_email_active_unique
    ON users (email)
    WHERE deleted_at IS NULL;

CREATE TABLE categories (
    id          TEXT PRIMARY KEY NOT NULL,
    name        TEXT NOT NULL CHECK (length(trim(name)) > 0),
    description TEXT,
    status      TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'inactive')),
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    deleted_at  TEXT
);

CREATE UNIQUE INDEX categories_name_active_unique
    ON categories (name)
    WHERE deleted_at IS NULL;

CREATE TABLE products (
    id          TEXT PRIMARY KEY NOT NULL,
    name        TEXT NOT NULL CHECK (length(trim(name)) > 0),
    sku         TEXT NOT NULL CHECK (length(trim(sku)) > 0),
    description TEXT,
    price_cents BIGINT NOT NULL CHECK (price_cents > 0),
    stock       BIGINT NOT NULL CHECK (stock >= 0),
    category_id TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'inactive')),
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    deleted_at  TEXT,
    FOREIGN KEY (category_id) REFERENCES categories (id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX products_sku_active_unique
    ON products (sku)
    WHERE deleted_at IS NULL;

CREATE INDEX products_category_id_idx ON products (category_id);
CREATE INDEX products_name_idx ON products (name);

CREATE TABLE refresh_tokens (
    id             TEXT PRIMARY KEY NOT NULL,
    user_id        TEXT NOT NULL,
    token_hash     TEXT NOT NULL UNIQUE,
    expires_at     TEXT NOT NULL,
    created_at     TEXT NOT NULL,
    revoked_at     TEXT,
    replaced_by_id TEXT,
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    FOREIGN KEY (replaced_by_id) REFERENCES refresh_tokens (id) ON DELETE SET NULL
);

CREATE INDEX refresh_tokens_user_id_idx ON refresh_tokens (user_id);
CREATE INDEX refresh_tokens_expires_at_idx ON refresh_tokens (expires_at);
