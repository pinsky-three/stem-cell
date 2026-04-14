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
    "OPENCODE_MODEL",
    "OPENCODE_PROVIDER",
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
];

/// Resolved path to the `opencode` binary (cached at first spawn).
static OPENCODE_BIN: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Resolves the opencode binary. Tries `mise where opencode` first,
/// then falls back to bare `opencode` on PATH.
fn resolve_opencode_bin() -> &'static str {
    OPENCODE_BIN.get_or_init(|| {
        if let Ok(output) = std::process::Command::new("mise")
            .args(["where", "opencode"])
            .output()
        {
            if output.status.success() {
                let dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let bin = format!("{dir}/opencode");
                if std::path::Path::new(&bin).exists() {
                    tracing::info!(path = %bin, "resolved opencode binary via mise");
                    return bin;
                }
                let bin_in_bin = format!("{dir}/bin/opencode");
                if std::path::Path::new(&bin_in_bin).exists() {
                    tracing::info!(path = %bin_in_bin, "resolved opencode binary via mise");
                    return bin_in_bin;
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
