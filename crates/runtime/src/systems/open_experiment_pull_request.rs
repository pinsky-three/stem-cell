//! Open a PR from the experiment branch to the repo's default branch.
//!
//! Calls `POST /repos/{owner}/{repo}/pulls` via an installation access
//! token. On 422 "A pull request already exists" we look up the existing PR
//! and return it — the system is idempotent so a retry after a partial
//! failure doesn't create duplicates.
use super::github_common::{LoadRepoContextError, load_active_repo_context};
use crate::system_api::*;
use stem_git::github::{InstallationClient, PullRequestRequest, config, open_pull_request};

#[async_trait::async_trait]
impl OpenExperimentPullRequestSystem for super::AppSystems {
    async fn execute(
        &self,
        pool: &sqlx::PgPool,
        input: OpenExperimentPullRequestInput,
    ) -> Result<OpenExperimentPullRequestOutput, OpenExperimentPullRequestError> {
        let ctx = load_active_repo_context(pool, input.project_id)
            .await
            .map_err(map_err)?;

        if input.branch_name.trim().is_empty() {
            return Err(OpenExperimentPullRequestError::PullRequestFailed(
                "branch_name must not be empty".into(),
            ));
        }
        if input.branch_name == ctx.default_branch {
            return Err(OpenExperimentPullRequestError::PullRequestFailed(
                "head branch must differ from the default (base) branch".into(),
            ));
        }
        if input.title.trim().is_empty() {
            return Err(OpenExperimentPullRequestError::PullRequestFailed(
                "title must not be empty".into(),
            ));
        }

        if config().is_none() {
            return Err(OpenExperimentPullRequestError::GithubApiError(
                "GitHub App not configured (GITHUB_APP_ID / \
                 GITHUB_APP_PRIVATE_KEY[_PATH] / GITHUB_APP_WEBHOOK_SECRET)"
                    .into(),
            ));
        }

        let client = InstallationClient::for_installation(ctx.installation_id_remote)
            .await
            .map_err(|e| OpenExperimentPullRequestError::GithubApiError(e.to_string()))?;

        let pr = open_pull_request(
            &client,
            PullRequestRequest {
                owner: ctx.owner.clone(),
                repo: ctx.repo.clone(),
                head_branch: input.branch_name.clone(),
                base_branch: ctx.default_branch.clone(),
                title: input.title,
                body: input.body,
            },
        )
        .await
        .map_err(|e| OpenExperimentPullRequestError::GithubApiError(e.to_string()))?;

        if !pr.already_exists {
            tracing::info!(
                project_id = %ctx.project_id,
                owner = %ctx.owner,
                repo = %ctx.repo,
                pr_number = pr.number,
                pr_url = %pr.html_url,
                "pull request opened"
            );
        }

        Ok(OpenExperimentPullRequestOutput {
            pr_number: pr.number,
            pr_url: pr.html_url,
            status: if pr.already_exists {
                "already_exists".into()
            } else {
                "opened".into()
            },
        })
    }
}

fn map_err(e: LoadRepoContextError) -> OpenExperimentPullRequestError {
    match e {
        LoadRepoContextError::ProjectNotFound => OpenExperimentPullRequestError::ProjectNotFound,
        LoadRepoContextError::RepoNotConnected => OpenExperimentPullRequestError::RepoNotConnected,
        LoadRepoContextError::InstallationInactive => {
            OpenExperimentPullRequestError::InstallationInactive
        }
        LoadRepoContextError::Database(msg) => OpenExperimentPullRequestError::Internal(msg),
    }
}
