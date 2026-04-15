use crate::system_api::*;
use sqlx::Row;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

const DEFAULT_REPO_URL: &str = "https://github.com/pinsky-three/stem-cell-shrank";
const CONTAINER_MEMORY_LIMIT: &str = "2g";

/// Max time allowed for the synchronous handler work (DB inserts).
const HANDLER_TIMEOUT: Duration = Duration::from_secs(30);

/// Max time the background container is allowed to run before being killed.
const CONTAINER_TIMEOUT: Duration = Duration::from_secs(600);

/// How long to wait for the child server to become healthy before giving up.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(300);

/// Interval between health-check polls on the child server.
const HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(3);

/// How often we flush accumulated log lines to the database.
const LOG_FLUSH_INTERVAL: Duration = Duration::from_secs(2);

/// Max log size stored per job (prevents unbounded growth).
const MAX_LOG_BYTES: usize = 512 * 1024;

/// How many times we try `mise run dev` + health before giving up (with OpenCode repair between tries).
fn dev_start_max_attempts() -> u32 {
    std::env::var("STEM_CELL_DEV_START_ATTEMPTS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| (1..=10).contains(&n))
        .unwrap_or(3)
}

const DEV_SERVER_REPAIR_PROMPT: &str = "\
The local preview runs `mise run dev` and must serve GET /healthz on the port from .mise.toml. \
A previous attempt failed to become healthy in time. \
Inspect the repo: fix broken dependencies, config, missing env, port binding, or startup errors so \
`mise install` and `mise run dev` succeed. Prefer minimal changes; keep the app runnable.";

#[async_trait::async_trait]
impl SpawnEnvironmentSystem for super::AppSystems {
    async fn execute(
        &self,
        pool: &sqlx::PgPool,
        input: SpawnEnvironmentInput,
    ) -> Result<SpawnEnvironmentOutput, SpawnEnvironmentError> {
        super::cleanup_deployments::ensure_periodic_cleanup(pool.clone());

        let span = tracing::info_span!(
            "spawn_environment",
            org_id = %input.org_id,
            user_id = %input.user_id,
        );
        let _enter = span.enter();

        match tokio::time::timeout(HANDLER_TIMEOUT, create_records(pool, &input)).await {
            Ok(inner) => inner,
            Err(_) => {
                tracing::error!("handler timed out waiting for database");
                Err(SpawnEnvironmentError::DatabaseError(
                    "request timed out — database may be overloaded".into(),
                ))
            }
        }
    }
}

/// Derive a deterministic port from the job UUID (range 10000–59999).
fn port_for_job(job_id: uuid::Uuid) -> u16 {
    10_000 + (job_id.as_u128() % 50_000) as u16
}

async fn create_records(
    pool: &sqlx::PgPool,
    input: &SpawnEnvironmentInput,
) -> Result<SpawnEnvironmentOutput, SpawnEnvironmentError> {
    sqlx::query(
        "INSERT INTO organizations (id, name, slug, avatar_url, active, created_at, updated_at) \
         VALUES ($1, 'Anonymous', 'anonymous', NULL, true, NOW(), NOW()) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(input.org_id)
    .execute(pool)
    .await
    .map_err(|e| SpawnEnvironmentError::DatabaseError(e.to_string()))?;

    sqlx::query(
        "INSERT INTO users (id, name, email, avatar_url, auth_provider, active, created_at, updated_at) \
         VALUES ($1, 'Anonymous', $2, NULL, 'anonymous', true, NOW(), NOW()) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(input.user_id)
    .bind(format!("anon-{}@stem-cell.local", input.user_id))
    .execute(pool)
    .await
    .map_err(|e| SpawnEnvironmentError::DatabaseError(e.to_string()))?;

    let project_id = uuid::Uuid::new_v4();
    let conversation_id = uuid::Uuid::new_v4();
    let message_id = uuid::Uuid::new_v4();
    let job_id = uuid::Uuid::new_v4();

    let slug = format!("project-{}", project_id.as_simple());

    sqlx::query(
        "INSERT INTO projects \
             (id, name, slug, description, status, framework, visibility, active, \
              org_id, creator_id, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, 'active', NULL, 'private', true, $5, $6, NOW(), NOW())",
    )
    .bind(project_id)
    .bind(&input.prompt)
    .bind(&slug)
    .bind(&input.prompt)
    .bind(input.org_id)
    .bind(input.user_id)
    .execute(pool)
    .await
    .map_err(|e| SpawnEnvironmentError::DatabaseError(e.to_string()))?;

    sqlx::query(
        "INSERT INTO conversations \
             (id, title, active, project_id, created_at, updated_at) \
         VALUES ($1, 'Initial conversation', true, $2, NOW(), NOW())",
    )
    .bind(conversation_id)
    .bind(project_id)
    .execute(pool)
    .await
    .map_err(|e| SpawnEnvironmentError::DatabaseError(e.to_string()))?;

    sqlx::query(
        "INSERT INTO messages \
             (id, role, content, sort_order, has_attachment, \
              conversation_id, author_id, created_at, updated_at) \
         VALUES ($1, 'user', $2, 0, false, $3, $4, NOW(), NOW())",
    )
    .bind(message_id)
    .bind(&input.prompt)
    .bind(conversation_id)
    .bind(input.user_id)
    .execute(pool)
    .await
    .map_err(|e| SpawnEnvironmentError::DatabaseError(e.to_string()))?;

    sqlx::query(
        "INSERT INTO build_jobs \
             (id, status, prompt_summary, model, tokens_used, error_message, \
              duration_ms, logs, deployment_id, project_id, message_id, created_at, updated_at) \
         VALUES ($1, 'running', $2, 'container', 0, '', 0, '', NULL, $3, $4, NOW(), NOW())",
    )
    .bind(job_id)
    .bind(&input.prompt)
    .bind(project_id)
    .bind(message_id)
    .execute(pool)
    .await
    .map_err(|e| SpawnEnvironmentError::DatabaseError(e.to_string()))?;

    tracing::info!(%project_id, %job_id, "project and job created, spawning environment");

    let bg_pool = pool.clone();
    let prompt_for_opencode = input.prompt.clone();
    tokio::spawn(async move {
        let started = std::time::Instant::now();

        let result = match tokio::time::timeout(
            CONTAINER_TIMEOUT,
            run_environment(DEFAULT_REPO_URL, job_id, project_id, message_id, &bg_pool),
        )
        .await
        {
            Ok(inner) => inner,
            Err(_) => Err(format!(
                "environment killed after {}s timeout",
                CONTAINER_TIMEOUT.as_secs()
            )),
        };

        let duration_ms = started.elapsed().as_millis() as i64;

        let (status, error_message) = match result {
            Ok(()) => ("succeeded", String::new()),
            Err(e) => {
                tracing::error!(%job_id, error = %e, "environment failed");
                ("failed", e)
            }
        };

        if let Err(db_err) = sqlx::query(
            "UPDATE build_jobs \
             SET status = $2, error_message = $3, duration_ms = $4, updated_at = NOW() \
             WHERE id = $1",
        )
        .bind(job_id)
        .bind(status)
        .bind(&error_message)
        .bind(duration_ms)
        .execute(&bg_pool)
        .await
        {
            tracing::error!(%job_id, error = %db_err, "failed to update build_job status");
        }

        tracing::info!(%job_id, %status, duration_ms, "environment task finished");

        // ── Trigger OpenCode transformation ──────────────────────
        // After the template is deployed, create a new build job and run
        // OpenCode to transform the code based on the user's prompt.
        if status == "succeeded" {
            trigger_opencode_build(
                &bg_pool,
                project_id,
                message_id,
                &prompt_for_opencode,
            )
            .await;
        }
    });

    Ok(SpawnEnvironmentOutput {
        project_id: project_id.to_string(),
        job_id: job_id.to_string(),
        status: "running".to_string(),
    })
}

// ── Execution dispatch ─────────────────────────────────────────────────

async fn run_environment(
    repo_url: &str,
    job_id: uuid::Uuid,
    project_id: uuid::Uuid,
    message_id: uuid::Uuid,
    pool: &sqlx::PgPool,
) -> Result<(), String> {
    let mode = std::env::var("SPAWN_MODE").unwrap_or_default();
    if mode == "subprocess" {
        run_subprocess(repo_url, job_id, project_id, message_id, pool).await
    } else {
        run_in_container(repo_url, job_id, project_id, message_id, pool).await
    }
}

/// Clone + toolchain install only (idempotent if checkout already exists).
async fn run_subprocess_setup(
    repo_url: &str,
    job_id: uuid::Uuid,
    work_dir: &str,
    port: u16,
) -> Result<(), String> {
    let script = format!(
        "set -e && \
         if [ -d \"{dir}/.git\" ]; then echo 'repo already cloned'; else git clone {repo} \"{dir}\"; fi && \
         cd \"{dir}\" && \
         MISE=$( command -v mise || echo ~/.local/bin/mise ) && \
         if [ ! -x \"$MISE\" ]; then \
           curl -fsSL https://mise.run | bash && MISE=~/.local/bin/mise; \
         fi && \
         $MISE trust && \
         sed 's/^PORT = .*/PORT = \"{port}\"/' .mise.toml > .mise.toml.tmp && mv .mise.toml.tmp .mise.toml && \
         if command -v flock >/dev/null 2>&1; then flock /tmp/mise-install.lock $MISE install --yes; else $MISE install --yes; fi",
        repo = repo_url,
        dir = work_dir,
        port = port,
    );

    tracing::info!(%job_id, %repo_url, "subprocess: clone and toolchain install");
    let status = tokio::process::Command::new("bash")
        .args(["-c", &script])
        .env("MISE_YES", "1")
        .status()
        .await
        .map_err(|e| format!("setup script: {e}"))?;
    if !status.success() {
        return Err(format!("clone/toolchain setup failed: {status}"));
    }
    Ok(())
}

/// Subprocess mode: clone + install once, then retry `mise run dev` with OpenCode repair between failures.
async fn run_subprocess(
    repo_url: &str,
    job_id: uuid::Uuid,
    project_id: uuid::Uuid,
    message_id: uuid::Uuid,
    pool: &sqlx::PgPool,
) -> Result<(), String> {
    let port = port_for_job(job_id);
    let work_dir = format!("/tmp/stem-cell-{job_id}");

    run_subprocess_setup(repo_url, job_id, &work_dir, port).await?;

    let dev_script = format!(
        "set -e && cd \"{dir}\" && \
         MISE=$( command -v mise || echo ~/.local/bin/mise ) && \
         $MISE run dev",
        dir = work_dir,
    );

    let max_attempts = dev_start_max_attempts();
    tracing::info!(%repo_url, %port, max_attempts, mode = "subprocess", "starting environment (dev loop)");

    let mut last_err = String::from("no dev attempts");
    for attempt in 0..max_attempts {
        tracing::info!(%job_id, attempt = attempt + 1, max = max_attempts, "dev server start attempt");
        match spawn_and_serve(
            "bash",
            &["-c", &dev_script],
            job_id,
            project_id,
            port,
            pool,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = e;
                tracing::warn!(
                    %job_id,
                    attempt = attempt + 1,
                    error = %last_err,
                    "dev server did not become healthy"
                );
                if attempt + 1 < max_attempts {
                    tracing::info!(%job_id, "running OpenCode repair before next dev attempt");
                    if let Err(rep) =
                        run_opencode_repair_pass(pool, project_id, message_id, attempt + 1).await
                    {
                        tracing::warn!(%job_id, error = %rep, "OpenCode repair failed; retrying dev anyway");
                    }
                }
            }
        }
    }

    Err(last_err)
}

/// Container mode: runs the build inside an isolated container.
async fn run_in_container(
    repo_url: &str,
    job_id: uuid::Uuid,
    project_id: uuid::Uuid,
    _message_id: uuid::Uuid,
    pool: &sqlx::PgPool,
) -> Result<(), String> {
    let runtime = detect_runtime().await?;
    let port = port_for_job(job_id);

    let script = format!(
        "apt-get update && apt-get install -y --no-install-recommends \
             git curl ca-certificates build-essential pkg-config libssl-dev && \
         git clone {repo} /work && cd /work && \
         curl -fsSL https://mise.run | bash && \
         ~/.local/bin/mise trust && \
         sed 's/^PORT = .*/PORT = \"{port}\"/' .mise.toml > .mise.toml.tmp && mv .mise.toml.tmp .mise.toml && \
         ~/.local/bin/mise install --yes && \
         ~/.local/bin/mise run dev",
        repo = repo_url,
        port = port,
    );

    tracing::info!(%repo_url, %runtime, %port, mode = "container", "starting environment");

    spawn_and_serve(
        runtime,
        &[
            "run",
            "--rm",
            &format!("--memory={CONTAINER_MEMORY_LIMIT}"),
            "--network=host",
            "docker.io/library/debian:bookworm-slim",
            "bash",
            "-c",
            &script,
        ],
        job_id,
        project_id,
        port,
        pool,
    )
    .await
}

// ── Container runtime detection ────────────────────────────────────────

async fn detect_runtime() -> Result<&'static str, String> {
    for cmd in ["podman", "docker"] {
        let probe = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::process::Command::new(cmd)
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status(),
        )
        .await;

        if matches!(probe, Ok(Ok(s)) if s.success()) {
            tracing::info!(runtime = cmd, "container runtime detected");
            return Ok(cmd);
        }
    }

    Err("neither podman nor docker found in PATH".into())
}

