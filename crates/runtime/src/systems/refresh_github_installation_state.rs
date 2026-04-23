//! Reconcile the local installation record with GitHub's live state.
//!
//! We sign an App JWT, call `GET /app/installations/{id}`, and update the
//! mirrored row. A 404 from GitHub (installation deleted or revoked) marks
//! the row inactive instead of erroring — that's the expected terminal state
//! after a user uninstalls the App.
//!
//! When the App isn't configured (`GithubAppConfig::from_env` returned
//! `None`), we fall back to returning the persisted view with a warning.
//! That lets non-strict callers (e.g. webhook handlers that already carry
//! fresh state) proceed without an App JWT; `GITHUB_APP_REFRESH_STRICT=1`
//! forces an error instead.
use crate::github_app::{AppClient, GithubAppError, config};
use crate::system_api::*;

#[async_trait::async_trait]
impl RefreshGithubInstallationStateSystem for super::AppSystems {
    async fn execute(
        &self,
        pool: &sqlx::PgPool,
        input: RefreshGithubInstallationStateInput,
    ) -> Result<RefreshGithubInstallationStateOutput, RefreshGithubInstallationStateError> {
        #[derive(sqlx::FromRow)]
        struct Local {
            installation_id: i64,
            status: String,
            active: bool,
            permissions: String,
        }

        let local: Option<Local> = sqlx::query_as(
            "SELECT installation_id, status, active, permissions \
               FROM github_installations \
              WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(input.github_installation_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| RefreshGithubInstallationStateError::DatabaseError(e.to_string()))?;

        let local = local.ok_or(RefreshGithubInstallationStateError::InstallationNotFound)?;

        let Some(_cfg) = config() else {
            let strict = matches!(
                std::env::var("GITHUB_APP_REFRESH_STRICT").as_deref(),
                Ok("1") | Ok("true")
            );
            if strict {
                return Err(RefreshGithubInstallationStateError::GithubApiError(
                    "GitHub App not configured (GITHUB_APP_ID / \
                     GITHUB_APP_PRIVATE_KEY[_PATH] / GITHUB_APP_WEBHOOK_SECRET)"
                        .into(),
                ));
            }
            tracing::warn!(
                github_installation_id = %input.github_installation_id,
                "returning persisted installation state; App not configured"
            );
            return Ok(RefreshGithubInstallationStateOutput {
                status: local.status,
                active: local.active,
                permissions: local.permissions,
            });
        };

        let client = AppClient::new().map_err(map_app_err)?;
        let url = format!(
            "https://api.github.com/app/installations/{}",
            local.installation_id
        );

        let res = client
            .get(&url)
            .send()
            .await
            .map_err(|e| RefreshGithubInstallationStateError::GithubApiError(e.to_string()))?;
        let status_code = res.status();

        if status_code.as_u16() == 404 {
            // Installation deleted / revoked on GitHub. Mark it inactive and
            // cascade-disconnect its repo connections so the frontend stops
            // offering them.
            tracing::info!(
                github_installation_id = %input.github_installation_id,
                "installation missing on GitHub; marking inactive"
            );
            sqlx::query(
                "UPDATE github_installations SET status = 'deleted', active = false, \
                 updated_at = NOW() WHERE id = $1",
            )
            .bind(input.github_installation_id)
            .execute(pool)
            .await
            .map_err(|e| RefreshGithubInstallationStateError::DatabaseError(e.to_string()))?;
            sqlx::query(
                "UPDATE repo_connections SET status = 'installation_revoked', \
                 active = false, updated_at = NOW() \
                 WHERE installation_id = $1 AND active = true",
            )
            .bind(input.github_installation_id)
            .execute(pool)
            .await
            .map_err(|e| RefreshGithubInstallationStateError::DatabaseError(e.to_string()))?;
            return Ok(RefreshGithubInstallationStateOutput {
                status: "deleted".into(),
                active: false,
                permissions: local.permissions,
            });
        }
        if !status_code.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(RefreshGithubInstallationStateError::GithubApiError(format!(
                "GET /app/installations/{}: {} {}",
                local.installation_id, status_code, body
            )));
        }

        let body: serde_json::Value = res
            .json()
            .await
            .map_err(|e| RefreshGithubInstallationStateError::GithubApiError(e.to_string()))?;

        let suspended_at = body.get("suspended_at").and_then(|v| v.as_str());
        let live_status = match suspended_at {
            Some(_) => "suspended",
            None => "active",
        };
        let live_active = live_status == "active";
        let permissions_json = body
            .get("permissions")
            .cloned()
            .unwrap_or(serde_json::Value::Null)
            .to_string();

        sqlx::query(
            "UPDATE github_installations \
                SET status = $2, active = $3, permissions = $4, updated_at = NOW() \
              WHERE id = $1",
        )
        .bind(input.github_installation_id)
        .bind(live_status)
        .bind(live_active)
        .bind(&permissions_json)
        .execute(pool)
        .await
        .map_err(|e| RefreshGithubInstallationStateError::DatabaseError(e.to_string()))?;

        tracing::info!(
            github_installation_id = %input.github_installation_id,
            status = live_status,
            active = live_active,
            "installation refreshed from GitHub"
        );

        Ok(RefreshGithubInstallationStateOutput {
            status: live_status.into(),
            active: live_active,
            permissions: permissions_json,
        })
    }
}

fn map_app_err(e: GithubAppError) -> RefreshGithubInstallationStateError {
    RefreshGithubInstallationStateError::GithubApiError(e.to_string())
}
