use stem_cell::system_api;
use stem_cell::{integrations, migrate, proxy, resource_api, systems};

mod auth;
mod email;
mod events;
mod migrate_auth;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use axum::response::IntoResponse;
use tower_http::cors::{Any, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use utoipa_scalar::{Scalar, Servable};

use auth::AppState;
use auth::config::{AuthConfig, SmtpConfig};
use email::EmailService;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "stem_cell=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&db_url).await?;

    migrate_auth::migrate_auth(&pool).await?;
    tracing::info!("auth migrations applied");

    migrate(&pool).await?;
    tracing::info!("resource migrations applied");

    let auth_config = AuthConfig::from_env();

    let email_service = match SmtpConfig::from_env() {
        Some(smtp_config) => match EmailService::new(&smtp_config) {
            Ok(svc) => {
                tracing::info!(host = %smtp_config.host, "SMTP email service configured");
                Some(svc)
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to initialize SMTP — email disabled");
                None
            }
        },
        None => {
            tracing::info!("SMTP not configured — email disabled");
            None
        }
    };

    if auth_config.github.is_some() {
        tracing::info!("GitHub OAuth configured");
    }
    if auth_config.google.is_some() {
        tracing::info!("Google OAuth configured");
    }

    let state = AppState {
        pool: pool.clone(),
        auth_config: Arc::new(auth_config),
        email: email_service,
    };

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "4200".into())
        .parse()
        .expect("PORT must be a valid u16");

    let serve_dir = std::env::var("SERVE_DIR").unwrap_or_else(|_| "public".into());

    // Generated CRUD API uses PgPool state — resolve it before merging
    let (api, openapi) = resource_api::router().split_for_parts();
    let api = api
        .layer(axum::middleware::from_fn(resource_api::api_key_auth))
        .with_state(pool.clone());

    // Auth routes use AppState — resolve it before merging
    let auth_routes = auth::router().with_state(state);

    // System endpoints — declarative workflows from systems.yaml
    let system_routes = system_api::router(
        pool.clone(),
        integrations::AppIntegrations,
        systems::AppSystems,
    );

    // Reverse proxy to spawned child environments
    let env_proxy = proxy::router(pool.clone());

    // OpenCode process manager — one server per project
    let oc_config = opencode_client::ProcessManagerConfig::from_env();
    let oc_manager = std::sync::Arc::new(opencode_client::ProcessManager::new(oc_config));
    systems::run_build::init_process_manager((*oc_manager).clone());
    oc_manager.spawn_reaper();
    tracing::info!("OpenCode process manager initialized");

    // SSE endpoint for streaming build events to the frontend
    let event_routes = events::router();

    // Health endpoints use PgPool state (consumes pool — must be last clone)
    let health_routes = Router::new()
        .route("/healthz", get(resource_api::healthz))
        .route("/readyz", get(resource_api::readyz))
        .with_state(pool);

    let cors = if let Ok(origins) = std::env::var("ALLOWED_ORIGINS") {
        let parsed: Vec<axum::http::HeaderValue> = origins
            .split(',')
            .filter_map(|o| o.trim().parse().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(parsed)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    };

    // SPA fallback: /project/{uuid} → serve the static project page
    let project_page = serve_dir.clone();
    let spa_fallback = Router::new().route(
        "/project/{id}",
        get(move || async move {
            let path = std::path::Path::new(&project_page).join("project/index.html");
            match tokio::fs::read(path).await {
                Ok(bytes) => (
                    [(axum::http::header::CACHE_CONTROL, "no-cache, no-store, must-revalidate")],
                    axum::response::Html(bytes),
                ).into_response(),
                Err(_) => axum::http::StatusCode::NOT_FOUND.into_response(),
            }
        }),
    );

    let app = Router::new()
        .merge(api)
        .merge(system_routes)
        .merge(auth_routes)
        .merge(health_routes)
        .merge(env_proxy)
        .merge(event_routes)
        .merge(spa_fallback)
        .merge(Scalar::with_url("/api/docs", openapi))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(PropagateRequestIdLayer::x_request_id())
        .fallback_service(ServeDir::new(&serve_dir));

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(port, "listening");
    tracing::info!(port, "api docs at /api/docs");
    axum::serve(listener, app).await?;

    Ok(())
}
