//! Shared helpers for the GitHub App lifecycle systems.
//!
//! These systems are intentionally designed around GitHub **App installations**
//! (not long-lived OAuth repo tokens). Each project may have at most one active
//! `RepoConnection`; all branch / push / PR operations mint short-lived
//! installation access tokens at call time — see the `contract boundary` notes
//! inside the individual system impls for what's wired here vs what requires
//! a scope expansion beyond `crates/runtime/src/systems/*.rs`.

use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Whether GitHub App credentials are present in the environment.
///
/// This is purely diagnostic: live refresh, push, and PR creation all need
/// a GitHub App JWT signer (the `jsonwebtoken` crate) to mint installation
/// access tokens. That dependency lives in `Cargo.toml`, which is outside
/// the editable surface in `AGENTS.md`. We expose this helper so system
/// implementations can return precise error messages instead of generic
/// failures when the wiring is incomplete.
pub(super) fn github_app_credentials_present() -> bool {
    std::env::var("GITHUB_APP_ID").ok().is_some_and(|v| !v.is_empty())
        && std::env::var("GITHUB_APP_PRIVATE_KEY")
            .ok()
            .is_some_and(|v| !v.is_empty())
}

/// Snapshot used by systems that operate on an active repo connection
/// (`StartExperimentBranch`, `PushProjectChangesToRepo`,
/// `OpenExperimentPullRequest`).
#[derive(Debug, Clone)]
pub(super) struct ActiveRepoContext {
    pub project_id: Uuid,
    pub project_slug: String,
    #[allow(dead_code)]
    pub repo_connection_id: Uuid,
    pub owner: String,
    pub repo: String,
    pub default_branch: String,
    #[allow(dead_code)]
    pub installation_uuid: Uuid,
    #[allow(dead_code)]
    pub installation_id_remote: i64,
}

/// Typed errors returned by [`load_active_repo_context`]. System impls map
/// these onto their own generated error enums.
#[derive(Debug)]
pub(super) enum LoadRepoContextError {
    ProjectNotFound,
    RepoNotConnected,
    InstallationInactive,
    Database(String),
}

/// Loads the single active repo connection for a project, joined with its
/// GitHub App installation. Enforces the installation-is-active invariant.
pub(super) async fn load_active_repo_context(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<ActiveRepoContext, LoadRepoContextError> {
    let project: Option<(Uuid, String, bool)> = sqlx::query_as(
        "SELECT id, slug, active FROM projects \
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| LoadRepoContextError::Database(e.to_string()))?;

    let (project_id, project_slug, project_active) = match project {
        Some(p) => p,
        None => return Err(LoadRepoContextError::ProjectNotFound),
    };
    if !project_active {
        return Err(LoadRepoContextError::ProjectNotFound);
    }

    let row = sqlx::query(
        "SELECT rc.id              AS rc_id,
                rc.owner           AS rc_owner,
                rc.repo            AS rc_repo,
                rc.default_branch  AS rc_default_branch,
                gi.id              AS gi_id,
                gi.installation_id AS gi_installation_id,
                gi.active          AS gi_active,
                gi.status          AS gi_status
           FROM repo_connections rc
           JOIN github_installations gi ON gi.id = rc.installation_id
          WHERE rc.project_id = $1
            AND rc.active = true
            AND rc.deleted_at IS NULL
            AND gi.deleted_at IS NULL
          ORDER BY rc.updated_at DESC
          LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| LoadRepoContextError::Database(e.to_string()))?;

    let row = row.ok_or(LoadRepoContextError::RepoNotConnected)?;

    let installation_active: bool = row.get("gi_active");
    let installation_status: String = row.get("gi_status");
    if !installation_active || installation_status != "active" {
        return Err(LoadRepoContextError::InstallationInactive);
    }

    Ok(ActiveRepoContext {
        project_id,
        project_slug,
        repo_connection_id: row.get("rc_id"),
        owner: row.get("rc_owner"),
        repo: row.get("rc_repo"),
        default_branch: row.get("rc_default_branch"),
        installation_uuid: row.get("gi_id"),
        installation_id_remote: row.get("gi_installation_id"),
    })
}

/// Deterministic experiment branch name: `exp/<project-slug>/<utc-unix-ts>`.
///
/// Keeping this in one place ensures every system that derives or validates
/// an experiment branch name agrees on the convention.
pub(super) fn generate_experiment_branch_name(project_slug: &str) -> String {
    let ts = chrono::Utc::now().timestamp();
    format!("exp/{project_slug}/{ts}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_name_uses_slug_and_timestamp_prefix() {
        let name = generate_experiment_branch_name("my-project");
        assert!(name.starts_with("exp/my-project/"));
        // Suffix is a unix timestamp — at minimum 10 digits until ~2286.
        let suffix = name.rsplit('/').next().unwrap();
        assert!(
            suffix.chars().all(|c| c.is_ascii_digit()),
            "suffix must be a unix timestamp"
        );
        assert!(suffix.len() >= 10);
    }
}
