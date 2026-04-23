//! Open a PR from the experiment branch to the repo's default branch.
//!
//! Contract boundary: the GitHub REST call
//! `POST /repos/{owner}/{repo}/pulls` needs an `Authorization: token
//! <installation-access-token>` header, and minting that token requires a
//! signed GitHub App JWT (the `jsonwebtoken` crate) — see the twin note in
//! `push_project_changes_to_repo.rs`. We validate the full context
//! (installation, repo connection, branch shape) and return a structured
//! `GithubApiError` when the signer wiring is absent.
//!
//! The next minimum scope expansion to make this real:
//!   1. Add `jsonwebtoken` to `crates/runtime/Cargo.toml`.
//!   2. Mint an installation token using the App's private key.
//!   3. `reqwest::post(format!("https://api.github.com/repos/{owner}/{repo}/pulls"))`
//!      with JSON body `{ title, body, head: branch_name, base: default_branch }`.
use super::github_common::{
    github_app_credentials_present, load_active_repo_context, LoadRepoContextError,
};
use crate::system_api::*;

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

        tracing::info!(
            project_id = %ctx.project_id,
            owner = %ctx.owner,
            repo = %ctx.repo,
            head = %input.branch_name,
            base = %ctx.default_branch,
            title = %input.title,
            creds_present = github_app_credentials_present(),
            "open_experiment_pull_request: hitting contract boundary"
        );

        let diagnostic = if github_app_credentials_present() {
            "pull-request creation not wired: GITHUB_APP_ID / GITHUB_APP_PRIVATE_KEY \
             are present, but the JWT signer (jsonwebtoken crate) required to \
             mint an installation access token is outside the editable surface \
             (Cargo.toml is forbidden by AGENTS.md)."
        } else {
            "pull-request creation not wired: GITHUB_APP_ID / GITHUB_APP_PRIVATE_KEY \
             are not configured, and the JWT signer (jsonwebtoken crate) is \
             outside the editable surface. Configure the app + wire the \
             signer in a follow-up scope expansion."
        };

        Err(OpenExperimentPullRequestError::GithubApiError(
            diagnostic.into(),
        ))
    }
}

fn map_err(e: LoadRepoContextError) -> OpenExperimentPullRequestError {
    match e {
        LoadRepoContextError::ProjectNotFound => OpenExperimentPullRequestError::ProjectNotFound,
        LoadRepoContextError::RepoNotConnected => {
            OpenExperimentPullRequestError::RepoNotConnected
        }
        LoadRepoContextError::InstallationInactive => {
            OpenExperimentPullRequestError::InstallationInactive
        }
        LoadRepoContextError::Database(msg) => OpenExperimentPullRequestError::Internal(msg),
    }
}
