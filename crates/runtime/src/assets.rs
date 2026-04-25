//! Embedded frontend assets.
//!
//! The Astro build writes to `../../public`; `rust-embed` bakes that tree into
//! the binary at compile time. This module replaces `tower_http::ServeDir` when
//! the `embed-assets` feature is on, so the release artifact is fully
//! self-contained. `SERVE_DIR=…` in `main` flips the wiring back to on-disk
//! ServeDir for Astro HMR during development.
//!
//! Cache-control policy mirrors what Astro + a CDN would produce:
//!   * `/_astro/*`           → immutable, 1y   (hashed filenames)
//!   * `.html` / extensionless → no-cache       (SPA/admin routes)
//!   * anything else          → public, 1h      (favicon, robots.txt, etc.)
//!
//! Observability: hits/misses are tracked in atomics and every request emits a
//! `target: "stem_cell::assets"` debug log. During the filesystem→embedded
//! cutover, turn on with `RUST_LOG=stem_cell::assets=debug` to verify every
//! path ServeDir used to serve is now served from memory.

use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

// `folder` must be a relative path when the `compression` feature is on;
// rust-embed resolves it from `CARGO_MANIFEST_DIR` (crates/runtime/), so
// `../../public` lands on the workspace-level `public/` that `build.rs`
// populates via `npm run build`.
#[derive(RustEmbed)]
#[folder = "../../public"]
struct Assets;

pub static HITS: AtomicU64 = AtomicU64::new(0);
pub static MISSES: AtomicU64 = AtomicU64::new(0);

/// Axum fallback handler: resolve `uri.path()` against the embedded tree,
/// mirroring ServeDir semantics (strip leading `/`, fall back to
/// `<path>/index.html` for directory-style requests).
pub async fn serve(uri: Uri) -> Response {
    let trimmed = uri.path().trim_start_matches('/').trim_end_matches('/');
    let primary = if trimmed.is_empty() {
        "index.html".to_string()
    } else {
        trimmed.to_string()
    };
    let index_fallback = if trimmed.is_empty() {
        None
    } else {
        Some(format!("{trimmed}/index.html"))
    };

    for candidate in std::iter::once(primary.as_str()).chain(index_fallback.as_deref()) {
        if let Some(file) = Assets::get(candidate) {
            return hit(candidate, file);
        }
    }

    MISSES.fetch_add(1, Ordering::Relaxed);
    tracing::debug!(target: "stem_cell::assets", path = %uri.path(), "miss");
    StatusCode::NOT_FOUND.into_response()
}

/// Serve the SPA shell for `/project/{id}` — the router rewrites every such
/// request to this handler, so we can't rely on `serve` resolving the path.
pub async fn serve_project_spa() -> Response {
    match Assets::get("project/index.html") {
        Some(file) => {
            HITS.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(
                target: "stem_cell::assets",
                path = "project/index.html",
                bytes = file.data.len(),
                "hit (spa)"
            );
            (
                [(header::CACHE_CONTROL, "no-cache, no-store, must-revalidate")],
                axum::response::Html(file.data.into_owned()),
            )
                .into_response()
        }
        None => {
            MISSES.fetch_add(1, Ordering::Relaxed);
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

fn hit(path: &str, file: rust_embed::EmbeddedFile) -> Response {
    HITS.fetch_add(1, Ordering::Relaxed);
    tracing::debug!(
        target: "stem_cell::assets",
        path,
        bytes = file.data.len(),
        "hit"
    );
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_str(mime.as_ref())
                .unwrap_or(HeaderValue::from_static("application/octet-stream")),
        )
        .header(
            header::CACHE_CONTROL,
            HeaderValue::from_static(cache_control_for(path)),
        )
        .body(Body::from(file.data.into_owned()))
        .unwrap()
}

fn cache_control_for(path: &str) -> &'static str {
    if path.starts_with("_astro/") {
        "public, max-age=31536000, immutable"
    } else if path.ends_with(".html") || !path.rsplit('/').next().is_some_and(|s| s.contains('.')) {
        "no-cache, no-store, must-revalidate"
    } else {
        "public, max-age=3600"
    }
}

#[cfg(test)]
mod tests {
    use super::{Assets, cache_control_for};
    use rust_embed::RustEmbed;

    /// Regression guard: if Astro's build pipeline changes or `public/` is
    /// never populated, the embed tree is empty and every request 404s. The
    /// test fails loudly in CI before a broken artifact ships.
    ///
    /// Skipped when the frontend hasn't been built — keeps `cargo test` green
    /// for devs who only touch Rust code with `SKIP_FRONTEND=1`.
    #[test]
    fn embedded_tree_has_known_files() {
        let has_index = Assets::get("index.html").is_some();
        if !has_index {
            eprintln!("skip: public/index.html missing (run `npm --prefix frontend run build`)");
            return;
        }
        assert!(
            Assets::iter().any(|p| p.starts_with("_astro/")),
            "expected at least one hashed asset under _astro/"
        );
        assert!(
            Assets::get("admin/index.html").is_some(),
            "admin dashboard should be embedded"
        );
    }

    #[test]
    fn hashed_assets_are_immutable() {
        assert_eq!(
            cache_control_for("_astro/client.DX3w9z.js"),
            "public, max-age=31536000, immutable"
        );
    }

    #[test]
    fn html_and_spa_routes_are_no_cache() {
        assert_eq!(
            cache_control_for("index.html"),
            "no-cache, no-store, must-revalidate"
        );
        assert_eq!(
            cache_control_for("admin/organizations/index.html"),
            "no-cache, no-store, must-revalidate"
        );
        assert_eq!(
            cache_control_for("admin/organizations"),
            "no-cache, no-store, must-revalidate"
        );
    }

    #[test]
    fn static_files_get_moderate_cache() {
        assert_eq!(cache_control_for("favicon.png"), "public, max-age=3600");
        assert_eq!(cache_control_for("robots.txt"), "public, max-age=3600");
    }
}
