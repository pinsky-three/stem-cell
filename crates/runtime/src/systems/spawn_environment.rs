use crate::system_api::*;
use opencode_client::types::BuildEvent;
use sqlx::Row;
use std::time::Duration;
use stem_projects::astro_port_patch_snippet;
use stem_sandbox::{ContainerNetwork, ContainerRunSpec, ProcessRunSpec, SandboxId, SandboxRoot};
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::Instrument;

const DEFAULT_REPO_URL: &str = "https://github.com/pinsky-three/stem-cell-shrank";
const CONTAINER_MEMORY_LIMIT: &str = "2g";
const CONTAINER_BASE_IMAGE: &str = "docker.io/library/debian:bookworm-slim";

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

/// Bash snippet that patches `frontend/package.json` so Astro dev binds
/// to `--host 0.0.0.0 --port {port}` and Vite is pinned to ^7.
///
/// The implementation now lives in the shared `stem-projects` crate so
/// the CLI (`stem init`) and the container-mode bootstrap below use the
/// same logic. We keep this re-export comment so call sites stay
/// self-documenting.
/// Characters of combined stdout/stderr tail included in “exited before healthy” errors + `tracing::error`.
const PREVIEW_EXIT_LOG_TAIL_CHARS: usize = 4096;

/// How many times we try `mise run dev` + health before giving up (with OpenCode repair between tries).
fn dev_start_max_attempts() -> u32 {
    std::env::var("STEM_CELL_DEV_START_ATTEMPTS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| (1..=10).contains(&n))
        .unwrap_or(3)
}

/// Normalize DB `projects.scope` (empty string from legacy additive migration → frontend).
fn effective_preview_scope(raw: &str) -> &'static str {
    match raw {
        "full" => "full",
        _ => "frontend",
    }
}

/// Bash fragment after `MISE=…` is set: brings preview up. Frontend runs **`frontend:install`**
/// before **`frontend:dev`** so `node_modules/.bin/astro` exists (clone + `mise install` does not run npm).
fn mise_preview_command_chain(scope_effective: &str) -> &'static str {
    match scope_effective {
        "full" => "\"$MISE\" run dev",
        _ => "\"$MISE\" run frontend:install && \"$MISE\" run frontend:dev",
    }
}

fn preview_commands_label(scope_effective: &str) -> &'static str {
    match scope_effective {
        "full" => "mise run dev",
        _ => "mise run frontend:install && mise run frontend:dev",
    }
}

/// Container bootstrap uses a fixed mise path (no `$MISE` in that script).
fn container_mise_preview_chain(scope_effective: &str) -> &'static str {
    match scope_effective {
        "full" => "~/.local/bin/mise run dev",
        _ => "~/.local/bin/mise run frontend:install && ~/.local/bin/mise run frontend:dev",
    }
}

/// Heuristic: log immediately at WARN so failures like `astro: command not found` show up before health timeout.
fn preview_log_line_smells_fatal(line: &str) -> bool {
    let s = line.to_ascii_lowercase();
    s.contains("command not found")
        || s.contains("npm err")
        || s.contains("error task failed")
        || s.contains("elifecycle")
        || s.contains("eaddrinuse")
        || s.contains("enoent")
        || s.contains("cannot find module")
        || s.contains("failed to resolve")
        || s.contains("failed to load")
        || s.contains("is not recognized as an internal or external command")
        || (s.contains("error:")
            && (s.contains("astro") || s.contains("vite") || s.contains("esbuild")))
}

async fn publish_deploy_status(
    project_id: uuid::Uuid,
    job_id: uuid::Uuid,
    phase: &str,
    message: &str,
) {
    let event = BuildEvent::DeployStatus {
        job_id: job_id.to_string(),
        project_id: project_id.to_string(),
        phase: phase.to_string(),
        message: message.to_string(),
    };
    let bus = super::run_build::event_bus();
    let readers = bus.read().await;
    if let Some(tx) = readers.get(&project_id) {
        let _ = tx.send(event);
    }
}

fn preview_log_tail_for_error(log_buf: &str) -> String {
    if log_buf.len() <= PREVIEW_EXIT_LOG_TAIL_CHARS {
        return log_buf.to_string();
    }
    log_buf
        .chars()
        .rev()
        .take(PREVIEW_EXIT_LOG_TAIL_CHARS)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

/// Instructions embedded in OpenCode repair prompts — must match actual dev task + health URL.
fn dev_server_repair_body(scope_raw: &str) -> &'static str {
    match effective_preview_scope(scope_raw) {
        "full" => {
            "\
The local preview runs `mise run dev` and must serve GET /healthz on the port from .mise.toml. \
A previous attempt failed to become healthy in time. \
Inspect the repo: fix broken dependencies, config, missing env, port binding, or startup errors so \
`mise install` and `mise run dev` succeed. Prefer minimal changes; keep the app runnable."
        }
        _ => {
            "\
The host runs `mise run frontend:install` then `mise run frontend:dev` (Astro in `frontend/`). The preview must respond with HTTP 200 \
on GET / at the port from .mise.toml. \
A previous attempt failed to become healthy in time. \
Inspect the repo: fix broken frontend dependencies, Astro config, missing env, port binding, or startup errors so \
`mise install`, `mise run frontend:install`, and `mise run frontend:dev` succeed. Prefer minimal changes under `frontend/src/`; keep the preview runnable."
        }
    }
}

async fn fetch_project_scope_raw(
    pool: &sqlx::PgPool,
    project_id: uuid::Uuid,
) -> Result<String, String> {
    let row = sqlx::query("SELECT scope FROM projects WHERE id = $1 AND deleted_at IS NULL")
        .bind(project_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "project not found".to_string())?;
    Ok(row.get::<String, _>("scope"))
}

