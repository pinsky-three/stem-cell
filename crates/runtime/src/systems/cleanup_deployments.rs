use crate::system_api::*;
use std::time::Duration;

const DEFAULT_MAX_AGE_MINUTES: i32 = 60;
const KILL_GRACE: Duration = Duration::from_secs(5);
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
        let span = tracing::info_span!("cleanup_deployments");
        let _enter = span.enter();

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
            tracing::warn!(cleaned, total, error_count = errors.len(), "periodic cleanup finished with errors");
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

    let work_dir = format!("/tmp/stem-cell-{}", dep.build_job_id);
    remove_work_dir(&work_dir).await;

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

async fn fetch_stale(pool: &sqlx::PgPool, max_age_minutes: i32) -> Result<Vec<DeploymentRow>, String> {
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
    #[cfg(unix)]
    {
        use std::process::Command;

        let pgid_kill = Command::new("kill")
            .args(["-TERM", &format!("-{pid}")])
            .output();
        let direct_kill = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output();

        if pgid_kill.is_err() && direct_kill.is_err() {
            tracing::debug!(pid, "SIGTERM failed — process may already be gone");
            return;
        }

        tokio::time::sleep(KILL_GRACE).await;

        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .output();
        let _ = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .output();

        tracing::debug!(pid, "kill sequence complete");
    }

    #[cfg(not(unix))]
    {
        tracing::warn!(pid, "process kill not supported on this platform");
    }
}

async fn remove_work_dir(path: &str) {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => tracing::info!(%path, "removed work directory"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(%path, "work directory already gone");
        }
        Err(e) => {
            tracing::warn!(%path, error = %e, "failed to remove work directory");
        }
    }
}
