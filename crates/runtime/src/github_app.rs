//! GitHub App auth primitives: JWT signing, installation access tokens,
//! webhook HMAC verification, and a thin authenticated HTTP helper.
//!
//! The three flows this module supports:
//!
//! 1. **App JWT** — RS256-signed for 10 min using the App's private key.
//!    Only useful for calling `/app/*` endpoints and minting installation
//!    tokens.
//! 2. **Installation access token** — short-lived (~1 h) bearer token scoped
//!    to one installation. Used for every repo operation. We cache tokens in
//!    memory per installation-id, refreshing ~60 s before they expire.
//! 3. **Webhook verification** — constant-time HMAC-SHA256 compare of the
//!    `X-Hub-Signature-256` header against `GITHUB_APP_WEBHOOK_SECRET`.
//!
//! The module is deliberately framework-free — it just needs `reqwest` and
//! `jsonwebtoken`. Systems call through `InstallationClient` which bundles a
//! minted token with a pre-configured `reqwest::Client`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use hmac::{Hmac, Mac};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// GitHub App configuration loaded from environment variables. Held once per
/// process; mutation happens via hot-restarting the server, not at runtime.
#[derive(Clone)]
pub struct GithubAppConfig {
    pub app_id: i64,
    /// PEM bytes of the App's private key (RS256).
    pub private_key_pem: Vec<u8>,
    pub webhook_secret: String,
    pub app_slug: Option<String>,
}

impl std::fmt::Debug for GithubAppConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubAppConfig")
            .field("app_id", &self.app_id)
            .field("private_key_pem", &"<redacted>")
            .field("webhook_secret", &"<redacted>")
            .field("app_slug", &self.app_slug)
            .finish()
    }
}

impl GithubAppConfig {
    /// Load from env. Returns None (with a single info log) if any of the
    /// required bits are missing — callers should treat that as "App mode
    /// disabled" and fall back to contract-boundary errors in the systems.
    pub fn from_env() -> Option<Self> {
        let app_id: i64 = std::env::var("GITHUB_APP_ID").ok()?.trim().parse().ok()?;
        let webhook_secret = std::env::var("GITHUB_APP_WEBHOOK_SECRET")
            .ok()
            .filter(|v| !v.is_empty())?;
        let app_slug = std::env::var("GITHUB_APP_SLUG")
            .ok()
            .filter(|v| !v.is_empty());

        let private_key_pem = match std::env::var("GITHUB_APP_PRIVATE_KEY") {
            Ok(pem) if !pem.is_empty() => {
                // Support single-line PEMs with literal "\n" escapes (handy
                // for .env files). jsonwebtoken wants real newlines.
                pem.replace("\\n", "\n").into_bytes()
            }
            _ => {
                let path = std::env::var("GITHUB_APP_PRIVATE_KEY_PATH").ok()?;
                match std::fs::read(&path) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::warn!(%path, error = %e, "could not read GITHUB_APP_PRIVATE_KEY_PATH");
                        return None;
                    }
                }
            }
        };

        Some(Self {
            app_id,
            private_key_pem,
            webhook_secret,
            app_slug,
        })
    }
}

/// Process-wide singleton. `None` means App mode is disabled; systems surface
/// that as a structured error.
pub fn config() -> Option<&'static GithubAppConfig> {
    static CFG: OnceLock<Option<GithubAppConfig>> = OnceLock::new();
    CFG.get_or_init(GithubAppConfig::from_env).as_ref()
}

/// Errors produced by GitHub App auth / HTTP operations. Systems map these
/// onto their own generated error enums.
#[derive(Debug, thiserror::Error)]
pub enum GithubAppError {
    #[error("GitHub App not configured: set GITHUB_APP_ID, GITHUB_APP_PRIVATE_KEY(_PATH), and GITHUB_APP_WEBHOOK_SECRET")]
    NotConfigured,
    #[error("failed to sign GitHub App JWT: {0}")]
    Jwt(String),
    #[error("GitHub API call failed ({status}): {body}")]
    Api { status: u16, body: String },
    #[error("HTTP transport error: {0}")]
    Transport(String),
    #[error("response could not be parsed: {0}")]
    Parse(String),
}

