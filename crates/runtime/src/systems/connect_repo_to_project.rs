//! Bind a GitHub repo to a project under an installation.
//!
//! Invariant: at most one active `RepoConnection` per project. We deactivate
//! any prior active connection inside the same transaction so the invariant
//! is enforced even under concurrent callers. We do **not** delete prior
//! rows — keeping history is useful for auditing repo re-bindings.
use crate::system_api::*;
use sqlx::Row;

#[async_trait::async_trait]
impl ConnectRepoToProjectSystem for super::AppSystems {
    async fn execute(
        &self,
        pool: &sqlx::PgPool,
        input: ConnectRepoToProjectInput,
    ) -> Result<ConnectRepoToProjectOutput, ConnectRepoToProjectError> {
        let project_row = sqlx::query(
            "SELECT active FROM projects \
             WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(input.project_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| ConnectRepoToProjectError::DatabaseError(e.to_string()))?;
        let project_row = project_row.ok_or(ConnectRepoToProjectError::ProjectNotFound)?;
        let project_active: bool = project_row.get("active");
        if !project_active {
            return Err(ConnectRepoToProjectError::ProjectNotFound);
        }

        let installation_row = sqlx::query(
            "SELECT active, status FROM github_installations \
             WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(input.github_installation_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| ConnectRepoToProjectError::DatabaseError(e.to_string()))?;
        let installation_row =
            installation_row.ok_or(ConnectRepoToProjectError::InstallationNotFound)?;
        let installation_active: bool = installation_row.get("active");
        let installation_status: String = installation_row.get("status");
        if !installation_active || installation_status != "active" {
            return Err(ConnectRepoToProjectError::InstallationInactive);
        }

        let default_branch = input
            .default_branch
            .clone()
            .unwrap_or_else(|| "main".to_string());

        let mut tx = pool
            .begin()
            .await
            .map_err(|e| ConnectRepoToProjectError::DatabaseError(e.to_string()))?;

        // Invariant: one active RepoConnection per project. Deactivate prior
        // active rows before inserting the new one.
        sqlx::query(
            "UPDATE repo_connections \
                SET active = false, status = 'disconnected', updated_at = NOW() \
              WHERE project_id = $1 \
                AND active = true \
                AND deleted_at IS NULL",
        )
        .bind(input.project_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| ConnectRepoToProjectError::DatabaseError(e.to_string()))?;

        let new_id = uuid::Uuid::new_v4();
        sqlx::query(
            "INSERT INTO repo_connections \
               (id, owner, repo, default_branch, status, active, project_id, installation_id) \
             VALUES ($1, $2, $3, $4, 'connected', true, $5, $6)",
        )
        .bind(new_id)
        .bind(&input.owner)
        .bind(&input.repo)
        .bind(&default_branch)
        .bind(input.project_id)
        .bind(input.github_installation_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| ConnectRepoToProjectError::DatabaseError(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| ConnectRepoToProjectError::DatabaseError(e.to_string()))?;

        tracing::info!(
            repo_connection_id = %new_id,
            project_id = %input.project_id,
            owner = %input.owner,
            repo = %input.repo,
            %default_branch,
            "repo connected to project"
        );

        Ok(ConnectRepoToProjectOutput {
            repo_connection_id: new_id,
            default_branch,
            status: "connected".to_string(),
        })
    }
}
