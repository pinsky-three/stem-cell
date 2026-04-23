//! Plan the next experiment branch for a project.
//!
//! The branch name is deterministic (`exp/<slug>/<utc-ts>`) so downstream
//! systems (push + PR) can reconstruct it without extra state. We don't
//! create the ref on GitHub here — the ref comes into existence on first
//! push, which keeps this system cheap and failure-proof in the happy path.
//! When installation-scoped ref creation becomes necessary (e.g. opening a
//! PR with zero commits), the call slot lives at the bottom of this file.
use super::github_common::{
    generate_experiment_branch_name, load_active_repo_context, LoadRepoContextError,
};
use crate::system_api::*;

#[async_trait::async_trait]
impl StartExperimentBranchSystem for super::AppSystems {
    async fn execute(
        &self,
        pool: &sqlx::PgPool,
        input: StartExperimentBranchInput,
    ) -> Result<StartExperimentBranchOutput, StartExperimentBranchError> {
        let ctx = load_active_repo_context(pool, input.project_id)
            .await
            .map_err(map_err)?;

        let branch_name = generate_experiment_branch_name(&ctx.project_slug);
        let base_sha = input.base_sha.clone().unwrap_or_default();

        tracing::info!(
            project_id = %ctx.project_id,
            owner = %ctx.owner,
            repo = %ctx.repo,
            base_branch = %ctx.default_branch,
            branch = %branch_name,
            "experiment branch planned (ref will materialise on first push)"
        );

        Ok(StartExperimentBranchOutput {
            branch_name,
            base_branch: ctx.default_branch,
            base_sha,
            status: "planned".to_string(),
        })
    }
}

fn map_err(e: LoadRepoContextError) -> StartExperimentBranchError {
    match e {
        LoadRepoContextError::ProjectNotFound => StartExperimentBranchError::ProjectNotFound,
        LoadRepoContextError::RepoNotConnected => StartExperimentBranchError::RepoNotConnected,
        LoadRepoContextError::InstallationInactive => {
            StartExperimentBranchError::InstallationInactive
        }
        LoadRepoContextError::Database(msg) => StartExperimentBranchError::Internal(msg),
    }
}
