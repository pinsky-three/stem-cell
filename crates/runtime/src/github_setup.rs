//! GitHub App Setup URL handler + public App-info endpoint.
//!
//! Two routes:
//!
//! - `GET /github/app/info`
//!   Public `{ configured: bool, slug: Option<String> }` payload. The
//!   frontend hits this on mount to decide whether to render the Install
//!   button and which GitHub App slug to point it at. Emitting it from the
//!   backend means the frontend never needs the App slug baked into its
//!   build — we can change Apps just by flipping env vars.
//!
//! - `GET /github/app/setup`
//!   Redirect target registered on the App ("Post-installation → Setup
//!   URL"). GitHub sends the user here after they install the App; the
//!   query carries `installation_id`, `setup_action`, and our own opaque
//!   `state` (base64-encoded JSON — see [`SetupState`]).
//!
//! The Setup URL is the **authoritative creator** of the local
//! `github_installations` row. The webhook is intentionally write-only for
//! updates to existing rows (see `github_webhook.rs`) because only the
//! authenticated Setup URL request knows which Stem Cell organization the
//! installing user belongs to. Doing the upsert here keeps that invariant
//! clean and avoids orphan rows.
//!
//! Flow:
//!   1. Require a session cookie. No session → 401 HTML explaining the retry
//!      path (sign in, then re-run install). We don't try to preserve the
//!      query across login because installation is idempotent on GitHub's
//!      side — the user can click "Install" again safely.
//!   2. Decode `state` → optional `project_id`, `org_id`, `user_id`, `return_to`.
//!   3. Mint an App JWT and `GET /app/installations/{id}` to pull
//!      `account.login`, `account.type`, and `permissions`. This is the
//!      single source of truth — never trust the client-side query for these.
//!   4. Invoke `ConnectGithubInstallation` (reuses upsert + validation).
//!   5. 302 back to `return_to` / `/project/<id>` / `/github/install`
//!      with a `?github=installed&installation=<uuid>` marker the page
//!      reacts to.

use axum::{
    Json,
    Router,
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use crate::auth::AppState;
use crate::auth::middleware::{CurrentAccount, MaybeAccount};
use stem_cell::github_app::{self, AppClient};
use stem_cell::system_api::{ConnectGithubInstallationInput, ConnectGithubInstallationSystem};
use stem_cell::systems::AppSystems;

/// Matches the placeholder UUIDs wired into `HeroPrompt.tsx` /
/// `ProjectView.tsx`. Until we model `Account ↔ Organization` membership
/// properly, the frontend speaks in these defaults and we mirror that here
/// so an install always lands against a known-good organization row.
const DEFAULT_ORG_ID: Uuid = Uuid::from_u128(1);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/github/app/info", get(info))
        .route("/github/app/status", get(status))
        .route("/github/app/sync", post(sync))
        .route("/github/app/setup", get(setup))
}

#[derive(Serialize)]
struct InfoResponse {
    /// True when the backend has all three of `GITHUB_APP_ID`,
    /// `GITHUB_APP_PRIVATE_KEY[_PATH]`, and `GITHUB_APP_WEBHOOK_SECRET`.
    /// Frontends should hide the install CTA otherwise.
    configured: bool,
    /// The App slug used to build install URLs
    /// (`https://github.com/apps/<slug>/installations/new`). `None` when the
    /// App is not configured or `GITHUB_APP_SLUG` is unset.
    slug: Option<String>,
}

async fn info() -> impl IntoResponse {
    let (configured, slug) = match github_app::config() {
        Some(cfg) => (true, cfg.app_slug.clone()),
        None => (false, None),
    };
    axum::Json(InfoResponse { configured, slug })
}

#[derive(Serialize)]
struct InstallationSummary {
    id: Uuid,
    account_login: String,
    target_type: String,
    status: String,
    active: bool,
}

#[derive(Serialize)]
struct StatusResponse {
    configured: bool,
    slug: Option<String>,
    authenticated: bool,
    github_login: Option<String>,
    installations: Vec<InstallationSummary>,
}

#[derive(Serialize)]
struct SyncResponse {
    synced: bool,
    github_login: Option<String>,
    installation: Option<InstallationSummary>,
    message: String,
}

