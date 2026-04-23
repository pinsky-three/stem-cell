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
use crate::github_app::{InstallationClient, config};
use crate::system_api::*;
use sqlx::Row;

#[async_trait::async_trait]
impl CreateRepoFromTemplateSystem for super::AppSystems {
    async fn execute(
        &self,
        pool: &sqlx::PgPool,
        input: CreateRepoFromTemplateInput,
    ) -> Result<CreateRepoFromTemplateOutput, CreateRepoFromTemplateError> {
        // ── Validate project + installation ─────────────────────────────
        let project_row = sqlx::query(
            "SELECT active FROM projects WHERE id = $1 AND deleted_at IS NULL",
        )
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

        let url = format!(
            "https://api.github.com/repos/{}/{}/generate",
            input.template_owner, input.template_repo
        );
        let mut body = serde_json::json!({
            "owner": input.new_owner,
            "name":  input.new_name,
            "private": input.private.unwrap_or(true),
            "include_all_branches": input.include_all_branches.unwrap_or(false),
        });
        if let Some(desc) = input.description.as_deref().filter(|s| !s.is_empty()) {
            body["description"] = serde_json::Value::String(desc.into());
        }

        let res = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| CreateRepoFromTemplateError::GithubApiError(e.to_string()))?;
        let status = res.status();
        let response_body: serde_json::Value = res
            .json()
            .await
            .map_err(|e| CreateRepoFromTemplateError::GithubApiError(e.to_string()))?;

        if !status.is_success() {
            return Err(CreateRepoFromTemplateError::GithubApiError(format!(
                "POST {url}: {status} {response_body}"
            )));
        }

        let owner = response_body["owner"]["login"]
            .as_str()
            .unwrap_or(&input.new_owner)
            .to_string();
        let repo = response_body["name"]
            .as_str()
            .unwrap_or(&input.new_name)
            .to_string();
        let default_branch = response_body["default_branch"]
            .as_str()
            .unwrap_or("main")
            .to_string();
        let html_url = response_body["html_url"]
            .as_str()
            .unwrap_or("")
            .to_string();

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
