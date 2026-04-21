//! OpenAI-compatible inference server backed by OxiBonsai.
//!
//! Exposes:
//!   POST /v1/chat/completions   (streaming + non-streaming)
//!   POST /v1/completions        (legacy)
//!   POST /v1/embeddings
//!   GET  /v1/models
//!   GET  /health
//!   GET  /metrics               (Prometheus text exposition)
//!
//! Configuration is 12-Factor — all knobs are environment variables so the
//! same binary ships to dev and prod unchanged. See `.env.example`.
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use oxibonsai_runtime::engine::InferenceEngine;
use oxibonsai_runtime::presets::SamplingPreset;
use oxibonsai_runtime::server::{create_router_with_metrics, serve_with_shutdown, shutdown_signal};
use oxibonsai_runtime::tokenizer_bridge::TokenizerBridge;
use oxibonsai_runtime::tracing_setup::{init_tracing, TracingConfig};
use oxibonsai_runtime::InferenceMetrics;
use std::sync::Arc;

const MODEL_URL: &str =
    "https://huggingface.co/prism-ml/Bonsai-8B-gguf/resolve/main/Bonsai-8B.gguf";
const TOKENIZER_URL: &str =
    "https://huggingface.co/Qwen/Qwen3-8B/resolve/main/tokenizer.json";

struct ServerSettings {
    model_path: String,
    tokenizer_path: Option<String>,
    bind_addr: SocketAddr,
    max_seq_len: usize,
    preset: SamplingPreset,
    seed: u64,
    log_level: String,
    json_logs: bool,
}

impl ServerSettings {
    fn from_env() -> Result<Self> {
        let model_path = std::env::var("OXIBONSAI_MODEL_PATH")
            .unwrap_or_else(|_| "../../models/Bonsai-8B.gguf".to_string());

        let tokenizer_path = std::env::var("OXIBONSAI_TOKENIZER_PATH").ok();

        let bind_addr = std::env::var("OXIBONSAI_BIND_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
            .parse::<SocketAddr>()
            .context("OXIBONSAI_BIND_ADDR must be a valid socket address (e.g. 127.0.0.1:8080)")?;

        let max_seq_len = std::env::var("OXIBONSAI_MAX_SEQ_LEN")
            .ok()
            .map(|v| v.parse::<usize>())
            .transpose()
            .context("OXIBONSAI_MAX_SEQ_LEN must be a positive integer")?
            .unwrap_or(4096);

        let preset = match std::env::var("OXIBONSAI_PRESET")
            .unwrap_or_else(|_| "balanced".to_string())
            .to_ascii_lowercase()
            .as_str()
        {
            "greedy" => SamplingPreset::Greedy,
            "balanced" => SamplingPreset::Balanced,
            "creative" => SamplingPreset::Creative,
            "precise" => SamplingPreset::Precise,
            "conversational" => SamplingPreset::Conversational,
            other => anyhow::bail!(
                "unknown OXIBONSAI_PRESET={other:?}; expected greedy|balanced|creative|precise|conversational"
            ),
        };

        let seed = std::env::var("OXIBONSAI_SEED")
            .ok()
            .map(|v| v.parse::<u64>())
            .transpose()
            .context("OXIBONSAI_SEED must be a non-negative integer")?
            .unwrap_or(42);

        let log_level = std::env::var("OXIBONSAI_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
        let json_logs = std::env::var("OXIBONSAI_JSON_LOGS")
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);

        Ok(Self {
            model_path,
            tokenizer_path,
            bind_addr,
            max_seq_len,
            preset,
            seed,
            log_level,
            json_logs,
        })
    }

    /// Derive the models directory from the model path.
    fn models_dir(&self) -> &Path {
        Path::new(&self.model_path)
            .parent()
            .unwrap_or(Path::new("."))
    }

    /// Resolve the tokenizer path: explicit env var > auto-detected file in models dir.
    fn resolved_tokenizer_path(&self) -> Option<PathBuf> {
        if let Some(ref p) = self.tokenizer_path {
            return Some(PathBuf::from(p));
        }
        let candidate = self.models_dir().join("tokenizer.json");
        candidate.exists().then_some(candidate)
    }
}

// ── Model prelude (idempotent downloads) ─────────────────────────────────

async fn ensure_models(settings: &ServerSettings) -> Result<()> {
    let models_dir = settings.models_dir();
    tokio::fs::create_dir_all(models_dir)
        .await
        .with_context(|| format!("failed to create models dir: {}", models_dir.display()))?;

    let model_path = Path::new(&settings.model_path);
    if !model_path.exists() {
        eprintln!();
        eprintln!("  Model not found at {}", model_path.display());
        eprintln!("  Downloading Bonsai-8B (~1.1 GB) …");
        eprintln!();
        download(MODEL_URL, model_path).await.with_context(|| {
            format!("failed to download model to {}", model_path.display())
        })?;
    }

    let tokenizer_path = models_dir.join("tokenizer.json");
    if !tokenizer_path.exists() {
        eprintln!();
        eprintln!("  Tokenizer not found at {}", tokenizer_path.display());
        eprintln!("  Downloading Qwen3 tokenizer (~11 MB) …");
        eprintln!();
        download(TOKENIZER_URL, &tokenizer_path).await.with_context(|| {
            format!(
                "failed to download tokenizer to {}",
                tokenizer_path.display()
            )
        })?;
    }

    Ok(())
}

/// Download `url` to `dest` atomically via a `.part` temp file.
/// Uses `curl` — no extra Rust deps, handles redirects and progress natively.
async fn download(url: &str, dest: &Path) -> Result<()> {
    let part = dest.with_extension("part");

    let status = tokio::process::Command::new("curl")
        .args(["-fSL", "--progress-bar", "-o"])
        .arg(&part)
        .arg(url)
        .status()
        .await
        .context("failed to spawn curl — is it installed?")?;

    anyhow::ensure!(status.success(), "curl exited with {status}");

    tokio::fs::rename(&part, dest).await.with_context(|| {
        format!(
            "failed to rename {} → {}",
            part.display(),
            dest.display()
        )
    })?;

    Ok(())
}

// ── Main ─────────────────────────────────────────────────────────────────

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let settings = ServerSettings::from_env()?;