/// True if an OpenCode primary or repair job is already queued or running for this project.
async fn has_active_opencode_job(
    pool: &sqlx::PgPool,
    project_id: uuid::Uuid,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query(
        "SELECT 1 AS x FROM build_jobs \
         WHERE project_id = $1 AND deleted_at IS NULL \
           AND model IN ('opencode', 'opencode-repair') \
           AND status IN ('queued', 'running') \
         LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Dev servers run as `bash -c '… mise run dev'`. Without a dedicated process group, `kill` only
/// hits bash and node/vite can keep listening → **Address already in use** on restart.
#[cfg(unix)]
fn configure_unix_process_group(cmd: &mut tokio::process::Command) {
    use std::os::unix::process::CommandExt;
    let _ = cmd.as_std_mut().process_group(0);
}

#[cfg(not(unix))]
fn configure_unix_process_group(_cmd: &mut tokio::process::Command) {}

/// After SIGTERM/SIGKILL, wait until nothing accepts this port (avoids EADDRINUSE on rapid restart).
async fn wait_until_port_released(port: u16, timeout: Duration) {
    stem_sandbox::wait_until_port_released(port, timeout).await;
}

fn sandbox_work_dir(job_id: uuid::Uuid) -> std::path::PathBuf {
    SandboxRoot::temp_default().work_dir(&SandboxId::from_uuid(job_id))
}

async fn kill_dev_tree(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        super::cleanup_deployments::kill_process(pid as i32).await;
    }
    let _ = child.wait().await;
}

#[async_trait::async_trait]
impl SpawnEnvironmentSystem for super::AppSystems {
    async fn execute(
        &self,
        pool: &sqlx::PgPool,
        input: SpawnEnvironmentInput,
    ) -> Result<SpawnEnvironmentOutput, SpawnEnvironmentError> {
        super::cleanup_deployments::ensure_periodic_cleanup(pool.clone());

        // Use `.instrument(span).await` instead of holding an Enter guard
        // across `.await` points. The guard form is unsound in async code —
        // tasks polled across threads can drop guards out of order relative
        // to span lifetimes, which made tracing-subscriber panic with
        // "tried to clone a span that already closed" on follow-up calls.
        let span = tracing::info_span!(
            "spawn_environment",
            org_id = %input.org_id,
            user_id = %input.user_id,
        );
        async move {
            let work = async {
                if let Some(pid) = input.project_id {
                    append_prompt_and_queue_opencode(pool, &input, pid).await
                } else {
                    create_records(pool, &input).await
                }
            };

            match tokio::time::timeout(HANDLER_TIMEOUT, work).await {
                Ok(inner) => inner,
                Err(_) => {
                    tracing::error!("handler timed out waiting for database");
                    Err(SpawnEnvironmentError::DatabaseError(
                        "request timed out — database may be overloaded".into(),
                    ))
                }
            }
        }
        .instrument(span)
        .await
    }
}

/// Follow-up message on an existing project: no clone/deploy — reuse checkout, deployment, and
/// OpenCode session when we have a prior succeeded opencode job.
async fn append_prompt_and_queue_opencode(
    pool: &sqlx::PgPool,
    input: &SpawnEnvironmentInput,
    project_id: uuid::Uuid,
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

    let proj_row = sqlx::query("SELECT org_id FROM projects WHERE id = $1 AND deleted_at IS NULL")
        .bind(project_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| SpawnEnvironmentError::DatabaseError(e.to_string()))?
        .ok_or_else(|| SpawnEnvironmentError::SpawnFailed("project not found or deleted".into()))?;

    let proj_org: uuid::Uuid = proj_row.get("org_id");
    if proj_org != input.org_id {
        return Err(SpawnEnvironmentError::SpawnFailed(
            "project_id does not belong to the given org_id".into(),
        ));
    }

    if has_active_opencode_job(pool, project_id)
        .await
        .map_err(|e| SpawnEnvironmentError::DatabaseError(e.to_string()))?
    {
        return Err(SpawnEnvironmentError::SpawnFailed(
            "A build is already in progress for this project — wait for it to finish.".into(),
        ));
    }

    let conv_row = sqlx::query(
        "SELECT id FROM conversations \
         WHERE project_id = $1 AND deleted_at IS NULL \
         ORDER BY created_at ASC LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| SpawnEnvironmentError::DatabaseError(e.to_string()))?
    .ok_or_else(|| SpawnEnvironmentError::SpawnFailed("no conversation for project".into()))?;

    let conversation_id: uuid::Uuid = conv_row.get("id");
    let message_id = uuid::Uuid::new_v4();

    sqlx::query(
        "INSERT INTO messages \
             (id, role, content, sort_order, has_attachment, \
              conversation_id, author_id, created_at, updated_at) \
         VALUES ($1, 'user', $2, \
                 (SELECT COALESCE(MAX(sort_order),0)+1 FROM messages WHERE conversation_id = $3), \
                 false, $3, $4, NOW(), NOW())",
    )
    .bind(message_id)
    .bind(&input.prompt)
    .bind(conversation_id)
    .bind(input.user_id)
    .execute(pool)
    .await
    .map_err(|e| SpawnEnvironmentError::DatabaseError(e.to_string()))?;

    let seed_session: Option<String> = sqlx::query(
        "SELECT opencode_session_id AS sid FROM build_jobs \
         WHERE project_id = $1 AND model = 'opencode' AND deleted_at IS NULL \
           AND opencode_session_id IS NOT NULL AND status = 'succeeded' \
         ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| SpawnEnvironmentError::DatabaseError(e.to_string()))?
    .and_then(|r| r.try_get::<String, _>("sid").ok());

    let oc_job_id = uuid::Uuid::new_v4();
    insert_opencode_build_job_row(
        pool,
        oc_job_id,
        project_id,
        message_id,
        &input.prompt,
        seed_session.as_deref(),
        "opencode",
    )
    .await
    .map_err(|e| SpawnEnvironmentError::DatabaseError(e.to_string()))?;

    tracing::info!(
        %project_id,
        %oc_job_id,
        reuse_session = seed_session.is_some(),
        "follow-up prompt queued (OpenCode only)"
    );

    let bg_pool = pool.clone();
    tokio::spawn(async move {
        match invoke_run_build(&bg_pool, oc_job_id).await {
            Ok(output) => {
                tracing::info!(
                    %project_id,
                    artifacts = output.artifacts_count,
                    tokens = output.tokens_used,
                    "OpenCode follow-up build completed"
                );
            }
            Err(e) => {
                let _ = sqlx::query(
                    "UPDATE build_jobs SET status = 'failed', error_message = $2, updated_at = NOW() WHERE id = $1",
                )
                .bind(oc_job_id)
                .bind(format!("{e:?}"))
                .execute(&bg_pool)
                .await;
                tracing::error!(%project_id, %oc_job_id, error = %e, "OpenCode follow-up failed");
            }
        }
    });

    Ok(SpawnEnvironmentOutput {
        project_id: project_id.to_string(),
        job_id: oc_job_id.to_string(),
        status: "running".to_string(),
    })
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
             (id, name, slug, description, status, framework, visibility, scope, active, \
              org_id, creator_id, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, 'active', NULL, 'private', 'frontend', true, $5, $6, NOW(), NOW())",
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

        publish_deploy_status(
            project_id,
            job_id,
            "cloning",
            "Cloning template repository…",
        )
        .await;

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
                publish_deploy_status(project_id, job_id, "failed", &e).await;
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

        if status == "succeeded" {
            publish_deploy_status(
                project_id,
                job_id,
                "opencode_starting",
                "Preview is live — starting AI transformation…",
            )
            .await;

            trigger_opencode_build(&bg_pool, project_id, message_id, &prompt_for_opencode).await;
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
///
/// The actual filesystem + shell pipeline lives in the shared
/// `stem-projects` crate so the `stem` CLI runs the same bootstrap via
/// `stem init`. This wrapper only does the job_id/string-error
/// plumbing the runtime's contract system layer expects.
async fn run_subprocess_setup(
    repo_url: &str,
    job_id: uuid::Uuid,
    work_dir: &str,
    port: u16,
) -> Result<(), String> {
    tracing::info!(%job_id, %repo_url, "subprocess: clone and toolchain install");

    let dest = std::path::PathBuf::from(work_dir);
    let project = stem_projects::clone_repo(repo_url, &dest, stem_projects::CloneOpts::default())
        .await
        .map_err(|e| format!("clone_repo: {e}"))?;

    stem_projects::install_toolchain(
        &project,
        stem_projects::InstallOpts {
            port,
            skip_mise_install: false,
        },
    )
    .await
    .map_err(|e| format!("install_toolchain: {e}"))?;

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
    let work_dir = sandbox_work_dir(job_id);

    publish_deploy_status(project_id, job_id, "installing", "Installing toolchain…").await;
    let work_dir_str = work_dir.to_string_lossy().to_string();
    run_subprocess_setup(repo_url, job_id, &work_dir_str, port).await?;
    publish_deploy_status(project_id, job_id, "starting", "Starting preview server…").await;

    let scope_raw = fetch_project_scope_raw(pool, project_id).await?;
    let scope_eff = effective_preview_scope(scope_raw.as_str());
    let mise_chain = mise_preview_command_chain(scope_eff);
    let health_path = match scope_eff {
        "full" => "/healthz",
        _ => "/",
    };

    let dev_script = ProcessRunSpec::new(work_dir.clone(), port, mise_chain).bash_script();

    let max_attempts = dev_start_max_attempts();
    tracing::info!(
        %repo_url,
        %port,
        max_attempts,
        scope = %scope_raw,
        preview = preview_commands_label(scope_eff),
        %health_path,
        mode = "subprocess",
        "starting environment (dev loop)"
    );

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
            health_path,
            preview_commands_label(scope_eff),
            None,
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
                    wait_until_port_released(port, Duration::from_secs(15)).await;
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

/// `docker run` does not stream **image pull** through the container attach; pull first so job logs
/// show layer progress instead of minutes of silence before `bash -c` starts.
async fn pull_container_base_image_logged(
    runtime: &str,
    job_id: uuid::Uuid,
    pool: &sqlx::PgPool,
) -> Result<String, String> {
    tracing::info!(
        %job_id,
        image = CONTAINER_BASE_IMAGE,
        %runtime,
        "pulling container base image (progress streams into build job logs)"
    );

    let mut cmd = tokio::process::Command::new(runtime);
    cmd.arg("pull")
        .arg(CONTAINER_BASE_IMAGE)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("{runtime} pull spawn: {e}"))?;

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("{runtime} pull: no stderr pipe"))?;

    let mut reader = PreviewLogLines::new(stderr);
    let mut log_buf = String::from("--- docker pull (base image) ---\n");
    flush_logs(pool, job_id, &log_buf).await;

    loop {
        match reader.next_segment().await {
            Ok(Some(l)) if !l.is_empty() => {
                if log_buf.len() < MAX_LOG_BYTES {
                    log_buf.push_str("[docker pull] ");
                    log_buf.push_str(&l);
                    log_buf.push('\n');
                    flush_logs(pool, job_id, &log_buf).await;
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => return Err(format!("{runtime} pull log read: {e}")),
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| format!("{runtime} pull wait: {e}"))?;
    if !status.success() {
        return Err(format!(
            "{runtime} pull failed ({status}) — fix network/registry access or pre-pull {CONTAINER_BASE_IMAGE}"
        ));
    }

    log_buf.push_str("--- pull complete; starting container ---\n");
    flush_logs(pool, job_id, &log_buf).await;
    tracing::info!(%job_id, image = CONTAINER_BASE_IMAGE, "container base image pull finished");
    Ok(log_buf)
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

    // Subprocess mode publishes installing/starting before the dev child; container mode used to
    // only emit "cloning" then go silent for minutes while Docker runs apt/clone/mise/npm.
    publish_deploy_status(
        project_id,
        job_id,
        "installing",
        "Container preview: image pull, OS packages, git clone, mise, and npm — often 2–5 min the first time.",
    )
    .await;

    let scope_raw = fetch_project_scope_raw(pool, project_id).await?;
    let scope_eff = effective_preview_scope(scope_raw.as_str());
    let container_chain = container_mise_preview_chain(scope_eff);
    let health_path = match scope_eff {
        "full" => "/healthz",
        _ => "/",
    };

    // Explicit `echo` + `git --progress`: Docker/apt often buffer pipe output for long stretches;
    // newline-terminated milestones always reach the host log stream.
    //
    // TODO(phase1.5): container bootstrap still inlines its own `git clone`
    // and `mise install` steps. Subprocess mode has already been moved to
    // `stem_projects::{clone_repo, install_toolchain}`. Folding container
    // mode in requires piping the crate's logic through `bash -c` inside
    // the container; track as a follow-up to keep phase 1 reviewable.
    let astro_patch = astro_port_patch_snippet(port);
    let script = format!(
        "set -e && export DEBIAN_FRONTEND=noninteractive && \
         echo '[stem-cell] container bootstrap (port {port})' && \
         echo '[stem-cell] apt-get update…' && \
         apt-get update && \
         echo '[stem-cell] installing OS packages (git, curl, build tools)…' && \
         apt-get install -y --no-install-recommends \
             git curl ca-certificates build-essential pkg-config libssl-dev && \
         echo '[stem-cell] cloning {repo} → /work …' && \
         GIT_TERMINAL_PROMPT=0 git clone --progress {repo} /work && \
         echo '[stem-cell] clone done; configuring port and toolchain…' && \
         cd /work && \
         curl -fsSL https://mise.run | bash && \
         ~/.local/bin/mise trust && \
         sed 's/^PORT = .*/PORT = \"{port}\"/' .mise.toml > .mise.toml.tmp && mv .mise.toml.tmp .mise.toml && \
         if [ -f .env ]; then \
           _sc_env=$(mktemp) || exit 1; \
           (grep -vE '^[[:space:]]*PORT=' .env || true) > \"$_sc_env\" && \
           printf 'PORT=%s\\n' '{port}' >> \"$_sc_env\" && \
           mv \"$_sc_env\" .env; \
         fi && \
         {astro_patch} && \
         echo '[stem-cell] mise install (may take a few minutes)…' && \
         export PORT={port} && \
         ~/.local/bin/mise install --yes && \
         echo '[stem-cell] starting preview tasks…' && \
         {container_chain}",
        repo = repo_url,
        port = port,
        container_chain = container_chain,
        astro_patch = astro_patch,
    );

    tracing::info!(
        %repo_url,
        %runtime,
        %port,
        scope = %scope_raw,
        preview = preview_commands_label(scope_eff),
        %health_path,
        mode = "container",
        "starting environment"
    );

    publish_deploy_status(
        project_id,
        job_id,
        "starting",
        "Starting container and dev server…",
    )
    .await;

    let pull_log = pull_container_base_image_logged(runtime, job_id, pool).await?;

    let container_spec = ContainerRunSpec {
        runtime: runtime.to_string(),
        image: CONTAINER_BASE_IMAGE.to_string(),
        memory_limit: CONTAINER_MEMORY_LIMIT.to_string(),
        // Compatibility mode: the proxy/health checks still target localhost.
        network: ContainerNetwork::Host,
        port,
        script,
    };
    let container_args = container_spec.docker_args();
    let container_arg_refs: Vec<&str> = container_args.iter().map(String::as_str).collect();

    spawn_and_serve(
        runtime,
        &container_arg_refs,
        job_id,
        project_id,
        port,
        pool,
        health_path,
        preview_commands_label(scope_eff),
        Some(pull_log),
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

/// Piped `docker run` / `apt-get` / `npm` often emit **`\r`-only** progress lines. `BufReader::lines()`
/// waits for `\n`, so we would buffer forever and `log_bytes` stayed 0 until timeout.
struct PreviewLogLines<R: tokio::io::AsyncRead + Unpin> {
    inner: BufReader<R>,
    pending: Vec<u8>,
    skip_lf_after_cr: bool,
}

impl<R: tokio::io::AsyncRead + Unpin> PreviewLogLines<R> {
    fn new(read: R) -> Self {
        Self {
            inner: BufReader::new(read),
            pending: Vec::new(),
            skip_lf_after_cr: false,
        }
    }

    async fn next_segment(&mut self) -> std::io::Result<Option<String>> {
        if self.skip_lf_after_cr {
            self.skip_lf_after_cr = false;
            let buf = self.inner.fill_buf().await?;
            if !buf.is_empty() && buf[0] == b'\n' {
                self.inner.consume(1);
            }
        }

        loop {
            let buf = self.inner.fill_buf().await?;
            if buf.is_empty() {
                if self.pending.is_empty() {
                    return Ok(None);
                }
                let out = String::from_utf8_lossy(&self.pending).into_owned();
                self.pending.clear();
                return Ok(Some(out));
            }

            if let Some(i) = buf.iter().position(|&b| b == b'\n' || b == b'\r') {
                let delim = buf[i];
                self.pending.extend_from_slice(&buf[..i]);
                let out = String::from_utf8_lossy(&self.pending).into_owned();
                self.pending.clear();
                let buf_len = buf.len();

                if delim == b'\n' {
                    self.inner.consume(i + 1);
                    return Ok(Some(out));
                }

                // `\r` — same line refresh (apt) or `\r\n` line ending
                let after = i + 1;
                let has_crlf = after < buf_len && buf[after] == b'\n';
                if has_crlf {
                    self.inner.consume(after + 1);
                } else {
                    self.inner.consume(after);
                    if after == buf_len {
                        self.skip_lf_after_cr = true;
                    }
                }
                return Ok(Some(out));
            }

            self.pending.extend_from_slice(buf);
            let n = buf.len();
            self.inner.consume(n);
            // Rare: no `\n`/`\r` for a huge prefix — cap so we do not grow without bound.
            if self.pending.len() > MAX_LOG_BYTES {
                let keep = MAX_LOG_BYTES / 2;
                let drain = self.pending.len().saturating_sub(keep);
                self.pending.drain(..drain);
            }
        }
    }
}

// ── Spawn, stream logs, wait for healthy, create deployment ────────────

/// Spawns a long-running child process (the dev server), streams its logs,
/// polls `health_path` (e.g. `/healthz` for full stack, `/` for Astro-only) until the server is up,
/// then creates a Deployment record.
/// Returns Ok(()) once healthy (the process keeps running in the background).
async fn spawn_and_serve(
    program: &str,
    args: &[&str],
    job_id: uuid::Uuid,
    project_id: uuid::Uuid,
    port: u16,
    pool: &sqlx::PgPool,
    health_path: &str,
    preview_label: &str,
    // Prepended to streamed logs (e.g. `docker pull` output for container mode).
    initial_logs: Option<String>,
) -> Result<(), String> {
    let cmd_hint: String = args
        .windows(2)
        .find(|w| w[0] == "-c")
        .map(|w| {
            let c = w[1];
            let mut t: String = c.chars().take(400).collect();
            if c.len() > 400 {
                t.push_str(&format!("… ({} chars total)", c.len()));
            }
            t
        })
        .unwrap_or_else(|| program.to_string());

    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .env("MISE_YES", "1")
        .env("PORT", port.to_string())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    configure_unix_process_group(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to start {program}: {e}"))?;

    let pid = child.id().unwrap_or(0);
    let health_url = format!("http://localhost:{port}{health_path}");
    tracing::info!(
        %job_id,
        %pid,
        %port,
        %health_url,
        preview = %preview_label,
        program,
        cmd_hint = %cmd_hint,
        "preview child spawned — streaming logs until healthy or exit"
    );

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let mut stdout_reader = PreviewLogLines::new(stdout);
    let mut stderr_reader = PreviewLogLines::new(stderr);

    let mut log_buf = initial_logs.unwrap_or_default();
    if !log_buf.is_empty() && !log_buf.ends_with('\n') {
        log_buf.push('\n');
    }
    let mut dirty = !log_buf.is_empty();
    if dirty {
        flush_logs(pool, job_id, &log_buf).await;
        dirty = false;
    }
    let mut flush_timer = tokio::time::interval(LOG_FLUSH_INTERVAL);
    flush_timer.tick().await;

    let mut health_timer = tokio::time::interval(HEALTH_POLL_INTERVAL);
    health_timer.tick().await;
    let health_deadline = tokio::time::Instant::now() + HEALTH_TIMEOUT;
    let mut health_probes: u32 = 0;

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();

    loop {
        tokio::select! {
            seg = stdout_reader.next_segment() => {
                match seg {
                    Ok(Some(l)) if !l.is_empty() => {
                        if preview_log_line_smells_fatal(&l) {
                            tracing::warn!(
                                %job_id,
                                stream = "stdout",
                                preview = %preview_label,
                                line = %l,
                                "preview: error-shaped stdout (early signal)"
                            );
                        }
                        if log_buf.len() < MAX_LOG_BYTES {
                            log_buf.push_str(&l);
                            log_buf.push('\n');
                            dirty = true;
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            seg = stderr_reader.next_segment() => {
                match seg {
                    Ok(Some(l)) if !l.is_empty() => {
                        if preview_log_line_smells_fatal(&l) {
                            tracing::warn!(
                                %job_id,
                                stream = "stderr",
                                preview = %preview_label,
                                line = %l,
                                "preview: error-shaped stderr (early signal)"
                            );
                        }
                        if log_buf.len() < MAX_LOG_BYTES {
                            log_buf.push_str(&l);
                            log_buf.push('\n');
                            dirty = true;
                        }
                    }
                    Ok(Some(_)) => {}
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
                    let tail = preview_log_tail_for_error(&log_buf);
                    tracing::error!(
                        %job_id,
                        %port,
                        %health_url,
                        preview = %preview_label,
                        log_tail = %tail,
                        log_bytes = log_buf.len(),
                        "preview: health check deadline exceeded — killing child"
                    );
                    kill_dev_tree(&mut child).await;
                    return Err(format!(
                        "child server did not become healthy within {}s (see log_tail in tracing event)",
                        HEALTH_TIMEOUT.as_secs()
                    ));
                }
                health_probes = health_probes.saturating_add(1);
                // Long-running first boot: nudge UI + operators without spamming every probe.
                if health_probes > 1 && health_probes % 20 == 0 {
                    let waited_secs = health_probes.saturating_mul(HEALTH_POLL_INTERVAL.as_secs() as u32);
                    tracing::info!(
                        %job_id,
                        %health_url,
                        probe = health_probes,
                        waited_secs,
                        preview = %preview_label,
                        "preview: still waiting for HTTP 200 on health URL"
                    );
                    publish_deploy_status(
                        project_id,
                        job_id,
                        "waiting",
                        &format!(
                            "Still waiting for preview ({waited_secs}s) — install or dev server may be slow; see Logs tab."
                        ),
                    )
                    .await;
                }
                match http.get(&health_url).send().await {
                    Ok(resp) => {
                        let st = resp.status();
                        if st.is_success() {
                            tracing::info!(
                                %job_id,
                                %port,
                                %health_url,
                                preview = %preview_label,
                                "child server is healthy"
                            );

                            publish_deploy_status(
                                project_id,
                                job_id,
                                "healthy",
                                "Preview server is live",
                            )
                            .await;

                            if dirty {
                                flush_logs(pool, job_id, &log_buf).await;
                            }

                            let child_pid = child.id().map(|p| p as i32);
                            let exit_pid = child_pid.unwrap_or(-1);
                            let deployment_id = match create_deployment(
                                pool,
                                job_id,
                                project_id,
                                port,
                                child_pid,
                            )
                            .await
                            {
                                Ok(id) => id,
                                Err(e) => {
                                    tracing::error!(%job_id, error = %e, "failed to create deployment");
                                    kill_dev_tree(&mut child).await;
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
                        } else if health_probes == 1 || health_probes % 10 == 0 {
                            tracing::warn!(
                                %job_id,
                                %health_url,
                                http_status = %st,
                                probe = health_probes,
                                preview = %preview_label,
                                "preview: health URL reachable but not 200 yet"
                            );
                        }
                    }
                    Err(e) => {
                        if health_probes == 1 || health_probes % 20 == 0 {
                            tracing::debug!(
                                %job_id,
                                %health_url,
                                probe = health_probes,
                                error = %e,
                                preview = %preview_label,
                                "preview: health probe failed (connection refused while starting is normal)"
                            );
                        }
                    }
                }
            }
        }
    }

    // If we reach here, the process exited before becoming healthy
    if dirty {
        flush_logs(pool, job_id, &log_buf).await;
    }

    let status = child.wait().await.map_err(|e| format!("wait: {e}"))?;
    let tail = preview_log_tail_for_error(&log_buf);
    tracing::error!(
        %job_id,
        %port,
        program,
        preview = %preview_label,
        exit = %status,
        log_tail = %tail,
        "preview child exited before becoming healthy"
    );
    Err(format!("{program} exited with {status}: …{tail}"))
}

/// Continue streaming logs after the child is marked healthy.
/// When the process exits, mark the deployment as stopped.
async fn stream_until_exit(
    stdout_reader: &mut PreviewLogLines<tokio::process::ChildStdout>,
    stderr_reader: &mut PreviewLogLines<tokio::process::ChildStderr>,
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
            seg = stdout_reader.next_segment() => {
                match seg {
                    Ok(Some(l)) if !l.is_empty() => {
                        if log_buf.len() < MAX_LOG_BYTES {
                            log_buf.push_str(&l);
                            log_buf.push('\n');
                            dirty = true;
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            seg = stderr_reader.next_segment() => {
                match seg {
                    Ok(Some(l)) if !l.is_empty() => {
                        if log_buf.len() < MAX_LOG_BYTES {
                            log_buf.push_str(&l);
                            log_buf.push('\n');
                            dirty = true;
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => break,
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

/// After OpenCode writes files that Vite's own file watcher will pick up
/// (page edits under `frontend/src/**`, no deps/config/env touched), we
/// don't need to kill and re-launch `mise run dev`. Vite has already
/// invalidated its module graph from the fs event; the iframe just has
/// to re-fetch the document so the user sees the new markup.
///
/// This function emits a lightweight `deploy.status` event with phase
/// `soft_reload`; the frontend reacts by bumping the iframe reload
/// nonce **without** showing the heavy "restarting preview…" overlay.
/// No processes are touched — worst case is a wasted round-trip.
///
/// Subprocess mode only; best-effort. Safe to call even when no
/// deployment row exists (it just becomes a no-op).
pub(super) async fn soft_reload_preview_after_opencode_build(
    pool: &sqlx::PgPool,
    project_id: uuid::Uuid,
) {
    let mode = std::env::var("SPAWN_MODE").unwrap_or_default();
    if mode != "subprocess" {
        tracing::debug!(%project_id, %mode, "skip soft reload — not subprocess mode");
        return;
    }

    let row = sqlx::query(
        "SELECT d.build_job_id \
         FROM deployments d \
         WHERE d.project_id = $1 AND d.active = true AND d.deleted_at IS NULL \
           AND d.status = 'running' \
         ORDER BY d.created_at DESC LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await;

    let spawn_job_id: uuid::Uuid = match row {
        Ok(Some(r)) => r.get("build_job_id"),
        Ok(None) => {
            tracing::debug!(%project_id, "no active deployment to soft-reload");
            return;
        }
        Err(e) => {
            tracing::warn!(%project_id, error = %e, "soft reload: query failed");
            return;
        }
    };

    tracing::info!(
        %project_id,
        %spawn_job_id,
        "emitting soft_reload (Vite stays alive, iframe just refetches)"
    );

    publish_deploy_status(project_id, spawn_job_id, "soft_reload", "Applying changes…").await;
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
        "SELECT d.id, d.build_job_id, d.port, d.pid, p.scope AS scope \
         FROM deployments d \
         JOIN projects p ON p.id = d.project_id AND p.deleted_at IS NULL \
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
    let scope_raw: String = row.get("scope");

    let work_dir = sandbox_work_dir(spawn_job_id);
    if !tokio::fs::try_exists(&work_dir).await.unwrap_or(false) {
        tracing::warn!(work_dir = %work_dir.display(), "deploy restart: work dir missing");
        return;
    }

    // Let the frontend hide the iframe behind a "restarting preview…" overlay
    // for the ~10–15 s window where Vite is down and the proxy can only
    // return 502s. Without this event the user sees raw "upstream error"
    // and can't tell the preview is about to come back on its own.
    publish_deploy_status(
        project_id,
        spawn_job_id,
        "restart_started",
        "Applying changes and restarting preview…",
    )
    .await;

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
        wait_until_port_released(port, Duration::from_secs(25)).await;
    }

    let scope_eff = effective_preview_scope(scope_raw.as_str());
    let mise_chain = mise_preview_command_chain(scope_eff);
    let health_path = match scope_eff {
        "full" => "/healthz",
        _ => "/",
    };

    let script = format!(
        "set -e && export PORT={port} && cd {dir} && \
         if [ -f .env ]; then \
           _sc_env=$(mktemp) || exit 1; \
           (grep -vE '^[[:space:]]*PORT=' .env || true) > \"$_sc_env\" && \
           printf 'PORT=%s\\n' '{port}' >> \"$_sc_env\" && \
           mv \"$_sc_env\" .env; \
         fi && \
         MISE=$( command -v mise || echo ~/.local/bin/mise ) && \
         if [ ! -x \"$MISE\" ]; then \
           curl -fsSL https://mise.run | bash && MISE=~/.local/bin/mise; \
         fi && \
         $MISE trust && \
         {mise_chain}",
        dir = work_dir.display(),
        port = port,
        mise_chain = mise_chain,
    );

    let msg_id = spawn_message_id_for_project(pool, project_id).await;
    let max_attempts = dev_start_max_attempts();
    let mut last_err = String::from("deploy restart: no attempts");

    for attempt in 0..max_attempts {
        if attempt > 0 {
            wait_until_port_released(port, Duration::from_secs(15)).await;
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
            health_path,
            preview_commands_label(scope_eff),
        )
        .await
        {
            Ok(()) => {
                tracing::info!(%deployment_id, "deploy restart complete");
                publish_deploy_status(
                    project_id,
                    spawn_job_id,
                    "restart_healthy",
                    "Preview is ready.",
                )
                .await;
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
    publish_deploy_status(
        project_id,
        spawn_job_id,
        "restart_failed",
        &format!("Preview restart failed: {last_err}"),
    )
    .await;
}

/// One `mise run dev` + health wait + attach log stream (existing deployment row).
async fn restart_dev_single_attempt(
    pool: &sqlx::PgPool,
    deployment_id: uuid::Uuid,
    spawn_job_id: uuid::Uuid,
    port: u16,
    script: &str,
    health_path: &str,
    preview_label: &str,
) -> Result<(), String> {
    let mut cmd = tokio::process::Command::new("bash");
    cmd.args(["-c", script])
        .env("MISE_YES", "1")
        .env("PORT", port.to_string())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    configure_unix_process_group(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;

    let health_url = format!("http://localhost:{port}{health_path}");
    tracing::info!(
        %deployment_id,
        %spawn_job_id,
        %port,
        %health_url,
        preview = %preview_label,
        "deploy restart: dev process spawned"
    );

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let mut stdout_reader = PreviewLogLines::new(stdout);
    let mut stderr_reader = PreviewLogLines::new(stderr);

    let mut log_buf = String::new();
    let mut dirty = false;
    let mut flush_timer = tokio::time::interval(LOG_FLUSH_INTERVAL);
    flush_timer.tick().await;

    let mut health_timer = tokio::time::interval(HEALTH_POLL_INTERVAL);
    health_timer.tick().await;
    let health_deadline = tokio::time::Instant::now() + HEALTH_TIMEOUT;
    let mut health_probes: u32 = 0;

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();

    loop {
        tokio::select! {
            seg = stdout_reader.next_segment() => {
                match seg {
                    Ok(Some(l)) if !l.is_empty() => {
                        if preview_log_line_smells_fatal(&l) {
                            tracing::warn!(
                                %deployment_id,
                                stream = "stdout",
                                preview = %preview_label,
                                line = %l,
                                "deploy restart: error-shaped stdout"
                            );
                        }
                        if log_buf.len() < MAX_LOG_BYTES {
                            log_buf.push_str(&l);
                            log_buf.push('\n');
                            dirty = true;
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            seg = stderr_reader.next_segment() => {
                match seg {
                    Ok(Some(l)) if !l.is_empty() => {
                        if preview_log_line_smells_fatal(&l) {
                            tracing::warn!(
                                %deployment_id,
                                stream = "stderr",
                                preview = %preview_label,
                                line = %l,
                                "deploy restart: error-shaped stderr"
                            );
                        }
                        if log_buf.len() < MAX_LOG_BYTES {
                            log_buf.push_str(&l);
                            log_buf.push('\n');
                            dirty = true;
                        }
                    }
                    Ok(Some(_)) => {}
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
                    let tail = preview_log_tail_for_error(&log_buf);
                    tracing::error!(
                        %deployment_id,
                        %health_url,
                        preview = %preview_label,
                        log_tail = %tail,
                        "deploy restart: health check timed out"
                    );
                    kill_dev_tree(&mut child).await;
                    return Err("health check timed out".into());
                }
                health_probes = health_probes.saturating_add(1);
                match http.get(&health_url).send().await {
                    Ok(resp) => {
                        let st = resp.status();
                        if st.is_success() {
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
                                kill_dev_tree(&mut child).await;
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
                        } else if health_probes == 1 || health_probes % 10 == 0 {
                            tracing::warn!(
                                %deployment_id,
                                %health_url,
                                http_status = %st,
                                probe = health_probes,
                                preview = %preview_label,
                                "deploy restart: health not 200 yet"
                            );
                        }
                    }
                    Err(e) => {
                        if health_probes == 1 || health_probes % 20 == 0 {
                            tracing::debug!(
                                %deployment_id,
                                %health_url,
                                probe = health_probes,
                                error = %e,
                                preview = %preview_label,
                                "deploy restart: health probe error"
                            );
                        }
                    }
                }
            }
        }
    }

    if dirty {
        flush_logs(pool, spawn_job_id, &log_buf).await;
    }

    let status = child.wait().await.map_err(|e| format!("wait: {e}"))?;
    let tail = preview_log_tail_for_error(&log_buf);
    tracing::error!(
        %deployment_id,
        preview = %preview_label,
        exit = %status,
        log_tail = %tail,
        "deploy restart: dev exited before healthy"
    );
    Err(format!("dev exited before healthy: {status}: …{tail}"))
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

/// Insert a queued OpenCode `build_jobs` row (`opencode_session_id` optional for session reuse).
async fn insert_opencode_build_job_row(
    pool: &sqlx::PgPool,
    oc_job_id: uuid::Uuid,
    project_id: uuid::Uuid,
    message_id: uuid::Uuid,
    prompt: &str,
    reuse_opencode_session_id: Option<&str>,
    model: &str,
) -> Result<(), sqlx::Error> {
    let deployment_id = active_deployment_id_for_project(pool, project_id).await;

    sqlx::query(
        "INSERT INTO build_jobs \
             (id, status, prompt_summary, model, tokens_used, error_message, \
              duration_ms, logs, deployment_id, project_id, message_id, \
              opencode_session_id, created_at, updated_at) \
             VALUES ($1, 'queued', $2, $7, 0, '', 0, '', $3, $4, $5, $6, NOW(), NOW())",
    )
    .bind(oc_job_id)
    .bind(prompt)
    .bind(deployment_id)
    .bind(project_id)
    .bind(message_id)
    .bind(reuse_opencode_session_id)
    .bind(model)
    .execute(pool)
    .await?;

    Ok(())
}

async fn invoke_run_build(
    pool: &sqlx::PgPool,
    build_job_id: uuid::Uuid,
) -> Result<crate::system_api::RunBuildOutput, crate::system_api::RunBuildError> {
    let input = crate::system_api::RunBuildInput { build_job_id };
    <super::AppSystems as crate::system_api::RunBuildSystem>::execute(
        &super::AppSystems,
        pool,
        input,
    )
    .await
}

/// True if an `AiProviderError` looks like a transient upstream hiccup
/// that is safe to retry with a fresh session. These are network-layer
/// failures from OpenCode's LLM backend — the agent never got to act on
/// anything, so replaying the prompt is idempotent in practice.
///
/// We keep the list of patterns narrow on purpose: a retry on a semantic
/// error (bad prompt, tool error, quota exceeded) would just burn time
/// and end in the same failure, so we'd rather surface it to the user.
fn is_transient_provider_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("network connection lost")
        || m.contains("provider_unavailable")
        || m.contains("provider unavailable")
        || m.contains("502")
        || m.contains("503")
        || m.contains("504")
        || m.contains("timeout")
        || m.contains("timed out")
        || m.contains("ecconnreset")
        || m.contains("connection reset")
        || m.contains("temporarily unavailable")
}

/// Reset a build job row to a pre-run state so `execute` can be retried
/// cleanly: fresh session (no reuse), no stale error_message, status back
/// to `running` so the UI keeps its spinner instead of flashing failed.
async fn reset_build_job_for_retry(
    pool: &sqlx::PgPool,
    build_job_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE build_jobs \
         SET status = 'running', \
             error_message = NULL, \
             opencode_session_id = NULL, \
             updated_at = NOW() \
         WHERE id = $1",
    )
    .bind(build_job_id)
    .execute(pool)
    .await?;
    Ok(())
}

fn provider_retry_max_attempts() -> u32 {
    // total attempts, not retries. default 3 = first try + 2 retries.
    // Each retry costs one prompt replay (~30–180 s), so keep it tight.
    std::env::var("OPENCODE_PROVIDER_RETRIES")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .map(|v| v.clamp(1, 5))
        .unwrap_or(3)
}

/// Queue one OpenCode build job and run it to completion (blocks).
async fn run_one_opencode_build(
    pool: &sqlx::PgPool,
    project_id: uuid::Uuid,
    message_id: uuid::Uuid,
    prompt: &str,
    model: &str,
) -> Result<crate::system_api::RunBuildOutput, String> {
    if has_active_opencode_job(pool, project_id)
        .await
        .map_err(|e| e.to_string())?
    {
        if model == "opencode-repair" {
            tracing::info!(
                %project_id,
                "skip OpenCode repair job — another opencode job already queued/running"
            );
            return Ok(crate::system_api::RunBuildOutput {
                artifacts_count: 0,
                tokens_used: 0,
                status: "skipped".to_string(),
            });
        }
        return Err(
            "A build is already in progress for this project — wait for it to finish.".into(),
        );
    }

    let oc_job_id = uuid::Uuid::new_v4();

    insert_opencode_build_job_row(pool, oc_job_id, project_id, message_id, prompt, None, model)
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!(%project_id, %oc_job_id, %model, "OpenCode build job queued");

    // Bounded retry on transient upstream provider errors (e.g. OpenCode's
    // LLM backend returning 502 "Network connection lost"). These fail the
    // build before any diffs are produced, so replaying the prompt is
    // effectively idempotent — and it hides a very common source of
    // spurious user-facing failures behind ~a few seconds of extra wait.
    let max_attempts = provider_retry_max_attempts();
    let mut attempt: u32 = 1;
    loop {
        match invoke_run_build(pool, oc_job_id).await {
            Ok(output) => return Ok(output),
            Err(e) => {
                let err_str = format!("{e:?}");
                let is_provider_error =
                    matches!(&e, crate::system_api::RunBuildError::AiProviderError(_));
                let transient = is_provider_error && is_transient_provider_error(&err_str);

                if transient && attempt < max_attempts {
                    // Exponential-ish backoff: 3s, 7s, 15s, ...
                    let backoff_secs = 3u64 * (1u64 << (attempt - 1).min(4));
                    tracing::warn!(
                        %project_id,
                        %oc_job_id,
                        attempt,
                        max_attempts,
                        backoff_secs,
                        error = %err_str,
                        "transient OpenCode provider error — resetting build row and retrying"
                    );

                    publish_deploy_status(
                        project_id,
                        oc_job_id,
                        "provider_retry",
                        &format!(
                            "AI provider hiccup — retrying (attempt {next}/{max})…",
                            next = attempt + 1,
                            max = max_attempts,
                        ),
                    )
                    .await;

                    if let Err(reset_err) = reset_build_job_for_retry(pool, oc_job_id).await {
                        tracing::warn!(
                            %oc_job_id,
                            error = %reset_err,
                            "failed to reset build_jobs row before retry — continuing anyway"
                        );
                    }

                    tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                    attempt += 1;
                    continue;
                }

                // Non-retryable, or retries exhausted. Mark failed and surface.
                let _ = sqlx::query(
                    "UPDATE build_jobs SET status = 'failed', error_message = $2, \
                     updated_at = NOW() WHERE id = $1",
                )
                .bind(oc_job_id)
                .bind(&err_str)
                .execute(pool)
                .await;

                if transient {
                    tracing::error!(
                        %project_id,
                        %oc_job_id,
                        attempt,
                        max_attempts,
                        error = %err_str,
                        "OpenCode provider retries exhausted — giving up"
                    );
                }
                return Err(err_str);
            }
        }
    }
}

async fn run_opencode_repair_pass(
    pool: &sqlx::PgPool,
    project_id: uuid::Uuid,
    message_id: uuid::Uuid,
    round: u32,
) -> Result<(), String> {
    let scope_raw = fetch_project_scope_raw(pool, project_id).await?;
    let body = dev_server_repair_body(scope_raw.as_str());
    let prompt = format!(
        "{body}\n\n(Automated repair round {round} — preview server did not become healthy.)"
    );
    run_one_opencode_build(pool, project_id, message_id, &prompt, "opencode-repair").await?;
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
    if let Ok(true) = has_active_opencode_job(pool, project_id).await {
        tracing::info!(
            %project_id,
            "skip post-spawn OpenCode job — another opencode job already queued/running"
        );
        return;
    }

    match run_one_opencode_build(pool, project_id, message_id, prompt, "opencode").await {
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