async fn status(
    State(state): State<AppState>,
    MaybeAccount(account): MaybeAccount,
) -> Result<Json<StatusResponse>, (StatusCode, String)> {
    let (configured, slug) = match github_app::config() {
        Some(cfg) => (true, cfg.app_slug.clone()),
        None => (false, None),
    };

    let Some(account) = account else {
        return Ok(Json(StatusResponse {
            configured,
            slug,
            authenticated: false,
            github_login: None,
            installations: Vec::new(),
        }));
    };

    let github_login = github_login_for_account(&state.pool, account.id)
        .await
        .map_err(internal)?;
    let installations = local_installations(&state.pool)
        .await
        .map_err(internal)?;

    Ok(Json(StatusResponse {
        configured,
        slug,
        authenticated: true,
        github_login,
        installations,
    }))
}

async fn sync(
    State(state): State<AppState>,
    CurrentAccount(account): CurrentAccount,
) -> Result<Json<SyncResponse>, (StatusCode, String)> {
    if github_app::config().is_none() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "GitHub App not configured".into(),
        ));
    }

    let Some(github_login) = github_login_for_account(&state.pool, account.id)
        .await
        .map_err(internal)?
    else {
        return Ok(Json(SyncResponse {
            synced: false,
            github_login: None,
            installation: None,
            message: "Sign in with GitHub first so Stem Cell knows which GitHub account to sync.".into(),
        }));
    };

    let Some(payload) = find_app_installation_for_login(github_login.as_str()).await? else {
        return Ok(Json(SyncResponse {
            synced: false,
            github_login: Some(github_login),
            installation: None,
            message: "No GitHub App installation was found for this personal account. Install the App, then sync again.".into(),
        }));
    };

    let out = upsert_installation_payload(&state.pool, &payload, DEFAULT_ORG_ID, None).await?;
    let summary = installation_summary_by_id(&state.pool, out.github_installation_id)
        .await
        .map_err(internal)?;

    tracing::info!(
        github_installation_id = %out.github_installation_id,
        github_login = %github_login,
        "github installation synced from existing App installation"
    );

    Ok(Json(SyncResponse {
        synced: true,
        github_login: Some(github_login),
        installation: summary,
        message: "GitHub storage is connected. New successful builds will persist to a repo/branch automatically.".into(),
    }))
}

#[derive(Debug, Deserialize)]
struct SetupQuery {
    installation_id: Option<i64>,
    #[serde(default)]
    setup_action: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

/// Opaque blob the frontend round-trips through GitHub via the `state`
/// install-URL query param. All fields are optional so the button can pass
/// whatever context it has without breaking older clients.
#[derive(Debug, Deserialize, Default)]
struct SetupState {
    project_id: Option<Uuid>,
    org_id: Option<Uuid>,
    user_id: Option<Uuid>,
    /// Arbitrary relative URL to send the user back to. Falls back to a
    /// derived path (`/project/<id>` or `/github/install`).
    return_to: Option<String>,
}

fn decode_state(raw: Option<&str>) -> SetupState {
    raw.filter(|s| !s.is_empty())
        .and_then(|s| URL_SAFE_NO_PAD.decode(s).ok())
        .and_then(|bytes| serde_json::from_slice::<SetupState>(&bytes).ok())
        .unwrap_or_default()
}

/// Minimal allow-list for the `return_to` redirect. We only permit internal
/// relative paths so a stray query param can't turn the Setup URL into an
/// open redirect onto a third-party domain.
fn sanitize_return_to(raw: &str) -> Option<String> {
    if raw.starts_with('/') && !raw.starts_with("//") {
        Some(raw.to_string())
    } else {
        None
    }
}

fn internal(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

async fn github_login_for_account(
    pool: &sqlx::PgPool,
    account_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT username FROM oauth_links \
         WHERE account_id = $1 AND provider = 'github' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.and_then(|r| r.get::<Option<String>, _>("username")))
}

async fn local_installations(pool: &sqlx::PgPool) -> Result<Vec<InstallationSummary>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, account_login, target_type, status, active \
         FROM github_installations \
         WHERE org_id = $1 AND deleted_at IS NULL \
         ORDER BY active DESC, updated_at DESC",
    )
    .bind(DEFAULT_ORG_ID)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| InstallationSummary {
            id: r.get("id"),
            account_login: r.get("account_login"),
            target_type: r.get("target_type"),
            status: r.get("status"),
            active: r.get("active"),
        })
        .collect())
}

