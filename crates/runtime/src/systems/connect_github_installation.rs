//! Upsert a GitHub App installation record for an organization.
//!
//! We prefer the App installation model (per `AGENTS.md`): the GitHub-side
//! numeric `installation_id` is the source of truth, and this system
//! idempotently creates or refreshes the mirrored row. Actual GitHub
//! verification (fetching live permissions / status) is a separate concern
//! handled by `RefreshGithubInstallationState` — keeping them apart lets
//! webhook-driven flows connect the installation without a network round
//! trip.
use crate::system_api::*;
use sqlx::Row;

const VALID_TARGET_TYPES: &[&str] = &["User", "Organization"];

#[async_trait::async_trait]
impl ConnectGithubInstallationSystem for super::AppSystems {
    async fn execute(
        &self,
        pool: &sqlx::PgPool,
        input: ConnectGithubInstallationInput,
    ) -> Result<ConnectGithubInstallationOutput, ConnectGithubInstallationError> {
        if !VALID_TARGET_TYPES.contains(&input.target_type.as_str()) {
            return Err(ConnectGithubInstallationError::InvalidTargetType);
        }

        let status = input.status.clone().unwrap_or_else(|| "active".to_string());
        let active = status == "active";

        let org_row = sqlx::query(
            "SELECT active FROM organizations \
             WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(input.org_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| ConnectGithubInstallationError::DatabaseError(e.to_string()))?;

        let org_row = org_row.ok_or(ConnectGithubInstallationError::OrgNotFound)?;
        let org_active: bool = org_row.get("active");
        if !org_active {
            return Err(ConnectGithubInstallationError::OrgNotFound);
        }

        // Atomic upsert keyed on GitHub's numeric installation id so we tolerate
        // re-delivery of the `installation` webhook without creating duplicates.
        let new_id = uuid::Uuid::new_v4();
        let installed_id: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO github_installations \
               (id, installation_id, account_login, target_type, permissions, \
                status, active, org_id, installer_user_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             ON CONFLICT (installation_id) DO UPDATE SET \
               account_login     = EXCLUDED.account_login, \
               target_type       = EXCLUDED.target_type, \
               permissions       = EXCLUDED.permissions, \
               status            = EXCLUDED.status, \
               active            = EXCLUDED.active, \
               org_id            = EXCLUDED.org_id, \
               installer_user_id = EXCLUDED.installer_user_id, \
               deleted_at        = NULL, \
               updated_at        = NOW() \
             RETURNING id",
        )
        .bind(new_id)
        .bind(input.installation_id)
        .bind(&input.account_login)
        .bind(&input.target_type)
        .bind(&input.permissions)
        .bind(&status)
        .bind(active)
        .bind(input.org_id)
        .bind(input.installer_user_id)
        .fetch_one(pool)
        .await
        .map_err(|e| ConnectGithubInstallationError::DatabaseError(e.to_string()))?;

        tracing::info!(
            github_installation_id = %installed_id,
            installation_id = input.installation_id,
            account_login = %input.account_login,
            %status,
            active,
            "github installation upserted"
        );

        Ok(ConnectGithubInstallationOutput {
            github_installation_id: installed_id,
            status,
            active,
        })
    }
}
