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
use super::github_common::{
    load_active_repo_context, LoadRepoContextError,
};
use crate::github_app::{InstallationClient, config};
use crate::system_api::*;
use std::path::Path;
use tokio::process::Command;

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

        // Stage + commit. `--allow-empty` is important: OpenCode may not have
        // written anything and we still want a branch ref on the remote (so
        // the subsequent PR has something to aim at).
        run_git(source, &["add", "-A"]).await?;
        run_git(
            source,
            &[
                "-c",
                "user.name=Stem Cell",
                "-c",
                "user.email=bot@taller.diy",
                "commit",
                "--allow-empty",
                "-m",
                input.commit_message.as_str(),
            ],
        )
        .await?;

        let push_url = client.git_https_url(&ctx.owner, &ctx.repo);
        let refspec = format!("HEAD:refs/heads/{}", input.branch_name);
        let force = input.force.unwrap_or(false);

        // --force-with-lease is safer than --force: it still refuses to
        // clobber history that appeared on the remote since the caller's
        // last fetch. For a brand-new branch (most of our flows) it acts
        // like a normal push.
        let mut push_args: Vec<&str> = vec!["push"];
        if force {
            push_args.push("--force-with-lease");
        }
        push_args.push(&push_url);
        push_args.push(&refspec);

        run_git(source, &push_args).await.map_err(|e| {
            // Never leak the URL (tokenised). `run_git` already scrubs on
            // the way out, but belt-and-braces: re-scrub here.
            PushProjectChangesToRepoError::PushFailed(scrub_token(&e.to_string(), &client.token))
        })?;

        let sha = run_git_output(source, &["rev-parse", "HEAD"])
            .await
            .map_err(|e| {
                PushProjectChangesToRepoError::PushFailed(scrub_token(&e.to_string(), &client.token))
            })?;
        let sha = sha.trim().to_string();

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
            status: if force { "force_pushed".into() } else { "pushed".into() },
        })
    }
}

async fn run_git(cwd: &Path, args: &[&str]) -> Result<(), PushProjectChangesToRepoError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        // Disable any interactive auth prompts — we carry credentials in
        // the URL and never want git asking stdin for a password.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "echo")
        .output()
        .await
        .map_err(|e| PushProjectChangesToRepoError::PushFailed(e.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(PushProjectChangesToRepoError::PushFailed(format!(
            "git {}: {}",
            sanitize_args(args),
            stderr.trim()
        )));
    }
    Ok(())
}

async fn run_git_output(
    cwd: &Path,
    args: &[&str],
) -> Result<String, PushProjectChangesToRepoError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .map_err(|e| PushProjectChangesToRepoError::PushFailed(e.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(PushProjectChangesToRepoError::PushFailed(format!(
            "git {}: {}",
            sanitize_args(args),
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Redact any `x-access-token:<tok>@` substring from error text so logs and
/// API responses never echo a minted token.
fn scrub_token(s: &str, token: &str) -> String {
    if token.is_empty() {
        return s.to_string();
    }
    s.replace(token, "<redacted>")
}

fn sanitize_args(args: &[&str]) -> String {
    args.iter()
        .map(|a| if a.contains("x-access-token:") { "<redacted-url>" } else { *a })
        .collect::<Vec<_>>()
        .join(" ")
}

fn map_err(e: LoadRepoContextError) -> PushProjectChangesToRepoError {
    match e {
        LoadRepoContextError::ProjectNotFound => PushProjectChangesToRepoError::ProjectNotFound,
        LoadRepoContextError::RepoNotConnected => {
            PushProjectChangesToRepoError::RepoNotConnected
        }
        LoadRepoContextError::InstallationInactive => {
            PushProjectChangesToRepoError::InstallationInactive
        }
        LoadRepoContextError::Database(msg) => PushProjectChangesToRepoError::Internal(msg),
    }
}