async fn installation_summary_by_id(
    pool: &sqlx::PgPool,
    id: Uuid,
) -> Result<Option<InstallationSummary>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, account_login, target_type, status, active \
         FROM github_installations \
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| InstallationSummary {
        id: r.get("id"),
        account_login: r.get("account_login"),
        target_type: r.get("target_type"),
        status: r.get("status"),
        active: r.get("active"),
    }))
}

async fn find_app_installation_for_login(
    github_login: &str,
) -> Result<Option<Value>, (StatusCode, String)> {
    let app_client =
        AppClient::new().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let res = app_client
        .get("https://api.github.com/app/installations?per_page=100")
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("github call failed: {e}")))?;

    if !res.status().is_success() {
        let status = res.status().as_u16();
        let body = res.text().await.unwrap_or_default();
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("github {status}: {body}"),
        ));
    }

    let payload: Value = res
        .json()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("parse error: {e}")))?;
    let installations = payload
        .as_array()
        .ok_or((StatusCode::BAD_GATEWAY, "github response was not an array".into()))?;

    Ok(installations.iter().find_map(|installation| {
        let login = installation
            .get("account")
            .and_then(|a| a.get("login"))
            .and_then(|v| v.as_str())?;
        if login.eq_ignore_ascii_case(github_login) {
            Some(installation.clone())
        } else {
            None
        }
    }))
}

async fn fetch_installation_payload(
    installation_id: i64,
) -> Result<Value, (StatusCode, String)> {
    let app_client =
        AppClient::new().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let url = format!("https://api.github.com/app/installations/{installation_id}");
    let res = app_client
        .get(&url)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("github call failed: {e}")))?;
    if !res.status().is_success() {
        let status = res.status().as_u16();
        let body = res.text().await.unwrap_or_default();
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("github {status}: {body}"),
        ));
    }
    res.json()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("parse error: {e}")))
}

