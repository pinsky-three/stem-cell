use crate::client::OpenCodeClient;
use crate::error::{Error, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};
use tokio::sync::RwLock;

const HEALTH_TIMEOUT: Duration = Duration::from_secs(30);

/// Environment variables forwarded from the parent process to spawned
/// OpenCode servers. Covers major AI provider credentials and OpenCode
/// configuration knobs that the user may set in their shell or .env.
const FORWARDED_ENV_VARS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GOOGLE_API_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_REGION",
    "AZURE_OPENAI_API_KEY",
    "AZURE_OPENAI_ENDPOINT",
    "OPENROUTER_API_KEY",
    "GROQ_API_KEY",
    "MISTRAL_API_KEY",
    "XAI_API_KEY",
    "DEEPSEEK_API_KEY",
    // Ollama (local) — no API key required, but we still forward a
    // placeholder if present for users fronting Ollama behind an auth proxy.
    "OLLAMA_BASE_URL",
    "OLLAMA_MODELS",
    "OLLAMA_API_KEY",
    "OPENCODE_MODEL",
    "OPENCODE_PROVIDER",
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_CONTENT",
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
];

/// Known provider env vars → (provider_id, env_var_name).
/// Used to auto-generate `OPENCODE_CONFIG_CONTENT` when the user sets
/// a provider key but hasn't provided explicit inline config.
const PROVIDER_KEY_MAP: &[(&str, &str)] = &[
    ("openrouter", "OPENROUTER_API_KEY"),
    ("anthropic", "ANTHROPIC_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
    ("google", "GOOGLE_API_KEY"),
    ("groq", "GROQ_API_KEY"),
    ("mistral", "MISTRAL_API_KEY"),
    ("xai", "XAI_API_KEY"),
    ("deepseek", "DEEPSEEK_API_KEY"),
];

/// Default base URL for a local Ollama server (OpenAI-compatible endpoint).
const OLLAMA_DEFAULT_BASE_URL: &str = "http://localhost:11434/v1";

/// Builds the Ollama provider stanza (OpenAI-compatible shape) when
/// `OLLAMA_MODELS` is set. `OLLAMA_MODELS` is a comma-separated list of
/// Ollama model tags (e.g. `llama3.2,qwen2.5-coder:7b`). Returns `None`
/// when the user hasn't opted in.
///
/// We use `@ai-sdk/openai-compatible` because it's already bundled with
/// OpenCode and works against Ollama's `/v1` shim without any extra
/// npm install at runtime.
fn build_ollama_provider() -> Option<String> {
    let raw_models = std::env::var("OLLAMA_MODELS").ok()?;
    let models: Vec<&str> = raw_models
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if models.is_empty() {
        return None;
    }

    let base_url =
        std::env::var("OLLAMA_BASE_URL").unwrap_or_else(|_| OLLAMA_DEFAULT_BASE_URL.to_string());

    let models_json = models
        .iter()
        .map(|m| format!(r#""{m}": {{}}"#))
        .collect::<Vec<_>>()
        .join(", ");

    // Ollama typically needs no auth, but we pass a dummy `apiKey` because
    // some openai-compatible clients reject requests without one.
    Some(format!(
        r#""ollama": {{ "npm": "@ai-sdk/openai-compatible", "name": "Ollama (local)", "options": {{ "baseURL": "{base_url}", "apiKey": "ollama" }}, "models": {{ {models_json} }} }}"#,
    ))
}

/// Builds an inline JSON config for OpenCode from environment variables.
/// Returns `None` if no provider API keys (or local Ollama models) are
/// detected.
fn build_inline_config(pm_config: &ProcessManagerConfig) -> Option<String> {
    let mut providers = Vec::new();

    for &(provider_id, env_key) in PROVIDER_KEY_MAP {
        if std::env::var(env_key).is_ok() {
            // Use {env:VAR} substitution so the actual key stays in the env,
            // not baked into the JSON string.
            providers.push(format!(
                r#""{provider_id}": {{ "options": {{ "apiKey": "{{env:{env_key}}}" }} }}"#,
            ));
        }
    }

    if let Some(ollama) = build_ollama_provider() {
        providers.push(ollama);
    }

    if providers.is_empty() {
        return None;
    }

    let model = pm_config
        .default_model
        .clone()
        .or_else(|| std::env::var("OPENCODE_MODEL").ok());

    let model_line = model
        .as_deref()
        .map(|m| format!(r#", "model": "{m}""#))
        .unwrap_or_default();

    Some(format!(
        r#"{{ "$schema": "https://opencode.ai/config.json", "provider": {{ {} }}{} }}"#,
        providers.join(", "),
        model_line,
    ))
}

/// Resolved path to the `opencode` binary (cached at first spawn).
static OPENCODE_BIN: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Resolves the opencode binary.
/// 1. `mise where opencode` (tries PATH then `~/.local/bin/mise`).
/// 2. Falls back to bare `opencode` on PATH.
fn resolve_opencode_bin() -> &'static str {
    OPENCODE_BIN.get_or_init(|| {
        let mise_candidates: Vec<std::path::PathBuf> = {
            let mut v: Vec<std::path::PathBuf> = vec!["mise".into()];
            if let Some(home) = std::env::var_os("HOME") {
                v.push(std::path::PathBuf::from(home).join(".local/bin/mise"));
            }
            v
        };

        for mise_bin in &mise_candidates {
            if let Ok(output) = std::process::Command::new(mise_bin)
                .args(["where", "opencode"])
                .output()
            {
                if output.status.success() {
                    let dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    for suffix in ["opencode", "bin/opencode"] {
                        let bin = format!("{dir}/{suffix}");
                        if std::path::Path::new(&bin).exists() {
                            tracing::info!(path = %bin, mise = %mise_bin.display(), "resolved opencode binary via mise");
                            return bin;
                        }
                    }
                }
            }
        }

        tracing::warn!("could not resolve opencode via mise, falling back to PATH");
        "opencode".to_string()
    })
}

/// Per-project OpenCode server instance.
struct Instance {
    port: u16,
    process: Child,
    last_activity: Instant,
    client: OpenCodeClient,
}

impl Instance {
    fn touch(&mut self) {
        self.last_activity = Instant::now();
    }
}

/// Configuration for the process manager.
#[derive(Debug, Clone)]
pub struct ProcessManagerConfig {
    pub port_base: u16,
    pub port_range: u16,
    pub idle_timeout: Duration,
    pub server_password: Option<String>,
    pub default_model: Option<String>,
}

impl Default for ProcessManagerConfig {
    fn default() -> Self {
        Self {
            port_base: 14000,
            port_range: 200,
            idle_timeout: Duration::from_secs(600),
            server_password: None,
            default_model: None,
        }
    }
}

impl ProcessManagerConfig {
    pub fn from_env() -> Self {
        Self {
            port_base: std::env::var("OPENCODE_PORT_BASE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(14000),
            port_range: std::env::var("OPENCODE_PORT_RANGE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(200),
            idle_timeout: Duration::from_secs(
                std::env::var("OPENCODE_IDLE_TIMEOUT_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(600),
            ),
            server_password: std::env::var("OPENCODE_SERVER_PASSWORD").ok(),
            default_model: std::env::var("OPENCODE_MODEL").ok(),
        }
    }
}

/// Manages one OpenCode server process per project.
///
/// Spawn on first use, reap after idle timeout.
#[derive(Clone)]
pub struct ProcessManager {
    instances: Arc<RwLock<HashMap<uuid::Uuid, Instance>>>,
    config: Arc<ProcessManagerConfig>,
}

impl ProcessManager {
    pub fn new(config: ProcessManagerConfig) -> Self {
        Self {
            instances: Arc::new(RwLock::new(HashMap::new())),
            config: Arc::new(config),
        }
    }

    pub fn config(&self) -> &ProcessManagerConfig {
        &self.config
    }

    /// Returns a client to the OpenCode server for the given project.
    /// Spawns a new server if one isn't already running.
    pub async fn get_or_spawn(
        &self,
        project_id: uuid::Uuid,
        work_dir: &Path,
    ) -> Result<OpenCodeClient> {
        // Fast path: already running
        {
            let mut instances = self.instances.write().await;
            if let Some(inst) = instances.get_mut(&project_id) {
                // Check the process is still alive
                if inst.process.try_wait().ok().flatten().is_none() {
                    inst.touch();
                    return Ok(inst.client.clone());
                }
                // Process died — remove and re-spawn below
                tracing::warn!(%project_id, "OpenCode process exited unexpectedly, respawning");
                instances.remove(&project_id);
            }
        }

        // Slow path: spawn a new server
        let port = self.allocate_port().await?;
        tracing::info!(%project_id, port, dir = %work_dir.display(), "spawning OpenCode server");

        let opencode_bin = resolve_opencode_bin();
        let mut cmd = Command::new(opencode_bin);
        cmd.args(["serve", "--port", &port.to_string()])
            .current_dir(work_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        if let Some(ref pw) = self.config.server_password {
            cmd.env("OPENCODE_SERVER_PASSWORD", pw);
        }

        for key in FORWARDED_ENV_VARS {
            if let Ok(val) = std::env::var(key) {
                cmd.env(key, val);
            }
        }

        // Auto-generate OPENCODE_CONFIG_CONTENT when a provider API key is
        // present but the user hasn't set the config content explicitly.
        if std::env::var("OPENCODE_CONFIG_CONTENT").is_err() {
            if let Some(config_json) = build_inline_config(&self.config) {
                tracing::info!("injecting auto-generated OPENCODE_CONFIG_CONTENT");
                cmd.env("OPENCODE_CONFIG_CONTENT", config_json);
            }
        }

        let child = cmd
            .spawn()
            .map_err(|e| Error::SpawnFailed(format!("{opencode_bin} serve: {e}")))?;

        let client = OpenCodeClient::new(port, self.config.server_password.as_deref())?;

        // Wait for the server to become healthy
        client.wait_healthy(HEALTH_TIMEOUT).await?;

        let instance = Instance {
            port,
            process: child,
            last_activity: Instant::now(),
            client: client.clone(),
        };

        self.instances.write().await.insert(project_id, instance);
        tracing::info!(%project_id, port, "OpenCode server ready");

        Ok(client)
    }

    /// Kills servers that have been idle longer than `config.idle_timeout`.
    pub async fn reap_idle(&self) {
        let timeout = self.config.idle_timeout;
        let mut instances = self.instances.write().await;
        let before = instances.len();

        instances.retain(|project_id, inst| {
            if inst.last_activity.elapsed() > timeout {
                tracing::info!(%project_id, port = inst.port, "reaping idle OpenCode server");
                // kill_on_drop will SIGKILL, but let's try graceful first
                let _ = inst.process.start_kill();
                false
            } else {
                true
            }
        });

        let reaped = before - instances.len();
        if reaped > 0 {
            tracing::info!(reaped, remaining = instances.len(), "idle reap complete");
        }
    }

    /// Gracefully shuts down all managed OpenCode servers.
    pub async fn shutdown_all(&self) {
        let mut instances = self.instances.write().await;
        for (project_id, inst) in instances.iter_mut() {
            tracing::info!(%project_id, port = inst.port, "shutting down OpenCode server");
            let _ = inst.process.start_kill();
        }
        instances.clear();
    }

    /// Starts a background task that periodically reaps idle servers.
    pub fn spawn_reaper(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let mgr = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                mgr.reap_idle().await;
            }
        })
    }

    async fn allocate_port(&self) -> Result<u16> {
        let instances = self.instances.read().await;
        let used: std::collections::HashSet<u16> = instances.values().map(|i| i.port).collect();

        for offset in 0..self.config.port_range {
            let candidate = self.config.port_base + offset;
            if !used.contains(&candidate) && port_available(candidate) {
                return Ok(candidate);
            }
        }

        Err(Error::PortExhausted {
            base: self.config.port_base,
            range: self.config.port_range,
        })
    }
}

fn port_available(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `build_ollama_provider` reads process-wide env vars; gate with a mutex
    /// so parallel tests don't stomp on each other.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_env<F: FnOnce()>(vars: &[(&str, Option<&str>)], f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, _)| ((*k).to_string(), std::env::var(*k).ok()))
            .collect();
        // SAFETY: the ENV_LOCK mutex serializes access to process-wide env
        // vars; no other threads in this test binary touch these keys.
        unsafe {
            for (k, v) in vars {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
        f();
        // SAFETY: same as above.
        unsafe {
            for (k, v) in prev {
                match v {
                    Some(val) => std::env::set_var(&k, val),
                    None => std::env::remove_var(&k),
                }
            }
        }
    }

    #[test]
    fn ollama_disabled_without_models() {
        with_env(
            &[("OLLAMA_MODELS", None), ("OLLAMA_BASE_URL", None)],
            || {
                assert!(build_ollama_provider().is_none());
            },
        );
    }

    #[test]
    fn ollama_stanza_has_openai_compatible_shape() {
        with_env(
            &[
                ("OLLAMA_MODELS", Some("llama3.2, qwen2.5-coder:7b ,")),
                ("OLLAMA_BASE_URL", None),
            ],
            || {
                let stanza = build_ollama_provider().expect("provider");
                assert!(stanza.contains(r#""npm": "@ai-sdk/openai-compatible""#));
                assert!(stanza.contains(r#""baseURL": "http://localhost:11434/v1""#));
                assert!(stanza.contains(r#""llama3.2": {}"#));
                assert!(stanza.contains(r#""qwen2.5-coder:7b": {}"#));
            },
        );
    }

    #[test]
    fn ollama_respects_custom_base_url() {
        with_env(
            &[
                ("OLLAMA_MODELS", Some("llama3.2")),
                ("OLLAMA_BASE_URL", Some("http://ollama.local:11434/v1")),
            ],
            || {
                let stanza = build_ollama_provider().expect("provider");
                assert!(stanza.contains(r#""baseURL": "http://ollama.local:11434/v1""#));
            },
        );
    }

    #[test]
    fn inline_config_emits_ollama_only() {
        let cleared: Vec<(&str, Option<&str>)> = PROVIDER_KEY_MAP
            .iter()
            .map(|(_, k)| (*k, None))
            .chain([
                ("OLLAMA_MODELS", Some("llama3.2")),
                ("OLLAMA_BASE_URL", None),
                ("OPENCODE_MODEL", Some("ollama/llama3.2")),
            ])
            .collect();
        with_env(&cleared, || {
            let cfg = ProcessManagerConfig {
                default_model: Some("ollama/llama3.2".into()),
                ..ProcessManagerConfig::default()
            };
            let json = build_inline_config(&cfg).expect("inline config");
            assert!(json.contains(r#""ollama""#));
            assert!(json.contains(r#""model": "ollama/llama3.2""#));
            let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
            assert!(parsed["provider"]["ollama"]["models"]["llama3.2"].is_object());
        });
    }
}
