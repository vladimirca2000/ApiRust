# Dockerfile multi-stage da API
#
# Dois estágios: um compila com a toolchain completa, o outro carrega apenas o
# binário. A imagem final não contém `rustc`, `cargo` nem o código-fonte — só o
# executável e as bibliotecas de sistema que ele precisa.

# ---------- Estágio de build ----------
FROM rust:1.85-bookworm AS builder

WORKDIR /build

# Copia primeiro os manifestos e o lock: enquanto eles não mudam, o Docker
# reaproveita a camada de dependências compiladas.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/domain/Cargo.toml crates/domain/Cargo.toml
COPY crates/application/Cargo.toml crates/application/Cargo.toml
COPY crates/infrastructure/Cargo.toml crates/infrastructure/Cargo.toml
COPY crates/api/Cargo.toml crates/api/Cargo.toml

# Stubs mínimos para o Cargo conseguir resolver o workspace antes do código.
RUN mkdir -p crates/domain/src crates/application/src \
             crates/infrastructure/src crates/api/src \
    && echo "" > crates/domain/src/lib.rs \
    && echo "" > crates/application/src/lib.rs \
    && echo "" > crates/infrastructure/src/lib.rs \
    && echo "fn main() {}" > crates/api/src/main.rs \
    && echo "" > crates/api/src/lib.rs \
    && cargo build --release -p api || true

# Agora o código real; só esta camada é recompilada quando o fonte muda.
COPY crates ./crates
COPY migrations ./migrations

# `touch` invalida o cache do crate `api` para que o binário final seja o real,
# e não o stub da camada anterior.
RUN touch crates/api/src/main.rs \
    && cargo build --release -p api

# ---------- Estágio de runtime ----------
FROM debian:bookworm-slim AS runtime

# `ca-certificates` para TLS de saída; `tini` reaproveita o PID 1 e repassa
# sinais, para o shutdown gracioso do Axum funcionar no container.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tini \
    && rm -rf /var/lib/apt/lists/*

# Usuário sem privilégios: se o processo for comprometido, não é root.
RUN useradd --system --create-home --uid 10001 apirust

WORKDIR /app

COPY --from=builder /build/target/release/api /app/api

# O banco fica em volume do PostgreSQL, não no container da API: recriar a
# imagem não pode apagar dados. `data/` existe apenas como diretório de trabalho.
RUN mkdir -p /app && chown -R apirust:apirust /app

USER apirust

ENV APP_ENV=production \
    APP_HOST=0.0.0.0 \
    APP_PORT=8080 \
    RUN_MIGRATIONS=true

EXPOSE 8080

# `tini` como entrypoint garante que SIGTERM chegue ao processo e o drain rode.
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/app/api"]