async fn upsert_installation_payload(
    pool: &sqlx::PgPool,
    payload: &Value,
    org_id: Uuid,
    installer_user_id: Option<Uuid>,
) -> Result<stem_cell::system_api::ConnectGithubInstallationOutput, (StatusCode, String)> {
    let installation_id = payload
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or((StatusCode::BAD_GATEWAY, "github response missing id".into()))?;
    let account_login = payload
        .get("account")
        .and_then(|a| a.get("login"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let target_type = payload
        .get("account")
        .and_then(|a| a.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("User")
        .to_string();
    let permissions = payload
        .get("permissions")
        .cloned()
        .unwrap_or(Value::Null)
        .to_string();

    if account_login.is_empty() {
        return Err((
            StatusCode::BAD_GATEWAY,
            "github response missing account.login".into(),
        ));
    }

    let input = ConnectGithubInstallationInput {
        installation_id,
        account_login,
        target_type,
        permissions,
        status: Some("active".into()),
        org_id,
        installer_user_id,
    };

    AppSystems
        .execute(pool, input)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("{e:?}")))
}

async fn setup(
    State(state): State<AppState>,
    MaybeAccount(account): MaybeAccount,
    Query(q): Query<SetupQuery>,
) -> Result<axum::response::Response, (StatusCode, String)> {
    // ── Require a session ────────────────────────────────────────────────
    if account.is_none() {
        return Ok(login_required_page().into_response());
    }

    let installation_id = q
        .installation_id
        .ok_or((StatusCode::BAD_REQUEST, "missing installation_id".into()))?;

    let decoded = decode_state(q.state.as_deref());

    // ── If the App isn't configured, degrade gracefully. The user still
    //    gets a friendly redirect; the installation row just isn't mirrored
    //    locally until credentials are wired. ─────────────────────────────
    let Some(_cfg) = github_app::config() else {
        let fallback = decoded
            .return_to
            .as_deref()
            .and_then(sanitize_return_to)
            .unwrap_or_else(|| match decoded.project_id {
                Some(pid) => format!("/project/{pid}?github=not_configured"),
                None => "/github/install?github=not_configured".into(),
            });
        tracing::warn!(
            installation_id,
            "github setup callback received but App is not configured; skipping local mirror"
        );
        return Ok(Redirect::to(&fallback).into_response());
    };

    // ── Pull authoritative metadata from GitHub. Never trust the query
    //    params for account/login/permissions — a user could hand-craft the
    //    URL. The App JWT can only read installations owned by this App. ──
    let payload = fetch_installation_payload(installation_id).await?;
    let org_id = decoded.org_id.unwrap_or(DEFAULT_ORG_ID);
    let out = upsert_installation_payload(&state.pool, &payload, org_id, decoded.user_id).await?;

    tracing::info!(
        github_installation_id = %out.github_installation_id,
        installation_id,
        setup_action = ?q.setup_action,
        "github installation mirrored via Setup URL"
    );

    let redirect = decoded
        .return_to
        .as_deref()
        .and_then(sanitize_return_to)
        .unwrap_or_else(|| match decoded.project_id {
            Some(pid) => format!(
                "/project/{pid}?github=installed&installation={}",
                out.github_installation_id
            ),
            None => format!(
                "/github/install?github=installed&installation={}",
                out.github_installation_id
            ),
        });

    Ok(Redirect::to(&redirect).into_response())
}

/// Landing page shown when someone hits `/github/app/setup` without a
/// session. We don't try to preserve the install query across login because
/// GitHub installation itself is idempotent — the user can re-click the
/// Install button and GitHub will recognize the existing installation and
/// just re-fire the Setup URL.
fn login_required_page() -> impl IntoResponse {
    const BODY: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Sign in — GitHub install</title>
  <style>
    body { font-family: system-ui, -apple-system, Segoe UI, sans-serif;
           background: #0a0a0b; color: #e5e5e5; margin: 0;
           min-height: 100vh; display: grid; place-items: center; }
    .card { max-width: 460px; padding: 2rem; border: 1px solid #262626;
            border-radius: 12px; background: #111; }
    h1 { margin: 0 0 .5rem; font-size: 1.35rem; }
    p  { margin: .5rem 0; color: #a3a3a3; line-height: 1.55; font-size: 0.95rem; }
    a.btn { display: inline-block; margin-top: 1rem; padding: .6rem 1rem;
            background: #4f46e5; color: white; border-radius: 8px;
            text-decoration: none; font-weight: 600; font-size: 0.9rem; }
    a.btn:hover { background: #4338ca; }
  </style>
</head>
<body>
  <div class="card">
    <h1>Sign in to finish installing on GitHub</h1>
    <p>Your GitHub App installation was created, but you need to be signed in here
       to finish connecting it to your workspace.</p>
    <p>After signing in, click the Install button again — GitHub will recognize
       the existing installation and send you straight back here.</p>
    <a class="btn" href="/login">Sign in</a>
  </div>
</body>
</html>
"#;
    (StatusCode::UNAUTHORIZED, Html(BODY))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_state_roundtrip() {
        let original = serde_json::json!({
            "project_id": "11111111-1111-1111-1111-111111111111",
            "org_id": null,
            "return_to": "/project/abc"
        });
        let encoded = URL_SAFE_NO_PAD.encode(original.to_string().as_bytes());
        let parsed = decode_state(Some(&encoded));
        assert_eq!(
            parsed.project_id,
            Some(Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap())
        );
        assert_eq!(parsed.return_to.as_deref(), Some("/project/abc"));
    }

    #[test]
    fn decode_state_missing_is_default() {
        let empty = decode_state(None);
        assert!(empty.project_id.is_none());
        assert!(empty.return_to.is_none());

        let garbage = decode_state(Some("not-base64!!"));
        assert!(garbage.project_id.is_none());
    }

    #[test]
    fn sanitize_return_to_rejects_external() {
        assert_eq!(
            sanitize_return_to("/project/123"),
            Some("/project/123".into())
        );
        assert_eq!(sanitize_return_to("//evil.example.com"), None);
        assert_eq!(sanitize_return_to("https://evil.example.com"), None);
        assert_eq!(sanitize_return_to("javascript:alert(1)"), None);
    }
}
