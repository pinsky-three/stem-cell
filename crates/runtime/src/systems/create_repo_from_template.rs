//! Create a new repo in the user's/org's GitHub account from a template, and
//! bind it to the project as a `RepoConnection`.
//!
//! Primary entrypoint for the "new project → repo in the user's GitHub
//! account" flow. Two side effects in order:
//!
//!   1. `POST /repos/{tpl_owner}/{tpl_repo}/generate` (installation-scoped)
//!      creates the new repo on GitHub.
//!   2. A transaction deactivates any prior active `RepoConnection` for the
//!      project and inserts a fresh `connected` one pointing at the new repo.
//!
//! `include_all_branches` defaults to false (single-branch template copy).
//! `private` defaults to true — safer default for user projects.
use crate::system_api::*;
use sqlx::Row;
use stem_git::github::{
    CreateRepoFromTemplateRequest, InstallationClient, config, create_repo_from_template,
};

#[async_trait::async_trait]
impl CreateRepoFromTemplateSystem for super::AppSystems {
    async fn execute(
        &self,
        pool: &sqlx::PgPool,
        input: CreateRepoFromTemplateInput,
    ) -> Result<CreateRepoFromTemplateOutput, CreateRepoFromTemplateError> {
        // ── Validate project + installation ─────────────────────────────
        let project_row =
            sqlx::query("SELECT active FROM projects WHERE id = $1 AND deleted_at IS NULL")
                .bind(input.project_id)
                .fetch_optional(pool)
                .await
                .map_err(|e| CreateRepoFromTemplateError::DatabaseError(e.to_string()))?;
        let project_row = project_row.ok_or(CreateRepoFromTemplateError::ProjectNotFound)?;
        let project_active: bool = project_row.get("active");
        if !project_active {
            return Err(CreateRepoFromTemplateError::ProjectNotFound);
        }

        let installation_row = sqlx::query(
            "SELECT installation_id, active, status FROM github_installations \
             WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(input.github_installation_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| CreateRepoFromTemplateError::DatabaseError(e.to_string()))?;
        let installation_row =
            installation_row.ok_or(CreateRepoFromTemplateError::InstallationNotFound)?;
        let installation_id_remote: i64 = installation_row.get("installation_id");
        let installation_active: bool = installation_row.get("active");
        let installation_status: String = installation_row.get("status");
        if !installation_active || installation_status != "active" {
            return Err(CreateRepoFromTemplateError::InstallationInactive);
        }

        if config().is_none() {
            return Err(CreateRepoFromTemplateError::GithubApiError(
                "GitHub App not configured (GITHUB_APP_ID / \
                 GITHUB_APP_PRIVATE_KEY[_PATH] / GITHUB_APP_WEBHOOK_SECRET)"
                    .into(),
            ));
        }

        // ── Call GitHub ─────────────────────────────────────────────────
        let client = InstallationClient::for_installation(installation_id_remote)
            .await
            .map_err(|e| CreateRepoFromTemplateError::GithubApiError(e.to_string()))?;
        let created = create_repo_from_template(
            &client,
            CreateRepoFromTemplateRequest {
                template_owner: input.template_owner,
                template_repo: input.template_repo,
                new_owner: input.new_owner,
                new_name: input.new_name,
                description: input.description,
                private: input.private.unwrap_or(true),
                include_all_branches: input.include_all_branches.unwrap_or(false),
            },
        )
        .await
        .map_err(|e| CreateRepoFromTemplateError::GithubApiError(e.to_string()))?;

        let owner = created.owner;
        let repo = created.repo;
        let default_branch = created.default_branch;
        let html_url = created.html_url;

        // ── Upsert RepoConnection ───────────────────────────────────────
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| CreateRepoFromTemplateError::DatabaseError(e.to_string()))?;

        sqlx::query(
            "UPDATE repo_connections \
                SET active = false, status = 'disconnected', updated_at = NOW() \
              WHERE project_id = $1 AND active = true AND deleted_at IS NULL",
        )
        .bind(input.project_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| CreateRepoFromTemplateError::DatabaseError(e.to_string()))?;

        let new_conn_id = uuid::Uuid::new_v4();
        sqlx::query(
            "INSERT INTO repo_connections \
               (id, owner, repo, default_branch, status, active, project_id, installation_id) \
             VALUES ($1, $2, $3, $4, 'connected', true, $5, $6)",
        )
        .bind(new_conn_id)
        .bind(&owner)
        .bind(&repo)
        .bind(&default_branch)
        .bind(input.project_id)
        .bind(input.github_installation_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| CreateRepoFromTemplateError::DatabaseError(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| CreateRepoFromTemplateError::DatabaseError(e.to_string()))?;

        tracing::info!(
            project_id = %input.project_id,
            repo_connection_id = %new_conn_id,
            %owner,
            %repo,
            %default_branch,
            %html_url,
            "repo created from template and connected"
        );

        Ok(CreateRepoFromTemplateOutput {
            owner,
            repo,
            default_branch,
            html_url,
            repo_connection_id: new_conn_id,
            status: "connected".into(),
        })
    }
}
