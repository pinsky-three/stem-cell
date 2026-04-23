//! GitHub App webhook handler.
//!
//! Mount point: `POST /github/app/webhook` (see `main.rs`).
//!
//! This endpoint does three things:
//!
//!   1. **Verify** `X-Hub-Signature-256` against the configured webhook
//!      secret (HMAC-SHA256, constant-time compare). Unsigned / mis-signed
//!      requests get 401.
//!   2. **Dispatch** `installation.*` and `installation_repositories.*`
//!      events to the local mirror (`github_installations` + `repo_connections`).
//!      The *creation* of an installation row is owned by the Setup-URL
//!      flow (the frontend knows the current session's org/user); this
//!      webhook only updates existing rows on state changes.
//!   3. **Ignore gracefully** anything else. We log at debug and return
//!      200 so GitHub doesn't mark the endpoint unhealthy and retry.
//!
//! Why updates-only (not upserts)? The `github_installations.org_id` column
//! is non-null. A raw webhook payload doesn't know which Stem Cell org it
//! belongs to — only the authenticated Setup-URL redirect does. Treating
//! the webhook as an idempotent state-mirror (not a creator) avoids both
//! placeholder rows and a cross-table reconciliation pass.
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use serde::Serialize;
use sqlx::PgPool;

use crate::github_app::{
    GithubAppConfig, SharedGithubAppConfig, config, verify_webhook_signature,
};

#[derive(Clone)]
pub struct WebhookState {
    pub pool: PgPool,
    pub cfg: SharedGithubAppConfig,
}

/// Build the webhook router. Returns None when the App isn't configured —
/// callers should log and skip mounting.
pub fn router(pool: PgPool) -> Option<Router> {
    let cfg = config()?.clone();
    let state = WebhookState {
        pool,
        cfg: Arc::new(GithubAppConfig {
            app_id: cfg.app_id,
            private_key_pem: cfg.private_key_pem.clone(),
            webhook_secret: cfg.webhook_secret.clone(),
            app_slug: cfg.app_slug.clone(),
        }),
    };
    Some(
        Router::new()
            .route("/github/app/webhook", post(handle))
            .with_state(state),
    )
}

#[derive(Serialize)]
struct WebhookResponse {
    ok: bool,
    handled: &'static str,
}