// ── JWT minting ────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct AppJwtClaims {
    iat: i64,
    exp: i64,
    iss: String,
}

/// Mint a 10-minute App JWT. GitHub recommends ≤ 10 min; we use 9 so clock
/// skew never pushes us over.
pub fn mint_app_jwt(cfg: &GithubAppConfig) -> Result<String, GithubAppError> {
    let now = Utc::now().timestamp();
    let claims = AppJwtClaims {
        // 60s back-date absorbs skew in the other direction.
        iat: now - 60,
        exp: now + (9 * 60),
        iss: cfg.app_id.to_string(),
    };
    let header = Header::new(jsonwebtoken::Algorithm::RS256);
    let key = EncodingKey::from_rsa_pem(&cfg.private_key_pem)
        .map_err(|e| GithubAppError::Jwt(e.to_string()))?;
    encode(&header, &claims, &key).map_err(|e| GithubAppError::Jwt(e.to_string()))
}

// ── Installation access tokens (cached) ────────────────────────────────────

#[derive(Clone)]
struct CachedToken {
    token: String,
    /// When this token is no longer safe to use. We refresh at
    /// `valid_until - REFRESH_LEEWAY`.
    valid_until: DateTime<Utc>,
}

/// Refresh tokens a minute before they actually expire so concurrent callers
/// don't race an API rejection.
const REFRESH_LEEWAY: ChronoDuration = ChronoDuration::seconds(60);

fn token_cache() -> &'static Mutex<HashMap<i64, CachedToken>> {
    static CACHE: OnceLock<Mutex<HashMap<i64, CachedToken>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Deserialize)]
struct InstallationTokenResponse {
    token: String,
    expires_at: DateTime<Utc>,
}

/// Returns a cached installation token if one is still fresh, otherwise mints
/// a new one via `POST /app/installations/{id}/access_tokens` and caches it.
pub async fn mint_installation_token(
    cfg: &GithubAppConfig,
    installation_id: i64,
) -> Result<String, GithubAppError> {
    let cached = {
        let map = token_cache().lock().expect("token cache poisoned");
        map.get(&installation_id).cloned()
    };
    if let Some(cached) = cached
        && cached.valid_until - REFRESH_LEEWAY > Utc::now()
    {
        return Ok(cached.token);
    }

    let jwt = mint_app_jwt(cfg)?;
    let url = format!(
        "https://api.github.com/app/installations/{installation_id}/access_tokens"
    );

    let res = http_client()
        .post(&url)
        .bearer_auth(&jwt)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", user_agent())
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| GithubAppError::Transport(e.to_string()))?;

    let status = res.status();
    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(GithubAppError::Api {
            status: status.as_u16(),
            body,
        });
    }

    let parsed: InstallationTokenResponse = res
        .json()
        .await
        .map_err(|e| GithubAppError::Parse(e.to_string()))?;

    {
        let mut map = token_cache().lock().expect("token cache poisoned");
        map.insert(
            installation_id,
            CachedToken {
                token: parsed.token.clone(),
                valid_until: parsed.expires_at,
            },
        );
    }

    Ok(parsed.token)
}

/// Drop the cached token for an installation. Call this after a 401 from any
/// installation-scoped request so the next attempt re-mints.
pub fn invalidate_installation_token(installation_id: i64) {
    if let Ok(mut map) = token_cache().lock() {
        map.remove(&installation_id);
    }
}

// ── HTTP helper ────────────────────────────────────────────────────────────

pub fn user_agent() -> &'static str {
    "stem-cell-github-app"
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(user_agent())
            .build()
            .expect("build reqwest client")
    })
}