    // Download model + tokenizer if missing (idempotent, runs before tracing
    // is initialised so progress output goes to raw stderr).
    ensure_models(&settings).await?;

    let tracing_cfg = TracingConfig {
        log_level: settings.log_level.clone(),
        json_output: settings.json_logs,
        ..Default::default()
    };
    init_tracing(&tracing_cfg).map_err(|e| anyhow::anyhow!("init_tracing failed: {e}"))?;

    let tokenizer_path = settings.resolved_tokenizer_path();

    tracing::info!(
        model = %settings.model_path,
        tokenizer = ?tokenizer_path,
        bind = %settings.bind_addr,
        max_seq_len = settings.max_seq_len,
        preset = settings.preset.name(),
        seed = settings.seed,
        "booting fully-local inference server"
    );

    // ── Load engine ──────────────────────────────────────────────────────
    let sampling_params = settings.preset.params();
    let model_path = settings.model_path.clone();
    let max_seq_len = settings.max_seq_len;
    let seed = settings.seed;

    let engine = tokio::task::spawn_blocking(move || {
        InferenceEngine::from_gguf_path(&model_path, sampling_params, seed, max_seq_len)
    })
    .await
    .context("engine loader task panicked")?
    .with_context(|| format!("failed to load GGUF from {}", settings.model_path))?;

    tracing::info!("engine ready");

    // ── Tokenizer (auto-resolved from models dir if env var is unset) ────
    let tokenizer = match tokenizer_path.as_deref() {
        Some(path) => {
            let path_str = path.to_str().context("tokenizer path is not valid UTF-8")?;
            Some(
                TokenizerBridge::from_file(path_str)
                    .with_context(|| format!("failed to load tokenizer from {}", path.display()))?,
            )
        }
        None => {
            tracing::warn!(
                "no tokenizer found; /v1/chat/completions will echo raw token IDs"
            );
            None
        }
    };

    // ── Router + server ──────────────────────────────────────────────────
    let metrics = Arc::new(InferenceMetrics::new());
    let router = create_router_with_metrics(engine, tokenizer, metrics);

    tracing::info!(addr = %settings.bind_addr, "listening");
    serve_with_shutdown(router, settings.bind_addr, shutdown_signal())
        .await
        .map_err(|e| anyhow::anyhow!("server error: {e}"))?;

    Ok(())
}