async fn handle(
    State(state): State<WebhookState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let sig = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
        .ok_or((StatusCode::UNAUTHORIZED, "missing signature".into()))?;

    if !verify_webhook_signature(&state.cfg.webhook_secret, &body, sig) {
        return Err((StatusCode::UNAUTHORIZED, "invalid signature".into()));
    }

    let event = headers
        .get("x-github-event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let delivery = headers
        .get("x-github-delivery")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return Err((StatusCode::BAD_REQUEST, format!("invalid json: {e}"))),
    };
    let action = payload
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    tracing::info!(
        %event,
        %action,
        %delivery,
        "github webhook received"
    );

    let handled = match (event.as_str(), action.as_str()) {
        ("installation", "deleted") => handle_installation_deleted(&state, &payload).await,
        ("installation", "suspend") => {
            handle_installation_lifecycle(&state, &payload, "suspended", false, "installation_suspended").await
        }
        ("installation", "unsuspend") => {
            handle_installation_lifecycle(&state, &payload, "active", true, "connected").await
        }
        ("installation", "new_permissions_accepted") => {
            handle_installation_permissions(&state, &payload).await
        }
        // Creation is authoritative from the Setup URL, not the webhook. We
        // still log for observability.
        ("installation", "created") => Ok("installation.created (ignored; Setup URL owns creation)"),
        ("installation_repositories", "removed") => handle_repos_removed(&state, &payload).await,
        ("installation_repositories", "added") => Ok("installation_repositories.added (observed)"),
        ("ping", _) => Ok("pong"),
        _ => Ok("ignored"),
    };

    match handled {
        Ok(tag) => Ok((StatusCode::OK, Json(WebhookResponse { ok: true, handled: tag }))),
        Err(e) => {
            tracing::error!(%event, %action, %delivery, error = %e, "webhook handler failed");
            Err((StatusCode::INTERNAL_SERVER_ERROR, e))
        }
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────

fn installation_id(payload: &serde_json::Value) -> Option<i64> {
    payload
        .get("installation")
        .and_then(|i| i.get("id"))
        .and_then(|v| v.as_i64())
}

async fn handle_installation_deleted(
    state: &WebhookState,
    payload: &serde_json::Value,
) -> Result<&'static str, String> {
    let id = installation_id(payload).ok_or("installation.id missing")?;

    sqlx::query(
        "UPDATE github_installations \
            SET status = 'deleted', active = false, updated_at = NOW() \
          WHERE installation_id = $1",
    )
    .bind(id)
    .execute(&state.pool)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "UPDATE repo_connections \
            SET status = 'installation_revoked', active = false, updated_at = NOW() \
          WHERE installation_id IN ( \
             SELECT id FROM github_installations WHERE installation_id = $1 \
          ) AND active = true",
    )
    .bind(id)
    .execute(&state.pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok("installation.deleted")
}

async fn handle_installation_lifecycle(
    state: &WebhookState,
    payload: &serde_json::Value,
    new_status: &str,
    new_active: bool,
    cascade_conn_status: &str,
) -> Result<&'static str, String> {
    let id = installation_id(payload).ok_or("installation.id missing")?;

    sqlx::query(
        "UPDATE github_installations \
            SET status = $2, active = $3, updated_at = NOW() \
          WHERE installation_id = $1",
    )
    .bind(id)
    .bind(new_status)
    .bind(new_active)
    .execute(&state.pool)
    .await
    .map_err(|e| e.to_string())?;

    // Only propagate the "active again" tag to connections that were inactive
    // *because* of the suspension — don't clobber a `disconnected` row.
    sqlx::query(
        "UPDATE repo_connections \
            SET status = $2, active = $3, updated_at = NOW() \
          WHERE installation_id IN ( \
             SELECT id FROM github_installations WHERE installation_id = $1 \
          ) AND status IN ('installation_suspended', 'connected')",
    )
    .bind(id)
    .bind(cascade_conn_status)
    .bind(new_active)
    .execute(&state.pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(if new_active { "installation.unsuspend" } else { "installation.suspend" })
}

async fn handle_installation_permissions(
    state: &WebhookState,
    payload: &serde_json::Value,
) -> Result<&'static str, String> {
    let id = installation_id(payload).ok_or("installation.id missing")?;
    let perms = payload
        .get("installation")
        .and_then(|i| i.get("permissions"))
        .cloned()
        .unwrap_or(serde_json::Value::Null)
        .to_string();
    sqlx::query(
        "UPDATE github_installations \
            SET permissions = $2, updated_at = NOW() \
          WHERE installation_id = $1",
    )
    .bind(id)
    .bind(perms)
    .execute(&state.pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok("installation.new_permissions_accepted")
}

async fn handle_repos_removed(
    state: &WebhookState,
    payload: &serde_json::Value,
) -> Result<&'static str, String> {
    let id = installation_id(payload).ok_or("installation.id missing")?;
    let removed = payload
        .get("repositories_removed")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for repo in removed {
        // "full_name" is "owner/repo"
        let full = repo.get("full_name").and_then(|v| v.as_str()).unwrap_or("");
        let Some((owner, name)) = full.split_once('/') else { continue };
        sqlx::query(
            "UPDATE repo_connections \
                SET status = 'disconnected', active = false, updated_at = NOW() \
              WHERE owner = $1 AND repo = $2 \
                AND installation_id IN ( \
                   SELECT id FROM github_installations WHERE installation_id = $3 \
                ) AND active = true",
        )
        .bind(owner)
        .bind(name)
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|e| e.to_string())?;
    }

    Ok("installation_repositories.removed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installation_id_extraction() {
        let v = serde_json::json!({
            "action": "deleted",
            "installation": { "id": 12345 }
        });
        assert_eq!(installation_id(&v), Some(12345));
        assert_eq!(installation_id(&serde_json::json!({})), None);
    }
}
