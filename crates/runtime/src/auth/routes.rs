use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::Cookie;
use serde::Deserialize;

use super::AppState;
use super::middleware::CurrentAccount;
use super::models::{
    AccountPublic, AuthResponse, ForgotPasswordRequest, LoginRequest, RegisterRequest,
    ResetPasswordRequest,
};
use super::password;
use super::repository;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("email already registered")]
    EmailTaken,
    #[error("invalid or expired token")]
    InvalidToken,
    #[error("password is required")]
    PasswordRequired,
    #[error("{0}")]
    Internal(String),
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            AuthError::InvalidCredentials => (StatusCode::UNAUTHORIZED, self.to_string()),
            AuthError::EmailTaken => (StatusCode::CONFLICT, self.to_string()),
            AuthError::InvalidToken => (StatusCode::BAD_REQUEST, self.to_string()),
            AuthError::PasswordRequired => (StatusCode::UNPROCESSABLE_ENTITY, self.to_string()),
            AuthError::Internal(m) => {
                tracing::error!(error = %m, "auth internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
        };
        (status, Json(serde_json::json!({"error": msg}))).into_response()
    }
}

pub fn session_cookie(token: &str, max_age_hours: i64) -> Cookie<'static> {
    Cookie::build(("session_token", token.to_string()))
        .path("/")
        .http_only(true)
        .same_site(axum_extra::extract::cookie::SameSite::Lax)
        .max_age(time::Duration::hours(max_age_hours))
        .build()
}

fn clear_session_cookie() -> Cookie<'static> {
    Cookie::build(("session_token", ""))
        .path("/")
        .http_only(true)
        .max_age(time::Duration::ZERO)
        .build()
}

// ── POST /auth/register ─────────────────────────────────────────────────

pub async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> Result<(CookieJar, Json<AuthResponse>), AuthError> {
    if body.password.len() < 8 {
        return Err(AuthError::PasswordRequired);
    }

    let hash =
        password::hash_password(&body.password).map_err(|e| AuthError::Internal(e.to_string()))?;

    let account = repository::create_account(&state.pool, &body.email, Some(&hash))
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref db_err) = e {
                if db_err.code().as_deref() == Some("23505") {
                    return AuthError::EmailTaken;
                }
            }
            AuthError::Internal(e.to_string())
        })?;

    let session =
        repository::create_session(&state.pool, account.id, state.auth_config.session_ttl_hours)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

    if let Some(ref email_svc) = state.email {
        let vtoken = repository::create_verification_token(&state.pool, account.id)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        if let Err(e) = email_svc
            .send_verification(&account.email, &state.auth_config.app_url, &vtoken.token)
            .await
        {
            tracing::warn!(error = %e, "failed to send verification email");
        }
    }

    let jar = CookieJar::new().add(session_cookie(
        &session.token,
        state.auth_config.session_ttl_hours,
    ));

    Ok((
        jar,
        Json(AuthResponse {
            account: AccountPublic::from(account),
        }),
    ))
}

// ── POST /auth/login ────────────────────────────────────────────────────

pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Result<(CookieJar, Json<AuthResponse>), AuthError> {
    let account = repository::find_account_by_email(&state.pool, &body.email)
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?
        .ok_or(AuthError::InvalidCredentials)?;

    let hash = account
        .password_hash
        .as_deref()
        .ok_or(AuthError::InvalidCredentials)?;

    let valid = password::verify_password(&body.password, hash)
        .map_err(|_| AuthError::InvalidCredentials)?;

    if !valid {
        return Err(AuthError::InvalidCredentials);
    }

    let session =
        repository::create_session(&state.pool, account.id, state.auth_config.session_ttl_hours)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

    let jar = CookieJar::new().add(session_cookie(
        &session.token,
        state.auth_config.session_ttl_hours,
    ));

    Ok((
        jar,
        Json(AuthResponse {
            account: AccountPublic::from(account),
        }),
    ))
}

// ── POST /auth/logout ───────────────────────────────────────────────────

pub async fn logout(State(state): State<AppState>, jar: CookieJar) -> Result<CookieJar, AuthError> {
    if let Some(cookie) = jar.get("session_token") {
        let _ = repository::delete_session(&state.pool, cookie.value()).await;
    }
    Ok(CookieJar::new().add(clear_session_cookie()))
}

// ── GET /auth/me ────────────────────────────────────────────────────────

pub async fn me(CurrentAccount(account): CurrentAccount) -> Json<AuthResponse> {
    Json(AuthResponse {
        account: AccountPublic::from(account),
    })
}

// ── GET /auth/verify-email?token=... ────────────────────────────────────

#[derive(Deserialize)]
pub struct VerifyEmailQuery {
    pub token: String,
}

pub async fn verify_email(
    State(state): State<AppState>,
    Query(q): Query<VerifyEmailQuery>,
) -> Result<Redirect, AuthError> {
    let vtoken = repository::find_valid_verification_token(&state.pool, &q.token)
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?
        .ok_or(AuthError::InvalidToken)?;

    repository::mark_email_verified(&state.pool, vtoken.account_id)
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?;

    repository::delete_verification_token(&state.pool, &q.token)
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?;

    if let Some(ref email_svc) = state.email {
        if let Some(account) = repository::find_account_by_id(&state.pool, vtoken.account_id)
            .await
            .ok()
            .flatten()
        {
            let _ = email_svc.send_welcome(&account.email, &account.email).await;
        }
    }

    Ok(Redirect::to("/login?verified=true"))
}

// ── POST /auth/forgot-password ──────────────────────────────────────────

pub async fn forgot_password(
    State(state): State<AppState>,
    Json(body): Json<ForgotPasswordRequest>,
) -> StatusCode {
    let Ok(Some(account)) = repository::find_account_by_email(&state.pool, &body.email).await
    else {
        return StatusCode::OK;
    };

    if let Some(ref email_svc) = state.email {
        if let Ok(reset_token) =
            repository::create_password_reset_token(&state.pool, account.id).await
        {
            if let Err(e) = email_svc
                .send_password_reset(
                    &account.email,
                    &state.auth_config.app_url,
                    &reset_token.token,
                )
                .await
            {
                tracing::warn!(error = %e, "failed to send password reset email");
            }
        }
    }

    StatusCode::OK
}

// ── POST /auth/reset-password ───────────────────────────────────────────

pub async fn reset_password(
    State(state): State<AppState>,
    Json(body): Json<ResetPasswordRequest>,
) -> Result<StatusCode, AuthError> {
    if body.password.len() < 8 {
        return Err(AuthError::PasswordRequired);
    }

    let reset_token = repository::find_valid_password_reset_token(&state.pool, &body.token)
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?
        .ok_or(AuthError::InvalidToken)?;

    let hash =
        password::hash_password(&body.password).map_err(|e| AuthError::Internal(e.to_string()))?;

    repository::update_password_hash(&state.pool, reset_token.account_id, &hash)
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?;

    repository::delete_password_reset_token(&state.pool, &body.token)
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}
