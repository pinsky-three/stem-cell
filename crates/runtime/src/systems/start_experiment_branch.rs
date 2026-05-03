//! Create a deterministic experiment branch on GitHub.
//!
//! Flow (all calls scoped to an installation access token):
//!   1. Resolve the repo-connection context (already active, not revoked).
//!   2. Get the head SHA of the base branch (default branch, unless
//!      `input.base_sha` is supplied).
//!   3. Create `refs/heads/exp/<slug>/<ts>` pointed at that SHA.
//!
//! If GitHub responds 422 "Reference already exists" we treat that as
//! success — the branch is simply already there. Any other failure
//! propagates as `GithubApiError`.
//!
//! When the App isn't configured, we degrade gracefully: plan the branch
//! name but mark the status `planned` so callers know the ref hasn't been
//! created yet. Set `GITHUB_APP_REQUIRE_LIVE_BRANCH=1` to force an error in
//! that degraded path.
use super::github_common::{
    LoadRepoContextError, generate_experiment_branch_name, load_active_repo_context,
};
use crate::system_api::*;
use stem_git::github::{
    CreateBranchStatus, InstallationClient, config, create_branch, resolve_head_sha,
};

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

        let Some(_cfg) = config() else {
            let strict = matches!(
                std::env::var("GITHUB_APP_REQUIRE_LIVE_BRANCH").as_deref(),
                Ok("1") | Ok("true")
            );
            if strict {
                return Err(StartExperimentBranchError::GithubApiError(
                    "GitHub App not configured".into(),
                ));
            }
            tracing::warn!(
                project_id = %ctx.project_id,
                branch = %branch_name,
                "App not configured; branch name planned but not created on GitHub"
            );
            return Ok(StartExperimentBranchOutput {
                branch_name,
                base_branch: ctx.default_branch,
                base_sha: input.base_sha.unwrap_or_default(),
                status: "planned".into(),
            });
        };

        let client = InstallationClient::for_installation(ctx.installation_id_remote)
            .await
            .map_err(|e| StartExperimentBranchError::GithubApiError(e.to_string()))?;

        // Resolve the base SHA. We always fetch the remote ref (even if the
        // caller supplied `base_sha`) when the caller didn't supply one, so
        // the branch can never point at a stale commit for this process.
        let base_sha = match input.base_sha.clone().filter(|s| !s.is_empty()) {
            Some(s) => s,
            None => resolve_head_sha(&client, &ctx.owner, &ctx.repo, &ctx.default_branch)
                .await
                .map_err(|e| StartExperimentBranchError::GithubApiError(e.to_string()))?,
        };

        let status = create_branch(&client, &ctx.owner, &ctx.repo, &branch_name, &base_sha)
            .await
            .map_err(|e| StartExperimentBranchError::GithubApiError(e.to_string()))?;

        if status == CreateBranchStatus::Created {
            tracing::info!(
                project_id = %ctx.project_id,
                owner = %ctx.owner,
                repo = %ctx.repo,
                base = %ctx.default_branch,
                %base_sha,
                branch = %branch_name,
                "experiment branch created"
            );
        }

        Ok(StartExperimentBranchOutput {
            branch_name,
            base_branch: ctx.default_branch,
            base_sha,
            status: match status {
                CreateBranchStatus::Created => "created".into(),
                CreateBranchStatus::AlreadyExists => "already_exists".into(),
            },
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
