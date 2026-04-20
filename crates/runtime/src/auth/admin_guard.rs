//! Route guard for `/admin/*`.
//!
//! Strategy: sit as middleware in front of a `ServeDir` that points at the
//! generated admin pages. We resolve the session cookie, load the linked
//! account, and:
//!
//! - no session          → 302 to `/login?next={original_path}`
//! - session, non-admin  → 403 HTML (no redirect loop; the account IS logged in)
//! - session, admin      → pass through to the next service (ServeDir)
//!
//! Keeping the enforcement at the server edge means the generated Astro pages
//! stay oblivious to auth — they're still overwritten by codegen without risk.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::CookieJar;

use super::AppState;
use super::repository;

const FORBIDDEN_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>403 — Forbidden</title>
  <style>
    body { font-family: system-ui, -apple-system, Segoe UI, sans-serif;
           background: #0a0a0b; color: #e5e5e5; margin: 0;
           min-height: 100vh; display: grid; place-items: center; }
    .card { max-width: 440px; padding: 2rem; border: 1px solid #262626;
            border-radius: 12px; background: #111; }
    h1 { margin: 0 0 .5rem; font-size: 1.5rem; }
    p  { margin: .25rem 0; color: #a3a3a3; line-height: 1.5; }
    a  { color: #818cf8; text-decoration: none; }
    a:hover { text-decoration: underline; }
  </style>
</head>
<body>
  <div class="card">
    <h1>403 &middot; Admins only</h1>
    <p>Your account is signed in, but does not have the <code>admin</code> role required for this section.</p>
    <p><a href="/">&larr; Back home</a> &nbsp;&middot;&nbsp; <a href="/auth/logout" onclick="fetch('/auth/logout',{method:'POST'}).then(()=>location.href='/login');return false;">Sign out</a></p>
  </div>
</body>
</html>
"#;

pub async fn admin_guard(
    State(state): State<AppState>,
    jar: CookieJar,
    uri: Uri,
    req: Request,
    next: Next,
) -> Response {
    let Some(token) = jar.get("session_token").map(|c| c.value().to_string()) else {
        return redirect_to_login(&uri);
    };

    let session = match repository::find_valid_session(&state.pool, &token).await {
        Ok(Some(s)) => s,
        Ok(None) => return redirect_to_login(&uri),
        Err(e) => {
            tracing::error!(error = %e, "admin_guard: session lookup failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let account = match repository::find_account_by_id(&state.pool, session.account_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return redirect_to_login(&uri),
        Err(e) => {
            tracing::error!(error = %e, "admin_guard: account lookup failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if !account.is_admin() {
        tracing::warn!(
            account_id = %account.id,
            email = %account.email,
            role = %account.role,
            path = %uri.path(),
            "admin_guard: denied non-admin access to /admin"
        );
        return (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            Body::from(FORBIDDEN_HTML),
        )
            .into_response();
    }

    next.run(req).await
}

fn redirect_to_login(uri: &Uri) -> Response {
    // Preserve the original path+query so we can bounce back post-login.
    let next = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/admin");
    Redirect::to(&format!(
        "/login?next={}",
        urlencoding_light(next)
    ))
    .into_response()
}

/// Minimal percent-encoder for the `next` query param. Keeps this module free
/// of an extra dep; only encodes the characters we'd actually see in a path
/// (spaces, `&`, `?`, `#`, `=`).
fn urlencoding_light(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'/'
            | b':' => out.push(*b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}
