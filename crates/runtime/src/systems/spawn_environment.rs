use crate::system_api::*;
use std::time::Duration;

/// MVP: hardcoded repo to clone inside the container.
/// Move to env var or input field when ready.
const DEFAULT_REPO_URL: &str = "https://github.com/pinsky-three/stem-cell";

/// Memory limit for spawned sub-containers (prevents OOM on Railway).
const CONTAINER_MEMORY_LIMIT: &str = "256m";

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
              duration_ms, project_id, message_id, created_at, updated_at) \
         VALUES ($1, 'running', $2, 'container', 0, '', 0, $3, $4, NOW(), NOW())",
    )
    .bind(job_id)
    .bind(&input.prompt)
    .bind(project_id)
    .bind(message_id)
    .execute(pool)
    .await
    .map_err(|e| SpawnEnvironmentError::DatabaseError(e.to_string()))?;

    tracing::info!(%project_id, %job_id, "project and job created, spawning container");

    // Spawn background task with its own timeout
    let bg_pool = pool.clone();
    tokio::spawn(async move {
        let started = std::time::Instant::now();

        let result = match tokio::time::timeout(
            CONTAINER_TIMEOUT,
            run_container(DEFAULT_REPO_URL),
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

/// Runs the container (called inside tokio::spawn).
async fn run_container(repo_url: &str) -> Result<(), String> {
    let runtime = detect_runtime().await?;

    let script = format!(
        "git clone {repo} /work && cd /work && \
         curl -fsSL https://mise.run | sh && \
         ~/.local/bin/mise install --yes && \
         ~/.local/bin/mise run dev",
        repo = repo_url,
    );

    tracing::info!(%repo_url, %runtime, "starting container");

    let output = tokio::process::Command::new(runtime)
        .args([
            "run",
            "--rm",
            &format!("--memory={CONTAINER_MEMORY_LIMIT}"),
            "--network=host",
            "docker.io/alpine/git",
            "sh",
            "-c",
            &script,
        ])
        .output()
        .await
        .map_err(|e| format!("failed to start {runtime}: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        Err(format!(
            "{runtime} exited with {}: stderr={stderr}, stdout={stdout}",
            output.status
        ))
    }
}
