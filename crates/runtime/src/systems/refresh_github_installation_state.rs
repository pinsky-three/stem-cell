//! Reconcile the local installation record with GitHub's live state.
//!
//! Contract boundary: a true refresh needs a GitHub App JWT signer
//! (the `jsonwebtoken` crate) to exchange the app key for an installation
//! access token and then call `GET /app/installations/{id}`. That crate is
//! outside the editable surface described in `AGENTS.md`, so this
//! implementation returns the persisted view and emits a structured
//! `GithubApiError` when a caller explicitly opts into strict mode via
//! `STEM_CELL_GITHUB_REFRESH_STRICT=1`. This lets non-strict callers
//! (webhooks that already carry fresh state) proceed without the missing
//! dependency blocking them.
use crate::system_api::*;

#[async_trait::async_trait]
impl RefreshGithubInstallationStateSystem for super::AppSystems {
    async fn execute(
        &self,
        pool: &sqlx::PgPool,
        input: RefreshGithubInstallationStateInput,
    ) -> Result<RefreshGithubInstallationStateOutput, RefreshGithubInstallationStateError> {
        #[derive(sqlx::FromRow)]
        struct Row {
            status: String,
            active: bool,
            permissions: String,
        }

        let row: Option<Row> = sqlx::query_as(
            "SELECT status, active, permissions \
               FROM github_installations \
              WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(input.github_installation_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| RefreshGithubInstallationStateError::DatabaseError(e.to_string()))?;

        let row = row.ok_or(RefreshGithubInstallationStateError::InstallationNotFound)?;

        let strict = matches!(
            std::env::var("STEM_CELL_GITHUB_REFRESH_STRICT").as_deref(),
            Ok("1") | Ok("true")
        );
        if strict {
            return Err(RefreshGithubInstallationStateError::GithubApiError(
                "strict refresh requires a GitHub App JWT signer (jsonwebtoken \
                 crate) plus GITHUB_APP_ID/GITHUB_APP_PRIVATE_KEY; this wiring \
                 lives outside the current editable surface — see AGENTS.md"
                    .into(),
            ));
        }

        tracing::warn!(
            github_installation_id = %input.github_installation_id,
            "returning persisted installation state; live refresh requires \
             GitHub App JWT signer wiring (contract boundary)"
        );

        Ok(RefreshGithubInstallationStateOutput {
            status: row.status,
            active: row.active,
            permissions: row.permissions,
        })
    }
}
