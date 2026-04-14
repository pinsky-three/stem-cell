use axum::{
    Router,
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
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

    let target = format!("http://127.0.0.1:{port}/{path}");

    let method = req.method().clone();
    let mut headers = req.headers().clone();
    strip_hop_by_hop(&mut headers);

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
    let is_html = resp_headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.contains("text/html"))
        .unwrap_or(false);

    let body_bytes = match upstream.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return (StatusCode::BAD_GATEWAY, format!("body read: {e}")).into_response();
        }
    };

    let final_body = if is_html {
        inject_base_tag(&body_bytes, deployment_id)
    } else {
        body_bytes.to_vec()
    };

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

/// Inject `<base href="...">` right after `<head>` so relative paths resolve
/// through the proxy prefix.
fn inject_base_tag(html_bytes: &[u8], deployment_id: uuid::Uuid) -> Vec<u8> {
    let html = String::from_utf8_lossy(html_bytes);
    let base_tag = format!(r#"<base href="/env/{deployment_id}/">"#);

    if let Some(pos) = html.find("<head>") {
        let insert_at = pos + "<head>".len();
        let mut result = String::with_capacity(html.len() + base_tag.len());
        result.push_str(&html[..insert_at]);
        result.push_str(&base_tag);
        result.push_str(&html[insert_at..]);
        result.into_bytes()
    } else if let Some(pos) = html.find("<HEAD>") {
        let insert_at = pos + "<HEAD>".len();
        let mut result = String::with_capacity(html.len() + base_tag.len());
        result.push_str(&html[..insert_at]);
        result.push_str(&base_tag);
        result.push_str(&html[insert_at..]);
        result.into_bytes()
    } else {
        html_bytes.to_vec()
    }
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
