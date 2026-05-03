use crate::system_api::*;
use std::time::Duration;
use stem_sandbox::{SandboxId, SandboxRoot};
use tracing::Instrument;

const DEFAULT_MAX_AGE_MINUTES: i32 = 60;
const PERIODIC_INTERVAL: Duration = Duration::from_secs(15 * 60);

#[derive(sqlx::FromRow)]
struct DeploymentRow {
    id: uuid::Uuid,
    build_job_id: uuid::Uuid,
    pid: Option<i32>,
}

// ── API endpoint (contract system trait) ────────────────────────────────

#[async_trait::async_trait]
impl CleanupDeploymentsSystem for super::AppSystems {
    async fn execute(
        &self,
        pool: &sqlx::PgPool,
        input: CleanupDeploymentsInput,
    ) -> Result<CleanupDeploymentsOutput, CleanupDeploymentsError> {
        // See run_build.rs for the rationale — `.instrument(span).await`
        // rather than `span.enter()` avoids the cross-await soundness issues
        // that panic tracing-subscriber's registry.
        let span = tracing::info_span!("cleanup_deployments");
        async move {
            let deployments = fetch_targets(pool, &input).await?;

            if deployments.is_empty() {
                if input.deployment_id.is_some() {
                    return Err(CleanupDeploymentsError::DeploymentNotFound);
                }
                return Ok(CleanupDeploymentsOutput {
                    cleaned_count: 0,
                    errors: String::new(),
                    status: "nothing to clean".into(),
                });
            }

            let total = deployments.len();
            let (cleaned, errors) = cleanup_batch(pool, &deployments).await;

            let status = if errors.is_empty() {
                format!("cleaned {cleaned}/{total}")
            } else {
                format!("cleaned {cleaned}/{total}, {} errors", errors.len())
            };

            tracing::info!(%status, "cleanup finished");

            Ok(CleanupDeploymentsOutput {
                cleaned_count: cleaned,
                errors: errors.join("; "),
                status,
            })
        }
        .instrument(span)
        .await
    }
}

// ── Startup reconciliation ──────────────────────────────────────────────

/// Runs once at boot to reconcile the `deployments` table with reality.
///
/// Why this exists: when the stem-cell process exits (crash, `Ctrl+C`,
/// deploy), its child dev servers die with it, but nothing updates the
/// DB — rows stay at `active=true, status='running'` pointing at ports
/// that no longer have anything listening. The proxy then cheerfully
/// forwards traffic there forever, and the iframe's auto-refresh error
/// page piles ~1 req/s of 502s into the logs (see the log flood that
/// motivated this fix).
///
/// The sweep TCP-probes each active deployment's port. If the port is
/// refusing connections, the deployment is marked `stopped` and its
/// `build_job` is cleared out of any zombie 'running' state. No
/// processes are killed (they're already dead); no filesystem work;
/// just DB hygiene so the proxy can return a proper `410 Gone` instead
/// of hitting a dead port.
///
/// Best-effort: any error during the sweep is logged but doesn't fail
/// startup — a partial sweep is strictly better than no sweep.
pub async fn sweep_stale_deployments_on_startup(pool: &sqlx::PgPool) {
    #[derive(sqlx::FromRow)]
    struct StartupRow {
        id: uuid::Uuid,
        build_job_id: uuid::Uuid,
        port: i32,
        pid: Option<i32>,
    }

    let rows: Vec<StartupRow> = match sqlx::query_as::<_, StartupRow>(
        "SELECT id, build_job_id, port, pid FROM deployments \
         WHERE active = true AND deleted_at IS NULL \
           AND status IN ('running', 'starting')",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rs) => rs,
        Err(e) => {
            tracing::warn!(error = %e, "startup sweep: query failed — skipping");
            return;
        }
    };

    if rows.is_empty() {
        tracing::info!("startup sweep: no active deployments to reconcile");
        return;
    }

    let mut alive = 0u32;
    let mut marked_stopped = 0u32;

    for row in &rows {
        let port: u16 = match row.port.try_into() {
            Ok(p) => p,
            Err(_) => {
                tracing::warn!(deployment_id = %row.id, port = row.port, "invalid port — marking stopped");
                mark_stopped_no_kill(pool, row.id, row.build_job_id).await;
                marked_stopped += 1;
                continue;
            }
        };

        // Short probe: 500 ms is plenty on localhost and keeps the sweep
        // from stalling boot if a node is genuinely slow.
        let probe = tokio::time::timeout(
            Duration::from_millis(500),
            tokio::net::TcpStream::connect(("127.0.0.1", port)),
        )
        .await;

        match probe {
            Ok(Ok(_stream)) => {
                alive += 1;
                tracing::info!(
                    deployment_id = %row.id,
                    port,
                    pid = ?row.pid,
                    "startup sweep: deployment still alive"
                );
            }
            Ok(Err(e)) => {
                tracing::info!(
                    deployment_id = %row.id,
                    port,
                    pid = ?row.pid,
                    error = %e,
                    "startup sweep: port not accepting connections — marking stopped"
                );
                mark_stopped_no_kill(pool, row.id, row.build_job_id).await;
                marked_stopped += 1;
            }
            Err(_) => {
                tracing::info!(
                    deployment_id = %row.id,
                    port,
                    pid = ?row.pid,
                    "startup sweep: probe timeout — marking stopped"
                );
                mark_stopped_no_kill(pool, row.id, row.build_job_id).await;
                marked_stopped += 1;
            }
        }
    }

    tracing::info!(
        total = rows.len(),
        alive,
        marked_stopped,
        "startup sweep complete"
    );
}

