use sqlx::PgPool;

pub async fn migrate_auth(pool: &PgPool) -> Result<(), sqlx::Error> {
    // raw_sql supports multiple statements; sqlx::query does not.
    sqlx::raw_sql(
        r#"
        CREATE TABLE IF NOT EXISTS accounts (
            id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            email       TEXT NOT NULL UNIQUE,
            password_hash TEXT,
            email_verified BOOLEAN NOT NULL DEFAULT false,
            created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
        );

        CREATE TABLE IF NOT EXISTS oauth_links (
            id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            account_id        UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            provider          TEXT NOT NULL,
            provider_user_id  TEXT NOT NULL,
            access_token      TEXT,
            refresh_token     TEXT,
            created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
            UNIQUE(provider, provider_user_id)
        );

        CREATE TABLE IF NOT EXISTS sessions (
            id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            account_id  UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            token       TEXT NOT NULL UNIQUE,
            expires_at  TIMESTAMPTZ NOT NULL,
            created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
        );

        CREATE TABLE IF NOT EXISTS email_verification_tokens (
            id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            account_id  UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            token       TEXT NOT NULL UNIQUE,
            expires_at  TIMESTAMPTZ NOT NULL,
            created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
        );

        CREATE TABLE IF NOT EXISTS password_reset_tokens (
            id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            account_id  UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            token       TEXT NOT NULL UNIQUE,
            expires_at  TIMESTAMPTZ NOT NULL,
            created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
        );

        -- RBAC: every session-bearing account carries a single role string.
        -- 'user' is the default; 'admin' gates /admin/* routes.
        -- Column is added idempotently so existing DBs keep working.
        ALTER TABLE accounts
            ADD COLUMN IF NOT EXISTS role TEXT NOT NULL DEFAULT 'user';

        -- Capture the provider's display handle (e.g. GitHub login) so we can
        -- authorize by username without a fresh API call on every request and
        -- render it in audit logs.
        ALTER TABLE oauth_links
            ADD COLUMN IF NOT EXISTS username TEXT;
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}
