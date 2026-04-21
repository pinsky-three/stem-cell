# fully-local

OpenAI-compatible inference server backed by [OxiBonsai](https://github.com/cool-japan/oxibonsai) — Pure Rust, on-prem, Metal GPU on Apple Silicon.

Drop-in target for OpenAI SDKs: `base_url = "http://127.0.0.1:8080/v1"`.

## Endpoints

| Method | Path                      | Notes                                                  |
| ------ | ------------------------- | ------------------------------------------------------ |
| POST   | `/v1/chat/completions`    | Streaming (SSE) and non-streaming                      |
| POST   | `/v1/completions`         | Legacy (non-chat)                                      |
| POST   | `/v1/embeddings`          |                                                        |
| GET    | `/v1/models`              | Returns `bonsai-8b`                                    |
| GET    | `/health`                 | Liveness probe (returns `ok`)                          |
| GET    | `/metrics`                | Prometheus text exposition (latency, tokens, errors…)  |

## Quick start

```bash
# 1. Put the GGUF file where the default expects it
#    (or set OXIBONSAI_MODEL_PATH absolutely).
ls ../../models/Bonsai-8B.gguf

# 2. Run (needs nightly — see ./rust-toolchain.toml for why).
cargo run --release

# 3. Hit it.
curl -s http://127.0.0.1:8080/health
curl -s http://127.0.0.1:8080/v1/models | jq
curl -s -X POST http://127.0.0.1:8080/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"messages":[{"role":"user","content":"hi"}],"max_tokens":32}'
curl -s http://127.0.0.1:8080/metrics | head -n 20
```

From the workspace root the same thing works as:

```bash
cargo +nightly run -p fully-local --release
```

## Configuration (12-Factor, all env-driven)

See [`.env.example`](./.env.example) for the authoritative list.

| Var                        | Default                                 | Notes                                                               |
| -------------------------- | --------------------------------------- | ------------------------------------------------------------------- |
| `OXIBONSAI_MODEL_PATH`     | `../../models/Bonsai-8B.gguf`           | Resolved from CWD. Use absolute for installed binaries.             |
| `OXIBONSAI_TOKENIZER_PATH` | _(unset)_                               | HF `tokenizer.json`. Without it, responses echo raw token IDs.      |
| `OXIBONSAI_BIND_ADDR`      | `127.0.0.1:8080`                        | Use `0.0.0.0:8080` to expose on LAN.                                |
| `OXIBONSAI_MAX_SEQ_LEN`    | `4096`                                  | Context window. Bonsai trained up to 65536.                          |
| `OXIBONSAI_PRESET`         | `balanced`                              | `greedy` \| `balanced` \| `creative` \| `precise` \| `conversational` |
| `OXIBONSAI_SEED`           | `42`                                    | RNG seed shared across requests.                                    |
| `OXIBONSAI_LOG_LEVEL`      | `info`                                  | Any `tracing-subscriber` env-filter spec.                           |
| `OXIBONSAI_JSON_LOGS`      | `false`                                 | JSON-structured logs for log aggregators.                           |

## Tokenizer

The server runs without a tokenizer, but `/v1/chat/completions.content` will be the raw token ID array (`"[13, 8, 320, ...]"`) — useful only for plumbing smoke tests.

For real chat, point `OXIBONSAI_TOKENIZER_PATH` at a HuggingFace `tokenizer.json` for Qwen3. If you downloaded Ternary Bonsai via OxiBonsai's `scripts/download_ternary.sh`, you'll already have `models/tokenizer.json`. Otherwise grab the Qwen3 tokenizer from HuggingFace.

## Architecture

- **Engine**: `oxibonsai_runtime::InferenceEngine::from_gguf_path` — memory-maps the GGUF, loads weights onto the GPU (Metal), pre-builds the GPU weight cache, and warms the fused full-forward path.
- **Server**: `oxibonsai_runtime::server::create_router_with_metrics` — Axum router with CORS/tracing middleware.
- **Acceleration**: `metal` feature (GPU, fused Q1 full-forward) + `simd-neon` CPU fallback. Runtime auto-detects via `KernelDispatcher`.
- **Observability**: structured `tracing` logs, Prometheus `/metrics`, per-request latency histograms.

## Why nightly?

`oxibonsai-kernels 0.1.2` uses `#![feature(stdarch_aarch64_prefetch)]` (tracking issue [rust-lang/rust#117217](https://github.com/rust-lang/rust/issues/117217), no FCP). On stable Rust + aarch64 the build fails immediately. Scoped to this crate only via `./rust-toolchain.toml`; the rest of the workspace keeps using stable.

## Troubleshooting

| Symptom                                           | Fix                                                                                  |
| ------------------------------------------------- | ------------------------------------------------------------------------------------ |
| `Address already in use (os error 48)`            | `lsof -nP -iTCP:8080 -sTCP:LISTEN -t \| xargs kill -9` then retry.                   |
| `GGUF file not found`                             | Set `OXIBONSAI_MODEL_PATH` to an absolute path, or `cd crates/fully-local` first.     |
| Responses are `"[13, 8, ...]"` instead of text    | Set `OXIBONSAI_TOKENIZER_PATH` to a `tokenizer.json`.                                |
| `E0554: #![feature] may not be used on stable`    | You bypassed the local toolchain override. Run via `cargo +nightly run -p fully-local`. |

## License

Apache-2.0, same as OxiBonsai.
