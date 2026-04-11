# Stem Cell

A spec-driven application server. Define your data model in a single YAML file and get a Postgres-backed REST API with an admin UI — no boilerplate.

Stem Cell compiles a [resource-model-macro](crates/resource-model-macro/) YAML spec into:

- **Rust structs** (entity, create, update) with serde + sqlx derives
- **SQL migrations** (CREATE TABLE with foreign keys, run on startup)
- **CRUD repositories** backed by sqlx
- **REST API** (Axum + OpenAPI via utoipa) with Scalar docs at `/api/docs`
- **Admin dashboard** (Astro + Tailwind) with CRUD pages generated from the same spec

Edit `crates/runtime/specs/self.yaml`, rebuild, and everything updates.

## Architecture

```
stem-cell/
├── crates/
│   ├── resource-model-macro/   # proc-macro: YAML → Rust codegen (publishable crate)
│   └── runtime/                # binary: Axum server + build.rs (frontend codegen)
│       ├── build.rs            # reads spec → generates Astro pages → builds frontend
│       ├── specs/self.yaml     # the single source of truth
│       └── src/main.rs         # ~45 LOC: connect DB, migrate, serve API + static files
├── frontend/                   # Astro 6 + Tailwind 4 (pages are @generated)
├── Dockerfile                  # multi-stage: rust:bookworm → debian:bookworm-slim
└── .mise.toml                  # tool versions + task runner
```

### How it works

1. `build.rs` reads `specs/self.yaml` and generates Astro pages into `frontend/src/pages/`
2. `build.rs` runs `npm run build` to compile the frontend into `public/`
3. The proc-macro reads the same spec and expands into structs, repos, migrations, and API routes
4. At startup, the server applies migrations, mounts the API under `/api/*`, serves OpenAPI docs at `/api/docs`, and serves the static frontend as a fallback

## Prerequisites

- [mise](https://mise.jdx.dev/) (installs Rust 1.94+ and Node 22 automatically)
- PostgreSQL (or a Neon / Supabase connection string)

## Quick start

```bash
# 1. Clone and enter
git clone <repo-url> stem-cell && cd stem-cell

# 2. Install toolchain (Rust + Node, versions locked in .mise.toml)
mise install

# 3. Configure environment
cp .env.example .env
# Edit .env — set DATABASE_URL to your Postgres connection string

# 4. Install frontend deps
mise run frontend:install

# 5. Run (builds frontend + starts server on :4200)
mise run dev
```

Then open:

| URL | Description |
|---|---|
| `http://localhost:4200` | Admin dashboard |
| `http://localhost:4200/api/docs` | Scalar API explorer |
| `http://localhost:4200/api/users` | Example JSON endpoint |

## Environment variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `DATABASE_URL` | yes | — | Postgres connection string |
| `PORT` | no | `4200` | HTTP listen port |
| `SERVE_DIR` | no | `public` | Static file directory |
| `RUST_LOG` | no | `stem_cell=info,tower_http=info` | Log filter |
| `SKIP_FRONTEND` | no | — | Set to skip frontend build in `build.rs` (used in Docker) |

## Tasks (mise)

```bash
mise run dev              # cargo run (builds frontend + server)
mise run build            # release build
mise run check            # type-check only (skips frontend)
mise run frontend:dev     # Astro dev server with HMR
mise run frontend:install # npm install
mise run docker           # docker build -t stem-cell .
```

## Docker

```bash
# Build
docker build -t stem-cell .

# Run
docker run --rm -p 4200:4200 \
  -e DATABASE_URL="postgresql://..." \
  stem-cell
```

The image is a two-stage build (~100 MB final) using `debian:bookworm-slim`. It runs as a non-root `app` user with a healthcheck on `/`.

## Defining your model

Edit `crates/runtime/specs/self.yaml`:

```yaml
version: 1
config:
  visibility: "pub"
  backend: "postgres"
  api: true

entities:
  - name: "User"
    table: "users"
    id: { name: "id", type: "uuid" }
    fields:
      - { name: "name",  type: "string", required: true }
      - { name: "email", type: "string", required: true, unique: true }

relations: []
```

Supported types: `uuid`, `string`, `text`, `int`, `bigint`, `float`, `bool`.
Supported relations: `has_many`, `belongs_to`.

See the [resource-model-macro README](crates/resource-model-macro/README.md) for the full spec format.

## Project layout

| Path | What it does |
|---|---|
| `crates/resource-model-macro/` | Proc-macro crate (YAML → Rust codegen). Independently publishable to crates.io. |
| `crates/runtime/` | The `stem-cell` binary. `build.rs` generates frontend pages; `main.rs` wires the server. |
| `crates/runtime/specs/self.yaml` | Single source of truth for the data model. |
| `frontend/` | Astro 6 + Tailwind 4. Pages under `src/pages/` are `@generated` — don't edit them by hand. |
| `public/` | Build output from Astro (gitignored). Served as static files. |

## License

MIT
