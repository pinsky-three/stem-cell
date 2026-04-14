use crate::error::{Error, Result};
use crate::types::*;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use std::time::Duration;

/// Typed HTTP client for a single OpenCode server instance.
#[derive(Debug, Clone)]
pub struct OpenCodeClient {
    http: reqwest::Client,
    base_url: String,
    auth_header: Option<String>,
}

impl OpenCodeClient {
    pub fn new(port: u16, password: Option<&str>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .connect_timeout(Duration::from_secs(5))
            .build()?;

        let auth_header = password.map(|pw| {
            use std::io::Write;
            let mut buf = Vec::new();
            write!(buf, "opencode:{pw}").unwrap();
            let encoded = base64_encode(&buf);
            format!("Basic {encoded}")
        });

        Ok(Self {
            http,
            base_url: format!("http://127.0.0.1:{port}"),
            auth_header,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth_header {
            Some(h) => req.header(AUTHORIZATION, h),
            None => req,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    // ── Health ────────────────────────────────────────────────

    pub async fn health(&self) -> Result<()> {
        let req = self
            .http
            .get(self.url("/global/health"))
            .header(ACCEPT, "application/json");
        let resp = self.apply_auth(req).send().await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(Error::ApiError {
                status: resp.status(),
                body: resp.text().await.unwrap_or_default(),
            })
        }
    }

    /// Polls health endpoint until OK or timeout.
    pub async fn wait_healthy(&self, timeout: Duration) -> Result<()> {
        let start = std::time::Instant::now();
        let poll_interval = Duration::from_millis(500);

        loop {
            match self.health().await {
                Ok(()) => return Ok(()),
                Err(_) if start.elapsed() < timeout => {
                    tokio::time::sleep(poll_interval).await;
                }
                Err(_) => {
                    return Err(Error::HealthTimeout {
                        elapsed_ms: start.elapsed().as_millis() as u64,
                    })
                }
            }
        }
    }

    // ── Sessions ──────────────────────────────────────────────

    pub async fn create_session(&self, title: Option<&str>) -> Result<Session> {
        let body = CreateSessionRequest {
            parent_id: None,
            title: title.map(|s| s.to_string()),
        };
        let req = self
            .http
            .post(self.url("/session"))
            .header(CONTENT_TYPE, "application/json")
            .json(&body);
        let resp = self.apply_auth(req).send().await?;
        parse_response(resp).await
    }

    pub async fn get_session(&self, session_id: &str) -> Result<Session> {
        let req = self
            .http
            .get(self.url(&format!("/session/{session_id}")))
            .header(ACCEPT, "application/json");
        let resp = self.apply_auth(req).send().await?;
        parse_response(resp).await
    }

    pub async fn list_sessions(&self) -> Result<Vec<Session>> {
        let req = self
            .http
            .get(self.url("/session"))
            .header(ACCEPT, "application/json");
        let resp = self.apply_auth(req).send().await?;
        parse_response(resp).await
    }

    // ── Messages ──────────────────────────────────────────────

    /// Sends a message and waits for the full response (blocking call).
    pub async fn send_message(
        &self,
        session_id: &str,
        parts: Vec<Part>,
        model: Option<&str>,
    ) -> Result<MessageResponse> {
        let body = SendMessageRequest {
            parts,
            model: model.map(|s| s.to_string()),
            agent: None,
            message_id: None,
            system: None,
            no_reply: None,
        };
        let req = self
            .http
            .post(self.url(&format!("/session/{session_id}/message")))
            .header(CONTENT_TYPE, "application/json")
            .json(&body);
        let resp = self.apply_auth(req).send().await?;
        parse_response(resp).await
    }

    /// Fire-and-forget: sends a prompt and returns immediately (204).
    /// Use the SSE event stream to track progress.
    pub async fn prompt_async(
        &self,
        session_id: &str,
        parts: Vec<Part>,
        model: Option<&str>,
    ) -> Result<()> {
        let body = SendMessageRequest {
            parts,
            model: model.map(|s| s.to_string()),
            agent: None,
            message_id: None,
            system: None,
            no_reply: None,
        };
        let req = self
            .http
            .post(self.url(&format!("/session/{session_id}/prompt_async")))
            .header(CONTENT_TYPE, "application/json")
            .json(&body);
        let resp = self.apply_auth(req).send().await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(Error::ApiError {
                status: resp.status(),
                body: resp.text().await.unwrap_or_default(),
            })
        }
    }

    // ── Diffs ─────────────────────────────────────────────────

    pub async fn session_diff(&self, session_id: &str) -> Result<Vec<FileDiff>> {
        let req = self
            .http
            .get(self.url(&format!("/session/{session_id}/diff")))
            .header(ACCEPT, "application/json");
        let resp = self.apply_auth(req).send().await?;
        parse_response(resp).await
    }
}

async fn parse_response<T: serde::de::DeserializeOwned>(resp: reqwest::Response) -> Result<T> {
    let status = resp.status();
    if status.is_success() {
        let body = resp.text().await?;
        serde_json::from_str(&body).map_err(Error::Json)
    } else {
        Err(Error::ApiError {
            status,
            body: resp.text().await.unwrap_or_default(),
        })
    }
}

fn base64_encode(input: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        out.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}
