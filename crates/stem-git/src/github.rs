//! GitHub App auth primitives and high-level repository operations.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use hmac::{Hmac, Mac};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct GithubAppConfig {
    pub app_id: i64,
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
    pub fn from_env() -> Option<Self> {
        let app_id: i64 = std::env::var("GITHUB_APP_ID").ok()?.trim().parse().ok()?;
        let webhook_secret = std::env::var("GITHUB_APP_WEBHOOK_SECRET")
            .ok()
            .filter(|v| !v.is_empty())?;
        let app_slug = std::env::var("GITHUB_APP_SLUG")
            .ok()
            .filter(|v| !v.is_empty());

        let private_key_pem = match std::env::var("GITHUB_APP_PRIVATE_KEY") {
            Ok(pem) if !pem.is_empty() => pem.replace("\\n", "\n").into_bytes(),
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

pub fn config() -> Option<&'static GithubAppConfig> {
    static CFG: OnceLock<Option<GithubAppConfig>> = OnceLock::new();
    CFG.get_or_init(GithubAppConfig::from_env).as_ref()
}

#[derive(Debug, thiserror::Error)]
pub enum GithubAppError {
    #[error(
        "GitHub App not configured: set GITHUB_APP_ID, GITHUB_APP_PRIVATE_KEY(_PATH), and GITHUB_APP_WEBHOOK_SECRET"
    )]
    NotConfigured,
    #[error("failed to sign GitHub App JWT: {0}")]
    Jwt(String),
    #[error("GitHub API call failed ({status}): {body}")]
    Api { status: u16, body: String },
    #[error("HTTP transport error: {0}")]
    Transport(String),
    #[error("response could not be parsed: {0}")]
    Parse(String),
    #[error("{0}")]
    InvalidInput(String),
}

#[derive(Serialize)]
struct AppJwtClaims {
    iat: i64,
    exp: i64,
    iss: String,
}

pub fn mint_app_jwt(cfg: &GithubAppConfig) -> Result<String, GithubAppError> {
    let now = Utc::now().timestamp();
    let claims = AppJwtClaims {
        iat: now - 60,
        exp: now + (9 * 60),
        iss: cfg.app_id.to_string(),
    };
    let header = Header::new(jsonwebtoken::Algorithm::RS256);
    let key = EncodingKey::from_rsa_pem(&cfg.private_key_pem)
        .map_err(|e| GithubAppError::Jwt(e.to_string()))?;
    encode(&header, &claims, &key).map_err(|e| GithubAppError::Jwt(e.to_string()))
}

#[derive(Clone)]
struct CachedToken {
    token: String,
    valid_until: DateTime<Utc>,
}

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
    let url = format!("https://api.github.com/app/installations/{installation_id}/access_tokens");

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

pub fn invalidate_installation_token(installation_id: i64) {
    if let Ok(mut map) = token_cache().lock() {
        map.remove(&installation_id);
    }
}

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

pub struct InstallationClient {
    pub token: String,
    pub installation_id: i64,
}

impl InstallationClient {
    pub async fn for_installation(installation_id: i64) -> Result<Self, GithubAppError> {
        let cfg = config().ok_or(GithubAppError::NotConfigured)?;
        let token = mint_installation_token(cfg, installation_id).await?;
        Ok(Self {
            token,
            installation_id,
        })
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

    pub fn git_https_url(&self, owner: &str, repo: &str) -> String {
        tokenized_git_https_url(owner, repo, &self.token)
    }
}

pub fn tokenized_git_https_url(owner: &str, repo: &str, token: &str) -> String {
    format!("https://x-access-token:{token}@github.com/{owner}/{repo}.git")
}

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

pub type SharedGithubAppConfig = Arc<GithubAppConfig>;

#[derive(Debug, Clone)]
pub struct CreateRepoFromTemplateRequest {
    pub template_owner: String,
    pub template_repo: String,
    pub new_owner: String,
    pub new_name: String,
    pub description: Option<String>,
    pub private: bool,
    pub include_all_branches: bool,
}

#[derive(Debug, Clone)]
pub struct CreatedRepository {
    pub owner: String,
    pub repo: String,
    pub default_branch: String,
    pub html_url: String,
}

pub async fn create_repo_from_template(
    client: &InstallationClient,
    req: CreateRepoFromTemplateRequest,
) -> Result<CreatedRepository, GithubAppError> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/generate",
        req.template_owner, req.template_repo
    );
    let mut body = serde_json::json!({
        "owner": req.new_owner,
        "name": req.new_name,
        "private": req.private,
        "include_all_branches": req.include_all_branches,
    });
    if let Some(desc) = req.description.as_deref().filter(|s| !s.is_empty()) {
        body["description"] = serde_json::Value::String(desc.into());
    }

    let response_body = send_json(client.post(&url).json(&body), &format!("POST {url}")).await?;
    Ok(CreatedRepository {
        owner: response_body["owner"]["login"]
            .as_str()
            .unwrap_or(&req.new_owner)
            .to_string(),
        repo: response_body["name"]
            .as_str()
            .unwrap_or(&req.new_name)
            .to_string(),
        default_branch: response_body["default_branch"]
            .as_str()
            .unwrap_or("main")
            .to_string(),
        html_url: response_body["html_url"].as_str().unwrap_or("").to_string(),
    })
}

