/// Typed auth configuration loaded from environment variables.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub app_url: String,
    pub session_ttl_hours: i64,
    pub github: Option<OAuthProviderConfig>,
    pub google: Option<OAuthProviderConfig>,
    /// GitHub logins that are auto-promoted to `role = 'admin'` on OAuth
    /// callback. Sourced from `ADMIN_GITHUB_USERNAMES` (comma-separated).
    /// Matched case-insensitively. Kept here rather than hard-coded so the
    /// bootstrap is reversible without redeploys.
    pub admin_github_usernames: Vec<String>,
}

impl AuthConfig {
    pub fn is_admin_github_user(&self, login: &str) -> bool {
        self.admin_github_usernames
            .iter()
            .any(|u| u.eq_ignore_ascii_case(login))
    }
}

#[derive(Debug, Clone)]
pub struct OAuthProviderConfig {
    pub client_id: String,
    pub client_secret: String,
}

/// SMTP configuration loaded from environment variables.
#[derive(Debug, Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from: String,
}

impl AuthConfig {
    pub fn from_env() -> Self {
        let app_url =
            std::env::var("APP_URL").unwrap_or_else(|_| "http://localhost:4200".to_string());

        let session_ttl_hours: i64 = std::env::var("SESSION_TTL_HOURS")
            .unwrap_or_else(|_| "168".to_string())
            .parse()
            .expect("SESSION_TTL_HOURS must be a valid i64");

        let github = match (
            std::env::var("GITHUB_CLIENT_ID"),
            std::env::var("GITHUB_CLIENT_SECRET"),
        ) {
            (Ok(id), Ok(secret)) if !id.is_empty() && !secret.is_empty() => {
                Some(OAuthProviderConfig {
                    client_id: id,
                    client_secret: secret,
                })
            }
            _ => None,
        };

        let google = match (
            std::env::var("GOOGLE_CLIENT_ID"),
            std::env::var("GOOGLE_CLIENT_SECRET"),
        ) {
            (Ok(id), Ok(secret)) if !id.is_empty() && !secret.is_empty() => {
                Some(OAuthProviderConfig {
                    client_id: id,
                    client_secret: secret,
                })
            }
            _ => None,
        };

        let admin_github_usernames = std::env::var("ADMIN_GITHUB_USERNAMES")
            .unwrap_or_else(|_| "bregydoc".to_string())
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Self {
            app_url,
            session_ttl_hours,
            github,
            google,
            admin_github_usernames,
        }
    }
}

impl SmtpConfig {
    pub fn from_env() -> Option<Self> {
        let host = std::env::var("SMTP_HOST").ok()?;
        if host.is_empty() {
            return None;
        }

        Some(Self {
            host,
            port: std::env::var("SMTP_PORT")
                .unwrap_or_else(|_| "587".to_string())
                .parse()
                .expect("SMTP_PORT must be a valid u16"),
            username: std::env::var("SMTP_USERNAME").unwrap_or_default(),
            password: std::env::var("SMTP_PASSWORD").unwrap_or_default(),
            from: std::env::var("SMTP_FROM").unwrap_or_else(|_| "noreply@example.com".to_string()),
        })
    }
}
