//! Open a PR from the experiment branch to the repo's default branch.
//!
//! Calls `POST /repos/{owner}/{repo}/pulls` via an installation access
//! token. On 422 "A pull request already exists" we look up the existing PR
//! and return it — the system is idempotent so a retry after a partial
//! failure doesn't create duplicates.
use super::github_common::{
    load_active_repo_context, LoadRepoContextError,
};
use crate::github_app::{InstallationClient, config};
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

        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls",
            ctx.owner, ctx.repo
        );
        let mut body = serde_json::json!({
            "title": input.title,
            "head":  input.branch_name,
            "base":  ctx.default_branch,
        });
        if let Some(b) = input.body.as_deref().filter(|s| !s.is_empty()) {
            body["body"] = serde_json::Value::String(b.into());
        }

        let res = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| OpenExperimentPullRequestError::GithubApiError(e.to_string()))?;
        let status = res.status();
        let status_u16 = status.as_u16();
        let response_body: serde_json::Value = res
            .json()
            .await
            .map_err(|e| OpenExperimentPullRequestError::GithubApiError(e.to_string()))?;

        if status.is_success() {
            let number = response_body["number"].as_i64().ok_or_else(|| {
                OpenExperimentPullRequestError::GithubApiError(
                    "POST /pulls response missing `number`".into(),
                )
            })?;
            let url_ = response_body["html_url"]
                .as_str()
                .unwrap_or("")
                .to_string();
            tracing::info!(
                project_id = %ctx.project_id,
                owner = %ctx.owner,
                repo = %ctx.repo,
                pr_number = number,
                pr_url = %url_,
                "pull request opened"
            );
            return Ok(OpenExperimentPullRequestOutput {
                pr_number: number as i32,
                pr_url: url_,
                status: "opened".into(),
            });
        }

        // Idempotent path: 422 "A pull request already exists" → look up the
        // open PR for (head -> base) and return it.
        if status_u16 == 422
            && response_body
                .get("errors")
                .and_then(|e| e.as_array())
                .is_some_and(|arr| {
                    arr.iter()
                        .any(|e| e["message"].as_str().unwrap_or("").contains("already exists"))
                })
        {
            let list_url = format!(
                "https://api.github.com/repos/{}/{}/pulls?state=open&head={}:{}&base={}",
                ctx.owner,
                ctx.repo,
                ctx.owner,
                input.branch_name,
                ctx.default_branch
            );
            let existing = client
                .get(&list_url)
                .send()
                .await
                .map_err(|e| OpenExperimentPullRequestError::GithubApiError(e.to_string()))?
                .json::<Vec<serde_json::Value>>()
                .await
                .map_err(|e| OpenExperimentPullRequestError::GithubApiError(e.to_string()))?;
            if let Some(pr) = existing.first() {
                if let (Some(n), Some(u)) = (pr["number"].as_i64(), pr["html_url"].as_str()) {
                    return Ok(OpenExperimentPullRequestOutput {
                        pr_number: n as i32,
                        pr_url: u.to_string(),
                        status: "already_exists".into(),
                    });
                }
            }
        }

        Err(OpenExperimentPullRequestError::GithubApiError(format!(
            "POST /repos/{}/{}/pulls: {} {}",
            ctx.owner,
            ctx.repo,
            status,
            response_body
        )))
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
