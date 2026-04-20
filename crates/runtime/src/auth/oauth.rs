use axum::extract::{Path, Query, State};
use axum::response::Redirect;
use axum_extra::extract::CookieJar;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, RedirectUrl, Scope,
    TokenResponse, TokenUrl,
};
use serde::Deserialize;

use super::AppState;
use super::config::OAuthProviderConfig;
use super::repository;
use super::routes::{AuthError, session_cookie};

struct ProviderUrls {
    auth_url: &'static str,
    token_url: &'static str,
    user_info_url: &'static str,
    scopes: &'static [&'static str],
}

fn provider_urls(provider: &str) -> Option<ProviderUrls> {
    match provider {
        "github" => Some(ProviderUrls {
            auth_url: "https://github.com/login/oauth/authorize",
            token_url: "https://github.com/login/oauth/access_token",
            user_info_url: "https://api.github.com/user",
            scopes: &["user:email"],
        }),
        "google" => Some(ProviderUrls {
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
            token_url: "https://oauth2.googleapis.com/token",
            user_info_url: "https://www.googleapis.com/oauth2/v2/userinfo",
            scopes: &["email", "profile"],
        }),
        _ => None,
    }
}

fn get_provider_config<'a>(state: &'a AppState, provider: &str) -> Option<&'a OAuthProviderConfig> {
    match provider {
        "github" => state.auth_config.github.as_ref(),
        "google" => state.auth_config.google.as_ref(),
        _ => None,
    }
}

// ── GET /auth/oauth/:provider ───────────────────────────────────────────

pub async fn oauth_redirect(
    State(state): State<AppState>,
    Path(provider): Path<String>,
) -> Result<Redirect, AuthError> {
    let urls = provider_urls(&provider)
        .ok_or_else(|| AuthError::Internal(format!("unsupported provider: {provider}")))?;

    let provider_config = get_provider_config(&state, &provider)
        .ok_or_else(|| AuthError::Internal(format!("{provider} OAuth not configured")))?;

    let redirect_url = format!(
        "{}/auth/oauth/{}/callback",
        state.auth_config.app_url, provider
    );

    let client = oauth2::basic::BasicClient::new(ClientId::new(provider_config.client_id.clone()))
        .set_client_secret(ClientSecret::new(provider_config.client_secret.clone()))
        .set_auth_uri(
            AuthUrl::new(urls.auth_url.to_string())
                .map_err(|e| AuthError::Internal(e.to_string()))?,
        )
        .set_token_uri(
            TokenUrl::new(urls.token_url.to_string())
                .map_err(|e| AuthError::Internal(e.to_string()))?,
        )
        .set_redirect_uri(
            RedirectUrl::new(redirect_url).map_err(|e| AuthError::Internal(e.to_string()))?,
        );

    let mut auth_request = client.authorize_url(CsrfToken::new_random);
    for scope in urls.scopes {
        auth_request = auth_request.add_scope(Scope::new(scope.to_string()));
    }
    let (auth_url, _csrf_token) = auth_request.url();

    Ok(Redirect::temporary(auth_url.as_str()))
}

// ── GET /auth/oauth/:provider/callback ──────────────────────────────────

#[derive(Deserialize)]
pub struct OAuthCallback {
    pub code: String,
    #[allow(dead_code)]
    pub state: Option<String>,
}

