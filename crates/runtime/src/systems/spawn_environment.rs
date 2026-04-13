use crate::system_api::*;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

/// MVP: hardcoded repo to clone inside the container.
/// Move to env var or input field when ready.
const DEFAULT_REPO_URL: &str = "https://github.com/pinsky-three/stem-cell";

/// Memory limit for spawned sub-containers (prevents OOM on Railway).
/// Needs headroom for rustup component downloads + cargo build.
const CONTAINER_MEMORY_LIMIT: &str = "2g";

/// Max time allowed for the synchronous handler work (DB inserts).
/// If the pool is starved or PG is slow, the caller gets an error instead of
/// hanging forever.
const HANDLER_TIMEOUT: Duration = Duration::from_secs(10);

/// Max time the background container is allowed to run before being killed.
const CONTAINER_TIMEOUT: Duration = Duration::from_secs(600);

#[async_trait::async_trait]
impl SpawnEnvironmentSystem for super::AppSystems {
    async fn execute(
        &self,
        pool: &sqlx::PgPool,
        input: SpawnEnvironmentInput,
    ) -> Result<SpawnEnvironmentOutput, SpawnEnvironmentError> {
        let span = tracing::info_span!(
            "spawn_environment",
            org_id = %input.org_id,
            user_id = %input.user_id,
        );
        let _enter = span.enter();

        // Wrap all DB work in a timeout so the HTTP response never hangs
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

/// All synchronous DB work extracted so we can wrap it in a single timeout.
async fn create_records(
    pool: &sqlx::PgPool,
    input: &SpawnEnvironmentInput,
) -> Result<SpawnEnvironmentOutput, SpawnEnvironmentError> {
    // Upsert anonymous org (landing-page callers won't have seed data)
    sqlx::query(
        "INSERT INTO organizations (id, name, slug, avatar_url, active, created_at, updated_at) \
         VALUES ($1, 'Anonymous', 'anonymous', NULL, true, NOW(), NOW()) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(input.org_id)
    .execute(pool)
    .await
    .map_err(|e| SpawnEnvironmentError::DatabaseError(e.to_string()))?;

    // Upsert anonymous user
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

    // Create project
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

    // Create conversation
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

    // Create message (user's prompt)
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

    // Create build job
    sqlx::query(
        "INSERT INTO build_jobs \
             (id, status, prompt_summary, model, tokens_used, error_message, \
              duration_ms, logs, project_id, message_id, created_at, updated_at) \
         VALUES ($1, 'running', $2, 'container', 0, '', 0, '', $3, $4, NOW(), NOW())",
    )
    .bind(job_id)
    .bind(&input.prompt)
    .bind(project_id)
    .bind(message_id)
    .execute(pool)
    .await
    .map_err(|e| SpawnEnvironmentError::DatabaseError(e.to_string()))?;

    tracing::info!(%project_id, %job_id, "project and job created, spawning container");

    let bg_pool = pool.clone();
    tokio::spawn(async move {
        let started = std::time::Instant::now();

        let result = match tokio::time::timeout(
            CONTAINER_TIMEOUT,
            run_container_streaming(DEFAULT_REPO_URL, job_id, &bg_pool),
        )
        .await
        {
            Ok(inner) => inner,
            Err(_) => Err(format!(
                "container killed after {}s timeout",
                CONTAINER_TIMEOUT.as_secs()
            )),
        };

        let duration_ms = started.elapsed().as_millis() as i64;

        let (status, error_message) = match result {
            Ok(()) => ("succeeded", String::new()),
            Err(e) => {
                tracing::error!(%job_id, error = %e, "container failed");
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

        tracing::info!(%job_id, %status, duration_ms, "container task finished");
    });

    Ok(SpawnEnvironmentOutput {
        project_id: project_id.to_string(),
        job_id: job_id.to_string(),
        status: "running".to_string(),
    })
}

/// Detect the container runtime available on this host.
/// Prefers podman (daemonless, rootless) but falls back to docker.
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

        let ok = matches!(probe, Ok(Ok(s)) if s.success());

        if ok {
            tracing::info!(runtime = cmd, "container runtime detected");
            return Ok(cmd);
        }
    }

    Err("neither podman nor docker found in PATH".into())
}

/// How often we flush accumulated log lines to the database.
const LOG_FLUSH_INTERVAL: Duration = Duration::from_secs(2);

/// Max log size stored per job (prevents unbounded growth).
const MAX_LOG_BYTES: usize = 512 * 1024;

/// Flush the current log buffer to the `logs` column.
async fn flush_logs(pool: &sqlx::PgPool, job_id: uuid::Uuid, logs: &str) {
    if let Err(e) = sqlx::query(
        "UPDATE build_jobs SET logs = $2, updated_at = NOW() WHERE id = $1",
    )
    .bind(job_id)
    .bind(logs)
    .execute(pool)
    .await
    {
        tracing::warn!(%job_id, error = %e, "failed to flush logs");
    }
}

/// Dispatch based on SPAWN_MODE env var: "subprocess" runs directly,
/// anything else (including unset) uses a container runtime.
async fn run_container_streaming(
    repo_url: &str,
    job_id: uuid::Uuid,
    pool: &sqlx::PgPool,
) -> Result<(), String> {
    let mode = std::env::var("SPAWN_MODE").unwrap_or_default();
    if mode == "subprocess" {
        run_subprocess_streaming(repo_url, job_id, pool).await
    } else {
        run_in_container_streaming(repo_url, job_id, pool).await
    }
}

/// Subprocess mode: clone + build directly on the host (no isolation).
/// Used on platforms like Railway where nested containers aren't allowed.
async fn run_subprocess_streaming(
    repo_url: &str,
    job_id: uuid::Uuid,
    pool: &sqlx::PgPool,
) -> Result<(), String> {
    let work_dir = format!("/tmp/stem-cell-{job_id}");

    let script = format!(
        "set -e && \
         git clone {repo} {dir} && cd {dir} && \
         if command -v mise >/dev/null 2>&1; then \
           mise trust && mise install --yes && mise run dev; \
         elif [ -x ~/.local/bin/mise ]; then \
           ~/.local/bin/mise trust && ~/.local/bin/mise install --yes && ~/.local/bin/mise run dev; \
         else \
           curl -fsSL https://mise.run | bash && \
           ~/.local/bin/mise trust && ~/.local/bin/mise install --yes && ~/.local/bin/mise run dev; \
         fi",
        repo = repo_url,
        dir = work_dir,
    );

    tracing::info!(%repo_url, mode = "subprocess", "starting build");

    spawn_and_stream("bash", &["-c", &script], job_id, pool).await
}

/// Container mode: runs the build inside an isolated container.
async fn run_in_container_streaming(
    repo_url: &str,
    job_id: uuid::Uuid,
    pool: &sqlx::PgPool,
) -> Result<(), String> {
    let runtime = detect_runtime().await?;

    let script = format!(
        "apt-get update && apt-get install -y --no-install-recommends \
             git curl ca-certificates build-essential pkg-config libssl-dev && \
         git clone {repo} /work && cd /work && \
         curl -fsSL https://mise.run | bash && \
         ~/.local/bin/mise install --yes && \
         ~/.local/bin/mise run dev",
        repo = repo_url,
    );

    tracing::info!(%repo_url, %runtime, mode = "container", "starting build");

    spawn_and_stream(
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
        pool,
    )
    .await
}

/// Shared streaming logic: spawns a process, reads stdout/stderr line-by-line,
/// and flushes the log buffer to the DB every LOG_FLUSH_INTERVAL.
async fn spawn_and_stream(
    program: &str,
    args: &[&str],
    job_id: uuid::Uuid,
    pool: &sqlx::PgPool,
) -> Result<(), String> {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .env("MISE_YES", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start {program}: {e}"))?;

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let mut stdout_reader = BufReader::new(stdout).lines();
    let mut stderr_reader = BufReader::new(stderr).lines();

    let mut log_buf = String::new();
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
        }
    }

    if dirty {
        flush_logs(pool, job_id, &log_buf).await;
    }

    let status = child
        .wait()
        .await
        .map_err(|e| format!("failed to wait on process: {e}"))?;

    if status.success() {
        Ok(())
    } else {
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
}
