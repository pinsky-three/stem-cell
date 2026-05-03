//! Push a local checkout to the connected repo on a given branch.
//!
//! Uses `git` as a subprocess and authenticates with a short-lived
//! installation access token embedded in the push URL
//! (`https://x-access-token:<tok>@github.com/owner/repo.git`). The token is
//! minted fresh per call and never persisted.
//!
//! Behaviour:
//!   * `git add -A && git commit -m <msg>` (allow-empty so no-op pushes
//!     don't fail; we detect "nothing to push" after the push itself).
//!   * `git push <url> HEAD:<branch>` — with `--force-with-lease` when
//!     `input.force` is true (template seeding to an existing branch).
//!   * `git rev-parse HEAD` to report the pushed commit SHA.
//!
//! We do **not** log the full push URL — the access token would leak. Only
//! `owner/repo`, branch, and commit SHA are emitted.
use super::github_common::{LoadRepoContextError, load_active_repo_context};
use crate::system_api::*;
use std::path::Path;
use stem_git::Redactor;
use stem_git::git::{CommitOptions, PushOptions, commit_all, push_head, rev_parse_head};
use stem_git::github::{InstallationClient, config};

#[async_trait::async_trait]
impl PushProjectChangesToRepoSystem for super::AppSystems {
    async fn execute(
        &self,
        pool: &sqlx::PgPool,
        input: PushProjectChangesToRepoInput,
    ) -> Result<PushProjectChangesToRepoOutput, PushProjectChangesToRepoError> {
        let ctx = load_active_repo_context(pool, input.project_id)
            .await
            .map_err(map_err)?;

        if input.branch_name.trim().is_empty() {
            return Err(PushProjectChangesToRepoError::PushFailed(
                "branch_name must not be empty".into(),
            ));
        }
        if input.commit_message.trim().is_empty() {
            return Err(PushProjectChangesToRepoError::PushFailed(
                "commit_message must not be empty".into(),
            ));
        }
        if input.source_dir.trim().is_empty() {
            return Err(PushProjectChangesToRepoError::PushFailed(
                "source_dir must not be empty".into(),
            ));
        }

        let source = Path::new(input.source_dir.as_str());
        if !source.exists() {
            return Err(PushProjectChangesToRepoError::PushFailed(format!(
                "source_dir does not exist: {}",
                source.display()
            )));
        }
        if !source.join(".git").exists() {
            return Err(PushProjectChangesToRepoError::PushFailed(format!(
                "source_dir is not a git checkout: {}",
                source.display()
            )));
        }

        if config().is_none() {
            return Err(PushProjectChangesToRepoError::PushFailed(
                "GitHub App not configured (GITHUB_APP_ID / \
                 GITHUB_APP_PRIVATE_KEY[_PATH] / GITHUB_APP_WEBHOOK_SECRET)"
                    .into(),
            ));
        }

        let client = InstallationClient::for_installation(ctx.installation_id_remote)
            .await
            .map_err(|e| PushProjectChangesToRepoError::GithubApiError(e.to_string()))?;

        // `--allow-empty` is important: OpenCode may not have written
        // anything and we still want a branch ref on the remote.
        commit_all(
            source,
            CommitOptions {
                message: input.commit_message.as_str(),
                author_name: "Stem Cell",
                author_email: "bot@taller.diy",
                allow_empty: true,
            },
        )
        .await
        .map_err(|e| PushProjectChangesToRepoError::PushFailed(e.to_string()))?;

        let push_url = client.git_https_url(&ctx.owner, &ctx.repo);
        let force = input.force.unwrap_or(false);

        push_head(
            source,
            PushOptions {
                remote_url: &push_url,
                branch_name: input.branch_name.as_str(),
                force_with_lease: force,
                redactor: Redactor::new().with_secret(client.token.as_str()),
            },
        )
        .await
        .map_err(|e| PushProjectChangesToRepoError::PushFailed(e.to_string()))?;

        let sha = rev_parse_head(source)
            .await
            .map_err(|e| PushProjectChangesToRepoError::PushFailed(e.to_string()))?;

        tracing::info!(
            project_id = %ctx.project_id,
            owner = %ctx.owner,
            repo = %ctx.repo,
            branch = %input.branch_name,
            commit_sha = %sha,
            force,
            "pushed to remote"
        );

        Ok(PushProjectChangesToRepoOutput {
            commit_sha: sha,
            status: if force {
                "force_pushed".into()
            } else {
                "pushed".into()
            },
        })
    }
}

fn map_err(e: LoadRepoContextError) -> PushProjectChangesToRepoError {
    match e {
        LoadRepoContextError::ProjectNotFound => PushProjectChangesToRepoError::ProjectNotFound,
        LoadRepoContextError::RepoNotConnected => PushProjectChangesToRepoError::RepoNotConnected,
        LoadRepoContextError::InstallationInactive => {
            PushProjectChangesToRepoError::InstallationInactive
        }
        LoadRepoContextError::Database(msg) => PushProjectChangesToRepoError::Internal(msg),
    }
}