pub async fn oauth_callback(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Query(params): Query<OAuthCallback>,
) -> Result<(CookieJar, Redirect), AuthError> {
    let urls = provider_urls(&provider)
        .ok_or_else(|| AuthError::Internal(format!("unsupported provider: {provider}")))?;

    let provider_config = get_provider_config(&state, &provider)
        .ok_or_else(|| AuthError::Internal(format!("{provider} OAuth not configured")))?;

    let redirect_url = format!(
        "{}/auth/oauth/{}/callback",
        state.auth_config.app_url, provider
    );

    let client = oauth2::basic::BasicClient::new(ClientId::new(provider_config.client_id.clone()))
        .set_client_secret(ClientSecret::new(provider_config.client_secret.clone()))
        .set_auth_uri(
            AuthUrl::new(urls.auth_url.to_string())
                .map_err(|e| AuthError::Internal(e.to_string()))?,
        )
        .set_token_uri(
            TokenUrl::new(urls.token_url.to_string())
                .map_err(|e| AuthError::Internal(e.to_string()))?,
        )
        .set_redirect_uri(
            RedirectUrl::new(redirect_url).map_err(|e| AuthError::Internal(e.to_string()))?,
        );

    let http_client = oauth2::reqwest::ClientBuilder::new()
        .build()
        .map_err(|e| AuthError::Internal(format!("failed to build HTTP client: {e}")))?;

    let token_response = client
        .exchange_code(AuthorizationCode::new(params.code))
        .request_async(&http_client)
        .await
        .map_err(|e| AuthError::Internal(format!("token exchange failed: {e}")))?;

    let access_token = token_response.access_token().secret().to_string();

    let user_info = fetch_user_info(&provider, urls.user_info_url, &access_token)
        .await
        .map_err(|e| AuthError::Internal(format!("failed to fetch user info: {e}")))?;

    let account = if let Some(link) =
        repository::find_oauth_link(&state.pool, &provider, &user_info.provider_user_id)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?
    {
        repository::find_account_by_id(&state.pool, link.account_id)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?
            .ok_or_else(|| AuthError::Internal("linked account not found".to_string()))?
    } else if let Some(existing) = repository::find_account_by_email(&state.pool, &user_info.email)
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?
    {
        existing
    } else {
        let mut account = repository::create_account(&state.pool, &user_info.email, None)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        repository::mark_email_verified(&state.pool, account.id)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        account.email_verified = true;
        account
    };

    repository::upsert_oauth_link(
        &state.pool,
        account.id,
        &provider,
        &user_info.provider_user_id,
        user_info.username.as_deref(),
        Some(&access_token),
        token_response
            .refresh_token()
            .map(|t: &oauth2::RefreshToken| t.secret().as_str()),
    )
    .await
    .map_err(|e| AuthError::Internal(e.to_string()))?;

    // Bootstrap: promote the account to `admin` if this OAuth identity matches
    // the ADMIN_GITHUB_USERNAMES allow-list. Runs on every login so the role
    // stays in sync if the list changes and a previously-linked user comes back.
    if let Some(login) = user_info
        .username
        .as_deref()
        .filter(|_| provider == "github")
    {
        let desired_role = if state.auth_config.is_admin_github_user(login) {
            "admin"
        } else {
            "user"
        };
        if desired_role == "admin" && !account.is_admin() {
            tracing::info!(
                account_id = %account.id,
                github_login = %login,
                "promoting account to admin via ADMIN_GITHUB_USERNAMES"
            );
        }
        repository::set_account_role(&state.pool, account.id, desired_role)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;
    }

    let session =
        repository::create_session(&state.pool, account.id, state.auth_config.session_ttl_hours)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

    let jar = CookieJar::new().add(session_cookie(
        &session.token,
        state.auth_config.session_ttl_hours,
    ));

    Ok((jar, Redirect::to("/")))
}

// ── User info fetching ──────────────────────────────────────────────────

struct OAuthUserInfo {
    email: String,
    provider_user_id: String,
    /// Provider's public handle (e.g. GitHub `login`). None for providers that
    /// don't expose one (e.g. Google returns email only).
    username: Option<String>,
}

async fn fetch_user_info(
    provider: &str,
    url: &str,
    access_token: &str,
) -> Result<OAuthUserInfo, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .bearer_auth(access_token)
        .header("User-Agent", "stem-cell")
        .header("Accept", "application/json")
        .send()
        .await?
        .error_for_status()?;

    let json: serde_json::Value = resp.json().await?;

    match provider {
        "github" => {
            let id = json["id"]
                .as_i64()
                .ok_or("missing github user id")?
                .to_string();

            let email = if let Some(e) = json["email"].as_str().filter(|e| !e.is_empty()) {
                e.to_string()
            } else {
                fetch_github_primary_email(access_token).await?
            };

            let username = json["login"].as_str().map(|s| s.to_string());

            Ok(OAuthUserInfo {
                email,
                provider_user_id: id,
                username,
            })
        }
        "google" => {
            let email = json["email"]
                .as_str()
                .ok_or("missing google email")?
                .to_string();
            let id = json["id"]
                .as_str()
                .ok_or("missing google user id")?
                .to_string();

            Ok(OAuthUserInfo {
                email,
                provider_user_id: id,
                username: None,
            })
        }
        _ => Err(format!("unsupported provider: {provider}").into()),
    }
}

async fn fetch_github_primary_email(
    access_token: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://api.github.com/user/emails")
        .bearer_auth(access_token)
        .header("User-Agent", "stem-cell")
        .header("Accept", "application/json")
        .send()
        .await?
        .error_for_status()?;

    let emails: Vec<serde_json::Value> = resp.json().await?;

    for entry in &emails {
        if entry["primary"].as_bool() == Some(true) && entry["verified"].as_bool() == Some(true) {
            if let Some(email) = entry["email"].as_str() {
                return Ok(email.to_string());
            }
        }
    }

    for entry in &emails {
        if entry["verified"].as_bool() == Some(true) {
            if let Some(email) = entry["email"].as_str() {
                return Ok(email.to_string());
            }
        }
    }

    Err("no verified email found on GitHub account".into())
}
