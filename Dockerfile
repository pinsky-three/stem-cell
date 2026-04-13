# ── Stage 1: build ────────────────────────────────────────────────
FROM rust:bookworm AS builder

WORKDIR /app

# Install mise for locked Node version
RUN curl https://mise.run | sh
ENV PATH="/root/.local/share/mise/shims:/root/.local/bin:$PATH" \
    MISE_YES=1
COPY .mise.toml ./
RUN mise install

# Cache Rust deps: copy manifests first, build a dummy, then overlay real source
COPY Cargo.toml Cargo.lock ./
COPY crates/resource-model-macro/Cargo.toml crates/resource-model-macro/Cargo.toml
COPY crates/system-model-macro/Cargo.toml crates/system-model-macro/Cargo.toml
COPY crates/systems-codegen/Cargo.toml crates/systems-codegen/Cargo.toml
COPY crates/runtime/Cargo.toml crates/runtime/Cargo.toml

RUN mkdir -p crates/resource-model-macro/src crates/system-model-macro/src \
             crates/systems-codegen/src crates/runtime/src \
    && echo 'fn main(){}' > crates/runtime/src/main.rs \
    && echo 'fn main(){}' > crates/runtime/build.rs \
    && echo '' > crates/resource-model-macro/src/lib.rs \
    && echo '' > crates/system-model-macro/src/lib.rs \
    && echo 'fn main(){}' > crates/systems-codegen/src/main.rs \
    && cargo build --release 2>/dev/null || true \
    && rm -rf crates/*/src crates/runtime/build.rs

# Specs: the YAML source-of-truth needed by proc-macros at compile time
COPY specs/ specs/

# Frontend: install + build (node version locked by .mise.toml)
COPY frontend/ frontend/
RUN cd frontend && npm ci && npm run build

# Real source (SKIP_FRONTEND since we already built above)
COPY crates/ crates/
RUN touch crates/runtime/src/main.rs crates/runtime/src/lib.rs \
          crates/resource-model-macro/src/lib.rs \
          crates/system-model-macro/src/lib.rs \
    && SKIP_FRONTEND=1 cargo build --release -p stem-cell

# ── Stage 2: runtime ─────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       ca-certificates curl git \
       build-essential pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --no-log-init app

WORKDIR /app

COPY --from=builder /app/target/release/stem-cell ./server
COPY --from=builder /app/public/ ./public/

RUN mkdir -p /app/data && chown -R app:app /app /home/app
USER app

ENV SERVE_DIR=public PORT=4200 \
    RUST_LOG=stem_cell=info,tower_http=info
EXPOSE 4200

HEALTHCHECK --interval=10s --timeout=3s --start-period=5s \
    CMD curl -sf http://localhost:4200/healthz || exit 1

ENTRYPOINT ["/app/server"]