pub async fn resolve_head_sha(
    client: &InstallationClient,
    owner: &str,
    repo: &str,
    branch: &str,
) -> Result<String, GithubAppError> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}/git/ref/heads/{branch}");
    let v = send_json(client.get(&url), &format!("GET {url}")).await?;
    v["object"]["sha"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| GithubAppError::Parse(format!("GET {url}: missing object.sha")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateBranchStatus {
    Created,
    AlreadyExists,
}

pub async fn create_branch(
    client: &InstallationClient,
    owner: &str,
    repo: &str,
    branch_name: &str,
    base_sha: &str,
) -> Result<CreateBranchStatus, GithubAppError> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}/git/refs");
    let body = serde_json::json!({
        "ref": format!("refs/heads/{branch_name}"),
        "sha": base_sha,
    });

    let res = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| GithubAppError::Transport(e.to_string()))?;
    let status = res.status();
    let status_u16 = status.as_u16();
    if status.is_success() {
        return Ok(CreateBranchStatus::Created);
    }

    let body_text = res.text().await.unwrap_or_default();
    if status_u16 == 422 && body_text.contains("Reference already exists") {
        return Ok(CreateBranchStatus::AlreadyExists);
    }
    Err(GithubAppError::Api {
        status: status_u16,
        body: format!("POST {url}: {body_text}"),
    })
}

#[derive(Debug, Clone)]
pub struct PullRequestRequest {
    pub owner: String,
    pub repo: String,
    pub head_branch: String,
    pub base_branch: String,
    pub title: String,
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: i32,
    pub html_url: String,
    pub already_exists: bool,
}

pub async fn open_pull_request(
    client: &InstallationClient,
    req: PullRequestRequest,
) -> Result<PullRequest, GithubAppError> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls",
        req.owner, req.repo
    );
    let mut body = serde_json::json!({
        "title": req.title,
        "head": req.head_branch,
        "base": req.base_branch,
    });
    if let Some(body_text) = req.body.as_deref().filter(|s| !s.is_empty()) {
        body["body"] = serde_json::Value::String(body_text.into());
    }

    let res = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| GithubAppError::Transport(e.to_string()))?;
    let status = res.status();
    let status_u16 = status.as_u16();
    let response_body: serde_json::Value = res
        .json()
        .await
        .map_err(|e| GithubAppError::Parse(e.to_string()))?;

    if status.is_success() {
        return pr_from_json(&response_body, false);
    }

    if status_u16 == 422
        && response_body
            .get("errors")
            .and_then(|e| e.as_array())
            .is_some_and(|arr| {
                arr.iter().any(|e| {
                    e["message"]
                        .as_str()
                        .unwrap_or("")
                        .contains("already exists")
                })
            })
    {
        let list_url = format!(
            "https://api.github.com/repos/{}/{}/pulls?state=open&head={}:{}&base={}",
            req.owner, req.repo, req.owner, req.head_branch, req.base_branch
        );
        let existing = client
            .get(&list_url)
            .send()
            .await
            .map_err(|e| GithubAppError::Transport(e.to_string()))?
            .json::<Vec<serde_json::Value>>()
            .await
            .map_err(|e| GithubAppError::Parse(e.to_string()))?;
        if let Some(pr) = existing.first() {
            return pr_from_json(pr, true);
        }
    }

    Err(GithubAppError::Api {
        status: status_u16,
        body: format!("POST {url}: {response_body}"),
    })
}

fn pr_from_json(
    value: &serde_json::Value,
    already_exists: bool,
) -> Result<PullRequest, GithubAppError> {
    let number = value["number"]
        .as_i64()
        .ok_or_else(|| GithubAppError::Parse("pull request response missing `number`".into()))?;
    let html_url = value["html_url"].as_str().unwrap_or("").to_string();
    Ok(PullRequest {
        number: number as i32,
        html_url,
        already_exists,
    })
}

async fn send_json(
    builder: reqwest::RequestBuilder,
    label: &str,
) -> Result<serde_json::Value, GithubAppError> {
    let res = builder
        .send()
        .await
        .map_err(|e| GithubAppError::Transport(e.to_string()))?;
    let status = res.status();
    let body: serde_json::Value = res
        .json()
        .await
        .map_err(|e| GithubAppError::Parse(e.to_string()))?;
    if !status.is_success() {
        return Err(GithubAppError::Api {
            status: status.as_u16(),
            body: format!("{label}: {body}"),
        });
    }
    Ok(body)
}

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
    fn tokenized_url_is_stable() {
        assert_eq!(
            tokenized_git_https_url("acme", "demo", "tok"),
            "https://x-access-token:tok@github.com/acme/demo.git"
        );
    }
}