/// Authenticated client bundle for one installation. Handed to system impls
/// so they don't each re-implement token minting + header plumbing.
pub struct InstallationClient {
    pub token: String,
    pub installation_id: i64,
}

impl InstallationClient {
    pub async fn for_installation(installation_id: i64) -> Result<Self, GithubAppError> {
        let cfg = config().ok_or(GithubAppError::NotConfigured)?;
        let token = mint_installation_token(cfg, installation_id).await?;
        Ok(Self { token, installation_id })
    }

    pub fn get(&self, url: impl reqwest::IntoUrl) -> reqwest::RequestBuilder {
        http_client()
            .get(url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }

    pub fn post(&self, url: impl reqwest::IntoUrl) -> reqwest::RequestBuilder {
        http_client()
            .post(url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }

    /// Bearer token formatted for a Git HTTP push URL:
    /// `https://x-access-token:<tok>@github.com/<owner>/<repo>.git`.
    pub fn git_https_url(&self, owner: &str, repo: &str) -> String {
        format!(
            "https://x-access-token:{}@github.com/{}/{}.git",
            self.token, owner, repo
        )
    }
}

// ── App-JWT client (for `/app/*` endpoints) ────────────────────────────────

/// Client authenticated as the App itself (not a specific installation). Used
/// for `GET /app/installations/:id`.
pub struct AppClient {
    pub jwt: String,
}

impl AppClient {
    pub fn new() -> Result<Self, GithubAppError> {
        let cfg = config().ok_or(GithubAppError::NotConfigured)?;
        Ok(Self {
            jwt: mint_app_jwt(cfg)?,
        })
    }

    pub fn get(&self, url: impl reqwest::IntoUrl) -> reqwest::RequestBuilder {
        http_client()
            .get(url)
            .header("Authorization", format!("Bearer {}", self.jwt))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }
}

// ── Webhook signature verification ─────────────────────────────────────────

/// Verify the raw body against `X-Hub-Signature-256: sha256=<hex>` using
/// constant-time compare. Returns true on match.
pub fn verify_webhook_signature(secret: &str, body: &[u8], signature_header: &str) -> bool {
    let Some(sig_hex) = signature_header.strip_prefix("sha256=") else {
        return false;
    };
    let Ok(sig) = hex_decode(sig_hex) else {
        return false;
    };

    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    let expected = mac.finalize().into_bytes();

    if sig.len() != expected.len() {
        return false;
    }
    expected.as_slice().ct_eq(&sig).unwrap_u8() == 1
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    if !s.len().is_multiple_of(2) {
        return Err(());
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for pair in s.as_bytes().chunks(2) {
        let h = hex_nibble(pair[0])?;
        let l = hex_nibble(pair[1])?;
        out.push((h << 4) | l);
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, ()> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(10 + b - b'a'),
        b'A'..=b'F' => Ok(10 + b - b'A'),
        _ => Err(()),
    }
}

// ── Pre-built Arc wrapper so main can share the config handle ──────────────

/// Type alias for the shared config handle passed into the webhook router.
pub type SharedGithubAppConfig = Arc<GithubAppConfig>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_roundtrip() {
        let body = b"{\"action\":\"created\"}";
        let secret = "hunter2";
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let sig = mac.finalize().into_bytes();
        let hex_sig: String = sig.iter().map(|b| format!("{b:02x}")).collect();
        let header = format!("sha256={hex_sig}");
        assert!(verify_webhook_signature(secret, body, &header));
        assert!(!verify_webhook_signature("wrong", body, &header));
        assert!(!verify_webhook_signature(secret, body, "sha256=deadbeef"));
        assert!(!verify_webhook_signature(secret, body, "not-a-signature"));
    }

    #[test]
    fn hex_decoder_rejects_odd() {
        assert!(hex_decode("abc").is_err());
        assert!(hex_decode("zz").is_err());
        assert_eq!(hex_decode("ff").unwrap(), vec![0xff]);
    }
}