/// DB-only counterpart to `cleanup_one`: assumes the process is already
/// gone (verified by the caller via TCP probe), so we just flip the rows
/// to `stopped`/`active=false` and leave the work dir on disk for
/// potential revival.
async fn mark_stopped_no_kill(
    pool: &sqlx::PgPool,
    deployment_id: uuid::Uuid,
    build_job_id: uuid::Uuid,
) {
    if let Err(e) = sqlx::query(
        "UPDATE deployments \
         SET status = 'stopped', active = false, pid = NULL, updated_at = NOW() \
         WHERE id = $1",
    )
    .bind(deployment_id)
    .execute(pool)
    .await
    {
        tracing::warn!(%deployment_id, error = %e, "startup sweep: update deployment failed");
    }

    if let Err(e) = sqlx::query(
        "UPDATE build_jobs SET status = 'stopped', updated_at = NOW() \
         WHERE id = $1 AND status IN ('running', 'succeeded')",
    )
    .bind(build_job_id)
    .execute(pool)
    .await
    {
        tracing::warn!(%build_job_id, error = %e, "startup sweep: update build_job failed");
    }
}

// ── Periodic background loop ────────────────────────────────────────────

static CLEANUP_LOOP: std::sync::OnceLock<()> = std::sync::OnceLock::new();

/// Start the periodic cleanup loop (idempotent — only spawns once per process).
pub(super) fn ensure_periodic_cleanup(pool: sqlx::PgPool) {
    CLEANUP_LOOP.get_or_init(|| {
        tokio::spawn(periodic_loop(pool));
        tracing::info!(
            interval_secs = PERIODIC_INTERVAL.as_secs(),
            max_age_minutes = DEFAULT_MAX_AGE_MINUTES,
            "periodic deployment cleanup activated",
        );
    });
}

async fn periodic_loop(pool: sqlx::PgPool) {
    let mut ticker = tokio::time::interval(PERIODIC_INTERVAL);
    ticker.tick().await; // skip the immediate first tick

    loop {
        ticker.tick().await;

        let stale = match fetch_stale(&pool, DEFAULT_MAX_AGE_MINUTES).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(error = %e, "periodic cleanup: failed to query stale deployments");
                continue;
            }
        };

        if stale.is_empty() {
            tracing::debug!("periodic cleanup: nothing to clean");
            continue;
        }

        let total = stale.len();
        let (cleaned, errors) = cleanup_batch(&pool, &stale).await;

        if errors.is_empty() {
            tracing::info!(cleaned, total, "periodic cleanup complete");
        } else {
            tracing::warn!(
                cleaned,
                total,
                error_count = errors.len(),
                "periodic cleanup finished with errors"
            );
        }
    }
}

