use axum::{
    Router,
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
};
use sqlx::PgPool;

/// Build the router that proxies `/env/{id}/...` to child servers.
pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/env/{deployment_id}", any(proxy_no_slash))
        .route("/env/{deployment_id}/", any(proxy_root))
        .route("/env/{deployment_id}/{*rest}", any(proxy_handler))
        .with_state(pool)
}

async fn proxy_no_slash(
    Path(deployment_id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    axum::response::Redirect::permanent(&format!("/env/{deployment_id}/"))
}

async fn proxy_root(
    Path(deployment_id): Path<uuid::Uuid>,
    State(pool): State<PgPool>,
    req: Request,
) -> impl IntoResponse {
    do_proxy(deployment_id, "", &pool, req).await
}

async fn proxy_handler(
    Path((deployment_id, rest)): Path<(uuid::Uuid, String)>,
    State(pool): State<PgPool>,
    req: Request,
) -> impl IntoResponse {
    do_proxy(deployment_id, &rest, &pool, req).await
}

async fn do_proxy(
    deployment_id: uuid::Uuid,
    path: &str,
    pool: &PgPool,
    req: Request,
) -> Response {
    let row = sqlx::query_as::<_, (i32, bool)>(
        "SELECT port, active FROM deployments WHERE id = $1 LIMIT 1",
    )
    .bind(deployment_id)
    .fetch_optional(pool)
    .await;

    let (port, active) = match row {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "deployment not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "proxy db lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response();
        }
    };

    if !active {
        return (StatusCode::GONE, "deployment is stopped").into_response();
    }

    let target = format!("http://localhost:{port}/{path}");

    let method = req.method().clone();
    let mut headers = req.headers().clone();
    strip_hop_by_hop(&mut headers);
    // Vite's `allowedHosts` check validates the Host header; set it to localhost
    // so proxied requests from external domains (e.g. Railway) aren't rejected.
    headers.insert(
        axum::http::header::HOST,
        format!("localhost:{port}").parse().unwrap(),
    );

    let body_bytes = match axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_REQUEST, "body too large").into_response(),
    };

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();

    let upstream = client
        .request(method, &target)
        .headers(reqwest_headers(&headers))
        .body(body_bytes)
        .send()
        .await;

    let upstream = match upstream {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%deployment_id, error = %e, "proxy upstream error");
            return (StatusCode::BAD_GATEWAY, format!("upstream: {e}")).into_response();
        }
    };

    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let resp_headers = upstream.headers().clone();
    let body_bytes = match upstream.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return (StatusCode::BAD_GATEWAY, format!("body read: {e}")).into_response();
        }
    };

    let ct_str = resp_headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let final_body = rewrite_response_body(&body_bytes, deployment_id, ct_str);

    let mut response = Response::builder().status(status);
    for (key, value) in resp_headers.iter() {
        let name = key.as_str();
        if is_hop_by_hop(name) || name == "content-length" || name == "content-encoding" {
            continue;
        }
        response = response.header(key, value);
    }

    response
        .body(Body::from(final_body))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "response build failed").into_response())
}

/// Rewrite absolute paths in proxied responses so `/_astro/...`, `/favicon.png`,
/// etc. route back through `/env/{id}/…` instead of hitting the parent server.
fn rewrite_response_body(body: &[u8], deployment_id: uuid::Uuid, content_type: &str) -> Vec<u8> {
    let text = match std::str::from_utf8(body) {
        Ok(t) => t,
        Err(_) => return body.to_vec(),
    };
    let prefix = format!("/env/{deployment_id}");

    if content_type.contains("text/html") {
        let rewritten = rewrite_html_attrs(text, &prefix);
        let rewritten = rewrite_asset_refs(&rewritten, &prefix);
        rewritten.into_bytes()
    } else if content_type.contains("javascript") || content_type.contains("text/css") {
        rewrite_asset_refs(text, &prefix).into_bytes()
    } else {
        body.to_vec()
    }
}

/// Rewrite `="/<path>"` and `='/<path>'` in HTML attributes to go through the
/// proxy prefix. Protocol-relative URLs (`="//..."`) are left untouched.
fn rewrite_html_attrs(html: &str, prefix: &str) -> String {
    let mut result = String::with_capacity(html.len() + 512);
    let mut remaining = html;

    loop {
        let dq = remaining.find("=\"/");
        let sq = remaining.find("='/");

        let pos = match (dq, sq) {
            (Some(d), Some(s)) => d.min(s),
            (Some(d), None) => d,
            (None, Some(s)) => s,
            (None, None) => break,
        };

        let quote_char_len = 2; // =" or ='
        let slash_pos = pos + quote_char_len; // index of the '/'
        let after_slash = slash_pos + 1;

        // Skip protocol-relative URLs: ="//..."
        if remaining.as_bytes().get(after_slash) == Some(&b'/') {
            result.push_str(&remaining[..after_slash]);
            remaining = &remaining[after_slash..];
            continue;
        }

        // Skip if already rewritten (starts with our prefix)
        if remaining[slash_pos..].starts_with(&format!("{prefix}/")) {
            result.push_str(&remaining[..after_slash]);
            remaining = &remaining[after_slash..];
            continue;
        }

        // Rewrite: ="/<path>" → ="<prefix>/<path>"
        result.push_str(&remaining[..slash_pos]); // up to and including ="
        result.push_str(prefix);
        result.push('/');
        remaining = &remaining[after_slash..]; // skip past the original /
    }

    result.push_str(remaining);
    result
}

/// Rewrite `/_astro/` references in JS, CSS, and inline `<script>` blocks.
fn rewrite_asset_refs(text: &str, prefix: &str) -> String {
    text.replace("\"/_astro/", &format!("\"{prefix}/_astro/"))
        .replace("'/_astro/", &format!("'{prefix}/_astro/"))
        .replace("`/_astro/", &format!("`{prefix}/_astro/"))
        .replace("(/_astro/", &format!("({prefix}/_astro/")) // url() in CSS
}

fn reqwest_headers(headers: &HeaderMap) -> reqwest::header::HeaderMap {
    let mut out = reqwest::header::HeaderMap::new();
    for (key, value) in headers.iter() {
        if let Ok(name) = reqwest::header::HeaderName::from_bytes(key.as_str().as_bytes()) {
            if let Ok(val) = reqwest::header::HeaderValue::from_bytes(value.as_bytes()) {
                out.insert(name, val);
            }
        }
    }
    out
}

fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let remove: Vec<_> = headers
        .keys()
        .filter(|k| is_hop_by_hop(k.as_str()))
        .cloned()
        .collect();
    for key in remove {
        headers.remove(&key);
    }
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
    )
}