// ── Log flushing ───────────────────────────────────────────────────────

async fn flush_logs(pool: &sqlx::PgPool, job_id: uuid::Uuid, logs: &str) {
    if let Err(e) = sqlx::query("UPDATE build_jobs SET logs = $2, updated_at = NOW() WHERE id = $1")
        .bind(job_id)
        .bind(logs)
        .execute(pool)
        .await
    {
        tracing::warn!(%job_id, error = %e, "failed to flush logs");
    }
}

// ── Spawn, stream logs, wait for healthy, create deployment ────────────

/// Spawns a long-running child process (the dev server), streams its logs,
/// polls `/healthz` until the server is up, then creates a Deployment record.
/// Returns Ok(()) once healthy (the process keeps running in the background).
async fn spawn_and_serve(
    program: &str,
    args: &[&str],
    job_id: uuid::Uuid,
    project_id: uuid::Uuid,
    port: u16,
    pool: &sqlx::PgPool,
) -> Result<(), String> {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .env("MISE_YES", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start {program}: {e}"))?;

    let pid = child.id().unwrap_or(0);
    tracing::info!(%job_id, %pid, %port, "child process spawned");

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let mut stdout_reader = BufReader::new(stdout).lines();
    let mut stderr_reader = BufReader::new(stderr).lines();

    let mut log_buf = String::new();
    let mut dirty = false;
    let mut flush_timer = tokio::time::interval(LOG_FLUSH_INTERVAL);
    flush_timer.tick().await;

    let health_url = format!("http://127.0.0.1:{port}/healthz");
    let mut health_timer = tokio::time::interval(HEALTH_POLL_INTERVAL);
    health_timer.tick().await;
    let health_deadline = tokio::time::Instant::now() + HEALTH_TIMEOUT;

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();

    loop {
        tokio::select! {
            line = stdout_reader.next_line() => {
                match line {
                    Ok(Some(l)) => {
                        if log_buf.len() < MAX_LOG_BYTES {
                            log_buf.push_str(&l);
                            log_buf.push('\n');
                            dirty = true;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            line = stderr_reader.next_line() => {
                match line {
                    Ok(Some(l)) => {
                        if log_buf.len() < MAX_LOG_BYTES {
                            log_buf.push_str(&l);
                            log_buf.push('\n');
                            dirty = true;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            _ = flush_timer.tick() => {
                if dirty {
                    flush_logs(pool, job_id, &log_buf).await;
                    dirty = false;
                }
            }
            _ = health_timer.tick() => {
                if tokio::time::Instant::now() > health_deadline {
                    if dirty { flush_logs(pool, job_id, &log_buf).await; }
                    let _ = child.kill().await;
                    return Err(format!(
                        "child server did not become healthy within {}s",
                        HEALTH_TIMEOUT.as_secs()
                    ));
                }
                if let Ok(resp) = http.get(&health_url).send().await
                    && resp.status().is_success()
                {
                    tracing::info!(%job_id, %port, "child server is healthy");

                    if dirty {
                        flush_logs(pool, job_id, &log_buf).await;
                    }

                    let child_pid = child.id().map(|p| p as i32);
                    let exit_pid = child_pid.unwrap_or(-1);
                    let deployment_id = match create_deployment(pool, job_id, project_id, port, child_pid)
                        .await
                    {
                        Ok(id) => id,
                        Err(e) => {
                            tracing::error!(%job_id, error = %e, "failed to create deployment");
                            let _ = child.kill().await;
                            return Err(e);
                        }
                    };

                    let bg_pool = pool.clone();
                    tokio::spawn(async move {
                        stream_until_exit(
                            &mut stdout_reader,
                            &mut stderr_reader,
                            &mut log_buf,
                            job_id,
                            deployment_id,
                            exit_pid,
                            &bg_pool,
                            child,
                        )
                        .await;
                    });
                    return Ok(());
                }
            }
        }
    }

    // If we reach here, the process exited before becoming healthy
    if dirty {
        flush_logs(pool, job_id, &log_buf).await;
    }

    let status = child.wait().await.map_err(|e| format!("wait: {e}"))?;
    let tail: String = log_buf
        .chars()
        .rev()
        .take(500)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    Err(format!("{program} exited with {status}: …{tail}"))
}

/// Continue streaming logs after the child is marked healthy.
/// When the process exits, mark the deployment as stopped.
async fn stream_until_exit(
    stdout_reader: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    stderr_reader: &mut tokio::io::Lines<BufReader<tokio::process::ChildStderr>>,
    log_buf: &mut String,
    job_id: uuid::Uuid,
    deployment_id: uuid::Uuid,
    exit_pid: i32,
    pool: &sqlx::PgPool,
    mut child: tokio::process::Child,
) {
    let mut dirty = false;
    let mut flush_timer = tokio::time::interval(LOG_FLUSH_INTERVAL);
    flush_timer.tick().await;

    loop {
        tokio::select! {
            line = stdout_reader.next_line() => {
                match line {
                    Ok(Some(l)) => {
                        if log_buf.len() < MAX_LOG_BYTES {
                            log_buf.push_str(&l);
                            log_buf.push('\n');
                            dirty = true;
                        }
                    }
                    _ => break,
                }
            }
            line = stderr_reader.next_line() => {
                match line {
                    Ok(Some(l)) => {
                        if log_buf.len() < MAX_LOG_BYTES {
                            log_buf.push_str(&l);
                            log_buf.push('\n');
                            dirty = true;
                        }
                    }
                    _ => break,
                }
            }
            _ = flush_timer.tick() => {
                if dirty {
                    flush_logs(pool, job_id, log_buf).await;
                    dirty = false;
                }
            }
        }
    }

    if dirty {
        flush_logs(pool, job_id, log_buf).await;
    }

    let exit_status = child.wait().await.ok();
    tracing::info!(%job_id, %exit_pid, ?exit_status, "child process exited");

    // Only mark stopped if this process is still the one recorded (avoids racing a deploy restart).
    if exit_pid >= 0 {
        let res = sqlx::query(
            "UPDATE deployments SET status = 'stopped', active = false, updated_at = NOW() \
             WHERE id = $1 AND pid = $2",
        )
        .bind(deployment_id)
        .bind(exit_pid)
        .execute(pool)
        .await;

        if let Ok(r) = res
            && r.rows_affected() > 0
        {
            tracing::info!(%deployment_id, "deployment marked stopped (dev process exit)");
        }
    }
}

/// Insert a Deployment row and link it back to the BuildJob.
async fn create_deployment(
    pool: &sqlx::PgPool,
    job_id: uuid::Uuid,
    project_id: uuid::Uuid,
    port: u16,
    pid: Option<i32>,
) -> Result<uuid::Uuid, String> {
    let deployment_id = uuid::Uuid::new_v4();
    let subdomain = format!("env-{}", &job_id.to_string()[..8]);
    let url = format!("/env/{deployment_id}/");

    sqlx::query(
        "INSERT INTO deployments \
             (id, status, url, subdomain, provider, port, pid, active, \
              project_id, build_job_id, created_at, updated_at) \
         VALUES ($1, 'running', $2, $3, 'local', $4, $5, true, $6, $7, NOW(), NOW())",
    )
    .bind(deployment_id)
    .bind(&url)
    .bind(&subdomain)
    .bind(port as i32)
    .bind(pid)
    .bind(project_id)
    .bind(job_id)
    .execute(pool)
    .await
    .map_err(|e| format!("insert deployment: {e}"))?;

    sqlx::query("UPDATE build_jobs SET deployment_id = $2, updated_at = NOW() WHERE id = $1")
        .bind(job_id)
        .bind(deployment_id)
        .execute(pool)
        .await
        .map_err(|e| format!("link deployment to job: {e}"))?;

    tracing::info!(%job_id, %deployment_id, %port, "deployment created");
    Ok(deployment_id)
}

/// After OpenCode writes files, restart `mise run dev` in the checkout so the preview reloads.
/// Subprocess mode only; best-effort (never fails the OpenCode build).
pub(super) async fn restart_deployment_after_opencode_build(
    pool: &sqlx::PgPool,
    project_id: uuid::Uuid,
) {
    let mode = std::env::var("SPAWN_MODE").unwrap_or_default();
    if mode != "subprocess" {
        tracing::debug!(%project_id, %mode, "skip deploy restart — not subprocess mode");
        return;
    }

    let row = sqlx::query(
        "SELECT d.id, d.build_job_id, d.port, d.pid \
         FROM deployments d \
         WHERE d.project_id = $1 AND d.active = true AND d.deleted_at IS NULL \
           AND d.status = 'running' \
         ORDER BY d.created_at DESC LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await;

    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => {
            tracing::debug!(%project_id, "no active deployment to restart after build");
            return;
        }
        Err(e) => {
            tracing::warn!(%project_id, error = %e, "deploy restart: query failed");
            return;
        }
    };

    let deployment_id: uuid::Uuid = row.get("id");
    let spawn_job_id: uuid::Uuid = row.get("build_job_id");
    let port: i32 = row.get("port");
    let port: u16 = port.try_into().unwrap_or(28_000);
    let old_pid: Option<i32> = row.get("pid");

    let work_dir = format!("/tmp/stem-cell-{spawn_job_id}");
    if !tokio::fs::try_exists(&work_dir).await.unwrap_or(false) {
        tracing::warn!(%work_dir, "deploy restart: work dir missing");
        return;
    }

    if let Some(pid) = old_pid {
        let _ = sqlx::query(
            "UPDATE deployments SET pid = NULL, updated_at = NOW() \
             WHERE id = $1 AND pid = $2",
        )
        .bind(deployment_id)
        .bind(pid)
        .execute(pool)
        .await;
        super::cleanup_deployments::kill_process(pid).await;
    }

    let script = format!(
        "set -e && cd {dir} && \
         MISE=$( command -v mise || echo ~/.local/bin/mise ) && \
         if [ ! -x \"$MISE\" ]; then \
           curl -fsSL https://mise.run | bash && MISE=~/.local/bin/mise; \
         fi && \
         $MISE trust && \
         $MISE run dev",
        dir = work_dir,
    );

    let msg_id = spawn_message_id_for_project(pool, project_id).await;
    let max_attempts = dev_start_max_attempts();
    let mut last_err = String::from("deploy restart: no attempts");

    for attempt in 0..max_attempts {
        if attempt > 0 {
            tracing::info!(%deployment_id, attempt = attempt + 1, "deploy restart: OpenCode repair before retry");
            if let Some(mid) = msg_id {
                if let Err(e) = run_opencode_repair_pass(pool, project_id, mid, attempt).await {
                    tracing::warn!(%deployment_id, error = %e, "deploy restart: repair pass failed");
                }
            }
        }

        match restart_dev_single_attempt(
            pool,
            deployment_id,
            spawn_job_id,
            port,
            &script,
        )
        .await
        {
            Ok(()) => {
                tracing::info!(%deployment_id, "deploy restart complete");
                return;
            }
            Err(e) => {
                last_err = e;
                tracing::warn!(
                    %deployment_id,
                    attempt = attempt + 1,
                    max = max_attempts,
                    error = %last_err,
                    "deploy restart attempt failed"
                );
            }
        }
    }

    tracing::error!(%deployment_id, error = %last_err, "deploy restart exhausted attempts");
}

/// One `mise run dev` + health wait + attach log stream (existing deployment row).
async fn restart_dev_single_attempt(
    pool: &sqlx::PgPool,
    deployment_id: uuid::Uuid,
    spawn_job_id: uuid::Uuid,
    port: u16,
    script: &str,
) -> Result<(), String> {
    let mut child = tokio::process::Command::new("bash")
        .args(["-c", script])
        .env("MISE_YES", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))?;

    tracing::info!(%deployment_id, %port, "deploy restart: dev process spawned");

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let mut stdout_reader = BufReader::new(stdout).lines();
    let mut stderr_reader = BufReader::new(stderr).lines();

    let mut log_buf = String::new();
    let mut dirty = false;
    let mut flush_timer = tokio::time::interval(LOG_FLUSH_INTERVAL);
    flush_timer.tick().await;

    let health_url = format!("http://127.0.0.1:{port}/healthz");
    let mut health_timer = tokio::time::interval(HEALTH_POLL_INTERVAL);
    health_timer.tick().await;
    let health_deadline = tokio::time::Instant::now() + HEALTH_TIMEOUT;

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();

    loop {
        tokio::select! {
            line = stdout_reader.next_line() => {
                match line {
                    Ok(Some(l)) => {
                        if log_buf.len() < MAX_LOG_BYTES {
                            log_buf.push_str(&l);
                            log_buf.push('\n');
                            dirty = true;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            line = stderr_reader.next_line() => {
                match line {
                    Ok(Some(l)) => {
                        if log_buf.len() < MAX_LOG_BYTES {
                            log_buf.push_str(&l);
                            log_buf.push('\n');
                            dirty = true;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            _ = flush_timer.tick() => {
                if dirty {
                    flush_logs(pool, spawn_job_id, &log_buf).await;
                    dirty = false;
                }
            }
            _ = health_timer.tick() => {
                if tokio::time::Instant::now() > health_deadline {
                    if dirty {
                        flush_logs(pool, spawn_job_id, &log_buf).await;
                    }
                    let _ = child.kill().await;
                    return Err("health check timed out".into());
                }
                if let Ok(resp) = http.get(&health_url).send().await
                    && resp.status().is_success()
                {
                    if dirty {
                        flush_logs(pool, spawn_job_id, &log_buf).await;
                    }

                    let child_pid = child.id().map(|p| p as i32);
                    let exit_pid = child_pid.unwrap_or(-1);
                    if let Err(e) = sqlx::query(
                        "UPDATE deployments SET pid = $2, status = 'running', active = true, \
                         updated_at = NOW() WHERE id = $1",
                    )
                    .bind(deployment_id)
                    .bind(child_pid)
                    .execute(pool)
                    .await
                    {
                        let _ = child.kill().await;
                        return Err(format!("record pid: {e}"));
                    }

                    let bg_pool = pool.clone();
                    tokio::spawn(async move {
                        stream_until_exit(
                            &mut stdout_reader,
                            &mut stderr_reader,
                            &mut log_buf,
                            spawn_job_id,
                            deployment_id,
                            exit_pid,
                            &bg_pool,
                            child,
                        )
                        .await;
                    });
                    return Ok(());
                }
            }
        }
    }

    if dirty {
        flush_logs(pool, spawn_job_id, &log_buf).await;
    }

    let status = child
        .wait()
        .await
        .map_err(|e| format!("wait: {e}"))?;
    Err(format!("dev exited before healthy: {status}"))
}

/// `message_id` from the original spawn-environment build job (user message), for repair builds.
async fn spawn_message_id_for_project(
    pool: &sqlx::PgPool,
    project_id: uuid::Uuid,
) -> Option<uuid::Uuid> {
    let row = sqlx::query(
        "SELECT message_id FROM build_jobs WHERE project_id = $1 AND model = 'container' \
         AND deleted_at IS NULL ORDER BY created_at ASC LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await
    .ok()??;
    Some(row.get("message_id"))
}

/// Latest running deployment for this project (preview URL / checkout anchor).
async fn active_deployment_id_for_project(
    pool: &sqlx::PgPool,
    project_id: uuid::Uuid,
) -> Option<uuid::Uuid> {
    let row = sqlx::query(
        "SELECT id FROM deployments \
         WHERE project_id = $1 AND deleted_at IS NULL AND active = true AND status = 'running' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await
    .ok()??;
    Some(row.get("id"))
}

/// Queue one OpenCode build job and run it to completion (blocks).
async fn run_one_opencode_build(
    pool: &sqlx::PgPool,
    project_id: uuid::Uuid,
    message_id: uuid::Uuid,
    prompt: &str,
) -> Result<crate::system_api::RunBuildOutput, String> {
    let oc_job_id = uuid::Uuid::new_v4();
    let deployment_id = active_deployment_id_for_project(pool, project_id).await;

    sqlx::query(
        "INSERT INTO build_jobs \
             (id, status, prompt_summary, model, tokens_used, error_message, \
              duration_ms, logs, deployment_id, project_id, message_id, created_at, updated_at) \
             VALUES ($1, 'queued', $2, 'opencode', 0, '', 0, '', $3, $4, $5, NOW(), NOW())",
    )
    .bind(oc_job_id)
    .bind(prompt)
    .bind(deployment_id)
    .bind(project_id)
    .bind(message_id)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    tracing::info!(%project_id, %oc_job_id, "OpenCode build job queued");

    let input = crate::system_api::RunBuildInput {
        build_job_id: oc_job_id,
    };

    match <super::AppSystems as crate::system_api::RunBuildSystem>::execute(
        &super::AppSystems,
        pool,
        input,
    )
    .await
    {
        Ok(output) => Ok(output),
        Err(e) => {
            let _ = sqlx::query(
                "UPDATE build_jobs SET status = 'failed', error_message = $2, updated_at = NOW() WHERE id = $1",
            )
            .bind(oc_job_id)
            .bind(format!("{e:?}"))
            .execute(pool)
            .await;
            Err(format!("{e:?}"))
        }
    }
}

async fn run_opencode_repair_pass(
    pool: &sqlx::PgPool,
    project_id: uuid::Uuid,
    message_id: uuid::Uuid,
    round: u32,
) -> Result<(), String> {
    let prompt = format!(
        "{DEV_SERVER_REPAIR_PROMPT}\n\n(Automated repair round {round} — `mise run dev` did not become healthy.)"
    );
    run_one_opencode_build(pool, project_id, message_id, &prompt).await?;
    Ok(())
}

/// Creates a new "opencode" build job and executes RunBuild to transform
/// the deployed template using the user's prompt via OpenCode.
async fn trigger_opencode_build(
    pool: &sqlx::PgPool,
    project_id: uuid::Uuid,
    message_id: uuid::Uuid,
    prompt: &str,
) {
    match run_one_opencode_build(pool, project_id, message_id, prompt).await {
        Ok(output) => {
            tracing::info!(
                %project_id,
                artifacts = output.artifacts_count,
                tokens = output.tokens_used,
                "OpenCode build completed"
            );
        }
        Err(e) => {
            tracing::error!(%project_id, error = %e, "OpenCode build failed");
        }
    }
}
