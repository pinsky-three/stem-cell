//! Push the project checkout to its connected repo on the given branch.
//!
//! Contract boundary: the actual push transport requires a short-lived
//! **installation access token**, which is obtained by signing a GitHub App
//! JWT (RS256) against the app's private key and POSTing to
//! `/app/installations/{id}/access_tokens`. That signing step needs the
//! `jsonwebtoken` crate, which is a `Cargo.toml` edit — off-limits per
//! `AGENTS.md`. Rather than faking a success, we validate everything we
//! *can* (project, repo connection, installation liveness, branch shape)
//! and return a precise `PushFailed` with the blocker diagnosis so callers
//! can surface it cleanly.
//!
//! The next minimum scope expansion needed to make this real:
//!   1. Add `jsonwebtoken` to `crates/runtime/Cargo.toml`.
//!   2. Implement `mint_installation_token(installation_id_remote) -> String`
//!      in a helper (can live here).
//!   3. Replace the `Err(...)` at the bottom with a real `git push` or
//!      REST-API-based content upload against `https://x-access-token:$TOKEN@github.com/...`.
use super::github_common::{
    github_app_credentials_present, load_active_repo_context, LoadRepoContextError,
};
use crate::system_api::*;

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

        tracing::info!(
            project_id = %ctx.project_id,
            owner = %ctx.owner,
            repo = %ctx.repo,
            branch = %input.branch_name,
            commit_message_len = input.commit_message.len(),
            creds_present = github_app_credentials_present(),
            "push_project_changes_to_repo: hitting contract boundary"
        );

        let diagnostic = if github_app_credentials_present() {
            "push transport not wired: GITHUB_APP_ID / GITHUB_APP_PRIVATE_KEY \
             are present, but the JWT signer (jsonwebtoken crate) required to \
             mint an installation access token is outside the editable surface \
             (Cargo.toml is forbidden by AGENTS.md)."
        } else {
            "push transport not wired: GITHUB_APP_ID / GITHUB_APP_PRIVATE_KEY \
             are not configured, and the JWT signer (jsonwebtoken crate) is \
             outside the editable surface. Configure the app + wire the \
             signer in a follow-up scope expansion."
        };

        Err(PushProjectChangesToRepoError::PushFailed(diagnostic.into()))
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