// ── Shared cleanup core ─────────────────────────────────────────────────

async fn cleanup_batch(pool: &sqlx::PgPool, deployments: &[DeploymentRow]) -> (i32, Vec<String>) {
    let mut cleaned = 0i32;
    let mut errors = Vec::new();

    for dep in deployments {
        match cleanup_one(pool, dep).await {
            Ok(()) => cleaned += 1,
            Err(e) => {
                tracing::warn!(deployment_id = %dep.id, error = %e, "cleanup failed");
                errors.push(format!("{}: {e}", dep.id));
            }
        }
    }

    (cleaned, errors)
}

async fn cleanup_one(pool: &sqlx::PgPool, dep: &DeploymentRow) -> Result<(), String> {
    tracing::info!(deployment_id = %dep.id, pid = ?dep.pid, "cleaning deployment");

    if let Some(pid) = dep.pid {
        kill_process(pid).await;
    }

    let root = SandboxRoot::temp_default();
    let work_dir = root.work_dir(&SandboxId::from_uuid(dep.build_job_id));
    remove_work_dir(&root, &work_dir).await;

    sqlx::query(
        "UPDATE deployments SET status = 'cleaned', active = false, updated_at = NOW() \
         WHERE id = $1",
    )
    .bind(dep.id)
    .execute(pool)
    .await
    .map_err(|e| format!("update deployment: {e}"))?;

    sqlx::query(
        "UPDATE build_jobs SET status = 'cleaned', updated_at = NOW() \
         WHERE id = $1 AND status IN ('running', 'succeeded')",
    )
    .bind(dep.build_job_id)
    .execute(pool)
    .await
    .map_err(|e| format!("update build_job: {e}"))?;

    tracing::info!(deployment_id = %dep.id, "deployment cleaned");
    Ok(())
}

// ── Queries ─────────────────────────────────────────────────────────────

async fn fetch_targets(
    pool: &sqlx::PgPool,
    input: &CleanupDeploymentsInput,
) -> Result<Vec<DeploymentRow>, CleanupDeploymentsError> {
    if let Some(dep_id) = input.deployment_id {
        let row = sqlx::query_as::<_, DeploymentRow>(
            "SELECT id, build_job_id, pid FROM deployments \
             WHERE id = $1 AND active = true AND deleted_at IS NULL",
        )
        .bind(dep_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| CleanupDeploymentsError::DatabaseError(e.to_string()))?;

        match row {
            Some(r) => Ok(vec![r]),
            None => Err(CleanupDeploymentsError::DeploymentNotFound),
        }
    } else {
        let max_age = input.max_age_minutes.unwrap_or(DEFAULT_MAX_AGE_MINUTES);
        fetch_stale(pool, max_age)
            .await
            .map_err(|e| CleanupDeploymentsError::DatabaseError(e))
    }
}

async fn fetch_stale(
    pool: &sqlx::PgPool,
    max_age_minutes: i32,
) -> Result<Vec<DeploymentRow>, String> {
    sqlx::query_as::<_, DeploymentRow>(
        "SELECT id, build_job_id, pid FROM deployments \
         WHERE active = true AND deleted_at IS NULL \
           AND created_at < NOW() - ($1 || ' minutes')::interval \
         ORDER BY created_at ASC",
    )
    .bind(max_age_minutes.to_string())
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())
}

// ── Process management ──────────────────────────────────────────────────

/// Best-effort SIGTERM then SIGKILL (process group + direct). Used by cleanup and deploy restart.
pub(crate) async fn kill_process(pid: i32) {
    stem_sandbox::kill_process_tree(pid).await;
    tracing::debug!(pid, "kill sequence complete");
}

async fn remove_work_dir(root: &SandboxRoot, path: &std::path::Path) {
    match stem_sandbox::remove_sandbox_dir(root, path).await {
        Ok(()) => tracing::info!(path = %path.display(), "removed work directory"),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "failed to remove work directory");
        }
    }
}
