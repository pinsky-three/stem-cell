pub mod admin_guard;
pub mod config;
pub mod middleware;
pub mod models;
pub mod oauth;
pub mod password;
pub mod repository;
pub mod routes;

use std::sync::Arc;

use axum::Router;
use axum::extract::FromRef;
use axum::routing::{get, post};

use crate::email::EmailService;
use config::AuthConfig;

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
    pub auth_config: Arc<AuthConfig>,
    pub email: Option<EmailService>,
}

impl FromRef<AppState> for sqlx::PgPool {
    fn from_ref(state: &AppState) -> Self {
        state.pool.clone()
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/register", post(routes::register))
        .route("/auth/login", post(routes::login))
        .route("/auth/logout", post(routes::logout))
        .route("/auth/me", get(routes::me))
        .route("/auth/verify-email", get(routes::verify_email))
        .route("/auth/forgot-password", post(routes::forgot_password))
        .route("/auth/reset-password", post(routes::reset_password))
        .route("/auth/oauth/{provider}", get(oauth::oauth_redirect))
        .route(
            "/auth/oauth/{provider}/callback",
            get(oauth::oauth_callback),
        )
}
