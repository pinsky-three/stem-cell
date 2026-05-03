use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration, Utc};
use rand::RngCore;
use sqlx::PgPool;
use uuid::Uuid;

use super::models::{Account, OAuthLink, PasswordResetToken, Session, VerificationToken};

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

// ── Accounts ────────────────────────────────────────────────────────────

pub async fn create_account(
    pool: &PgPool,
    email: &str,
    password_hash: Option<&str>,
) -> Result<Account, sqlx::Error> {
    sqlx::query_as::<_, Account>(
        r#"INSERT INTO accounts (email, password_hash)
           VALUES ($1, $2)
           RETURNING *"#,
    )
    .bind(email)
    .bind(password_hash)
    .fetch_one(pool)
    .await
}

pub async fn find_account_by_email(
    pool: &PgPool,
    email: &str,
) -> Result<Option<Account>, sqlx::Error> {
    sqlx::query_as::<_, Account>("SELECT * FROM accounts WHERE email = $1")
        .bind(email)
        .fetch_optional(pool)
        .await
}

pub async fn find_account_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Account>, sqlx::Error> {
    sqlx::query_as::<_, Account>("SELECT * FROM accounts WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn mark_email_verified(pool: &PgPool, account_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE accounts SET email_verified = true, updated_at = now() WHERE id = $1")
        .bind(account_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_password_hash(
    pool: &PgPool,
    account_id: Uuid,
    password_hash: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE accounts SET password_hash = $1, updated_at = now() WHERE id = $2")
        .bind(password_hash)
        .bind(account_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Promote (or demote) an account to a role. No-ops if the role already matches.
pub async fn set_account_role(
    pool: &PgPool,
    account_id: Uuid,
    role: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE accounts SET role = $1, updated_at = now() WHERE id = $2 AND role IS DISTINCT FROM $1",
    )
    .bind(role)
    .bind(account_id)
    .execute(pool)
    .await?;
    Ok(())
}

// ── Sessions ────────────────────────────────────────────────────────────

pub async fn create_session(
    pool: &PgPool,
    account_id: Uuid,
    ttl_hours: i64,
) -> Result<Session, sqlx::Error> {
    let token = generate_token();
    let expires_at = Utc::now() + Duration::hours(ttl_hours);

    sqlx::query_as::<_, Session>(
        r#"INSERT INTO sessions (account_id, token, expires_at)
           VALUES ($1, $2, $3)
           RETURNING *"#,
    )
    .bind(account_id)
    .bind(&token)
    .bind(expires_at)
    .fetch_one(pool)
    .await
}

pub async fn find_valid_session(
    pool: &PgPool,
    token: &str,
) -> Result<Option<Session>, sqlx::Error> {
    sqlx::query_as::<_, Session>("SELECT * FROM sessions WHERE token = $1 AND expires_at > now()")
        .bind(token)
        .fetch_optional(pool)
        .await
}

pub async fn delete_session(pool: &PgPool, token: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE token = $1")
        .bind(token)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_expired_sessions(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM sessions WHERE expires_at <= now()")
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

// ── OAuth Links ─────────────────────────────────────────────────────────

pub async fn upsert_oauth_link(
    pool: &PgPool,
    account_id: Uuid,
    provider: &str,
    provider_user_id: &str,
    username: Option<&str>,
    access_token: Option<&str>,
    refresh_token: Option<&str>,
) -> Result<OAuthLink, sqlx::Error> {
    sqlx::query_as::<_, OAuthLink>(
        r#"INSERT INTO oauth_links (account_id, provider, provider_user_id, username, access_token, refresh_token)
           VALUES ($1, $2, $3, $4, $5, $6)
           ON CONFLICT (provider, provider_user_id)
           DO UPDATE SET username      = COALESCE(EXCLUDED.username, oauth_links.username),
                         access_token  = EXCLUDED.access_token,
                         refresh_token = COALESCE(EXCLUDED.refresh_token, oauth_links.refresh_token)
           RETURNING *"#,
    )
    .bind(account_id)
    .bind(provider)
    .bind(provider_user_id)
    .bind(username)
    .bind(access_token)
    .bind(refresh_token)
    .fetch_one(pool)
    .await
}

pub async fn find_oauth_link(
    pool: &PgPool,
    provider: &str,
    provider_user_id: &str,
) -> Result<Option<OAuthLink>, sqlx::Error> {
    sqlx::query_as::<_, OAuthLink>(
        "SELECT * FROM oauth_links WHERE provider = $1 AND provider_user_id = $2",
    )
    .bind(provider)
    .bind(provider_user_id)
    .fetch_optional(pool)
    .await
}

// ── Email verification tokens ───────────────────────────────────────────

pub async fn create_verification_token(
    pool: &PgPool,
    account_id: Uuid,
) -> Result<VerificationToken, sqlx::Error> {
    let token = generate_token();
    let expires_at = Utc::now() + Duration::hours(24);

    sqlx::query("DELETE FROM email_verification_tokens WHERE account_id = $1")
        .bind(account_id)
        .execute(pool)
        .await?;

    sqlx::query_as::<_, VerificationToken>(
        r#"INSERT INTO email_verification_tokens (account_id, token, expires_at)
           VALUES ($1, $2, $3)
           RETURNING *"#,
    )
    .bind(account_id)
    .bind(&token)
    .bind(expires_at)
    .fetch_one(pool)
    .await
}

pub async fn find_valid_verification_token(
    pool: &PgPool,
    token: &str,
) -> Result<Option<VerificationToken>, sqlx::Error> {
    sqlx::query_as::<_, VerificationToken>(
        "SELECT * FROM email_verification_tokens WHERE token = $1 AND expires_at > now()",
    )
    .bind(token)
    .fetch_optional(pool)
    .await
}

pub async fn delete_verification_token(pool: &PgPool, token: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM email_verification_tokens WHERE token = $1")
        .bind(token)
        .execute(pool)
        .await?;
    Ok(())
}

// ── Password reset tokens ───────────────────────────────────────────────

pub async fn create_password_reset_token(
    pool: &PgPool,
    account_id: Uuid,
) -> Result<PasswordResetToken, sqlx::Error> {
    let token = generate_token();
    let expires_at = Utc::now() + Duration::hours(1);

    sqlx::query("DELETE FROM password_reset_tokens WHERE account_id = $1")
        .bind(account_id)
        .execute(pool)
        .await?;

    sqlx::query_as::<_, PasswordResetToken>(
        r#"INSERT INTO password_reset_tokens (account_id, token, expires_at)
           VALUES ($1, $2, $3)
           RETURNING *"#,
    )
    .bind(account_id)
    .bind(&token)
    .bind(expires_at)
    .fetch_one(pool)
    .await
}

pub async fn find_valid_password_reset_token(
    pool: &PgPool,
    token: &str,
) -> Result<Option<PasswordResetToken>, sqlx::Error> {
    sqlx::query_as::<_, PasswordResetToken>(
        "SELECT * FROM password_reset_tokens WHERE token = $1 AND expires_at > now()",
    )
    .bind(token)
    .fetch_optional(pool)
    .await
}

pub async fn delete_password_reset_token(pool: &PgPool, token: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM password_reset_tokens WHERE token = $1")
        .bind(token)
        .execute(pool)
        .await?;
    Ok(())
}
