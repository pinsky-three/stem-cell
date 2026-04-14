# Agent Instructions — Stem Cell

## Scope of Changes

You are working inside a **spec-driven** code-generation project. Almost all
Rust code, Astro pages, SQL migrations, and API routes are derived from two
YAML files. Your editable surface is intentionally small.

### Files you MAY edit

| Path | Purpose |
|---|---|
| `specs/self.yaml` | Data model: entities, fields, relations |
| `specs/systems.yaml` | Business workflows & integration contracts |
| `frontend/src/pages/index.astro` | Landing page (hand-authored, not generated) |
| `crates/runtime/src/systems/*.rs` | Implement generated trait stubs (business logic) |

### OpenCode builds and preview lifecycle

Stem Cell runs OpenCode against the project checkout while a **separate** host-managed
process runs `mise run dev` for the live preview. Template repos (and their own
`AGENTS.md`) should state that **agents must not start dev servers or assume
localhost preview URLs** — the host owns that lifecycle.

The server sends a default OpenCode **system** prompt with the same constraints.
Override with `STEM_CELL_OPENCODE_SYSTEM_PROMPT` (non-empty replaces the default;
whitespace-only disables the system message).

### Workflow after spec changes

1. Edit the relevant spec file (`specs/self.yaml` or `specs/systems.yaml`).
2. Run codegen to materialise stubs and update generated code:
   ```bash
   cargo run -p systems-codegen
   ```
3. Implement any new `// @generated-stub` files under `crates/runtime/src/systems/`.
   Once you remove the `@generated-stub` marker the codegen will no longer overwrite that file.
4. Verify with:
   ```bash
   mise run check   # type-check (no frontend build)
   mise run test    # full test suite
   ```

### Files you must NOT edit without approval

Everything outside the table above is **generated or framework code**:

- `crates/resource-model-macro/` — proc-macro (YAML → Rust codegen)
- `crates/system-model-macro/` — proc-macro (systems YAML → traits/DTOs)
- `crates/systems-codegen/` — CLI that generates stubs from specs
- `crates/runtime/src/main.rs` and `build.rs` — server wiring
- `frontend/src/pages/admin/**` — generated Astro pages (overwritten on build)
- `frontend/src/layouts/`, `frontend/src/components/` — shared UI scaffolding
- `public/` — build output (gitignored)
- `Dockerfile`, `Cargo.toml`, `Cargo.lock` — infrastructure

**If the user's request requires changes to any of these files, STOP and
explain what deeper changes are needed and why before proceeding.** Let the
user decide whether to expand scope.

### Quick reference

| Task | Command |
|---|---|
| Run codegen | `cargo run -p systems-codegen` |
| Dev server (backend + frontend) | `mise run dev` |
| Type-check only | `mise run check` |
| Clippy | `mise run lint` |
| Tests | `mise run test` |
| Contract tests only | `mise run test:contracts` |
| Full CI pipeline | `mise run ci` |

### Spec format cheat-sheet

**Entity fields** (`specs/self.yaml`): types are `uuid`, `string`, `text`,
`int`, `bigint`, `float`, `bool`. Add `required`, `unique`, and `references`
as needed.

**System steps** (`specs/systems.yaml`): step kinds are `load_one`,
`load_many`, `create`, `update`, `delete`, `guard`, `branch`,
`call_integration`, `emit_event`. Systems with `mode: "contract"` generate
only a trait + DTOs — you implement the body in
`crates/runtime/src/systems/<snake_name>.rs`.
