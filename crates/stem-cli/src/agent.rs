//! Thin orchestration layer over `opencode-client`.
//!
//! Responsibilities:
//! - Spin up a per-repo OpenCode server via `ProcessManager`.
//! - Open a session, send a prompt, and drain the SSE stream until idle.
//! - Surface tool calls / text deltas as `tracing` events so the CLI UI
//!   stays uniform across subcommands.
//!
//! This module is intentionally transport-agnostic: it returns a
//! `SessionOutcome` and leaves presentation to the caller.

use anyhow::{Context, Result};
use futures::StreamExt;
use opencode_client::{
    FileDiff, OpenCodeClient, OpenCodeEvent, Part, ProcessManager, ProcessManagerConfig, sse,
};
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

/// Default system prompt that pins OpenCode to the constraints documented
/// in `AGENTS.md`. Callers append task-specific guidance via the `goal`.
const DEFAULT_SYSTEM_PROMPT: &str = "You are editing the stem-cell repository via the `stem` CLI.\n\n\
Hard constraints (see AGENTS.md for full context):\n\
- Do NOT start dev servers, `mise run dev`, or any long-running processes.\n\
- Do NOT assume a localhost preview URL is reachable — the host owns that lifecycle.\n\
- Prefer minimal, reversible diffs. Annotate non-obvious decisions with comments explaining WHY.\n\
- Respect the editable surface: specs/self.yaml, specs/systems.yaml, crates/runtime/src/systems/*.rs,\n  frontend/src/pages/index.astro. Anything else is generated or framework code.\n\
- If the change requires editing generated/framework code, STOP and explain instead of silently patching.\n\
- After spec edits, remember that `cargo run -p systems-codegen` must be run to refresh stubs.\n\
";

/// Outcome of a single agent turn.
pub struct SessionOutcome {
    pub session_id: String,
    pub diffs: Vec<FileDiff>,
    pub reached_idle: bool,
}

pub struct Agent {
    manager: ProcessManager,
    client: OpenCodeClient,
    model: Option<String>,
}

impl Agent {
    /// Boots an OpenCode server against `work_dir`, keyed by `project_id`.
    pub async fn boot(
        project_id: Uuid,
        work_dir: &Path,
        model: Option<String>,
    ) -> Result<Self> {
        let mut config = ProcessManagerConfig::from_env();
        // CLI runs are short-lived; reap aggressively so we don't leave
        // OpenCode servers hanging if the parent is SIGKILL'd.
        config.idle_timeout = Duration::from_secs(120);
        if config.default_model.is_none() {
            config.default_model = model.clone();
        }

        let manager = ProcessManager::new(config);
        let client = manager
            .get_or_spawn(project_id, work_dir)
            .await
            .context("spawning OpenCode server")?;

        Ok(Self {
            manager,
            client,
            model,
        })
    }

    /// Sends `goal` as a new session, drains events, returns resulting diffs.
    ///
    /// `timeout` bounds the entire agent loop, not individual events — a hung
    /// tool call will still be force-aborted when the deadline elapses.
    pub async fn run_turn(
        &self,
        goal: &str,
        system_suffix: Option<&str>,
        timeout: Duration,
    ) -> Result<SessionOutcome> {
        let session = self
            .client
            .create_session(Some("stem-cli"))
            .await
            .context("creating OpenCode session")?;

        let system = match system_suffix {
            Some(s) => format!("{DEFAULT_SYSTEM_PROMPT}\n{s}"),
            None => DEFAULT_SYSTEM_PROMPT.to_string(),
        };

        let parts = vec![Part::Text {
            text: goal.to_string(),
        }];

        // Subscribe BEFORE prompting so we can't miss the first events.
        let base_url = self.client.base_url().to_string();
        // The client doesn't expose its auth header, but SSE on localhost
        // honours the same password via env var when present.
        let auth_header = std::env::var("OPENCODE_SERVER_PASSWORD")
            .ok()
            .map(|pw| format!("Basic {}", basic_encode(&pw)));
        let mut events = sse::subscribe(base_url, auth_header)
            .context("opening OpenCode SSE stream")?;

        self.client
            .prompt_async(
                &session.id,
                parts,
                self.model.as_deref(),
                Some(&system),
            )
            .await
            .context("sending prompt to OpenCode")?;

        tracing::info!(session = %session.id, "prompt dispatched");

        let deadline = tokio::time::Instant::now() + timeout;
        let mut reached_idle = false;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                tracing::warn!(session = %session.id, "timeout exceeded; aborting session");
                let _ = self.client.session_abort(&session.id).await;
                break;
            }

            let next = tokio::time::timeout(remaining, events.next()).await;
            let event = match next {
                Err(_) => {
                    tracing::warn!(session = %session.id, "SSE stream stalled past deadline");
                    let _ = self.client.session_abort(&session.id).await;
                    break;
                }
                Ok(None) => {
                    tracing::debug!("SSE stream closed");
                    break;
                }
                Ok(Some(Err(e))) => {
                    tracing::warn!(error = %e, "SSE error; treating as stream end");
                    break;
                }
                Ok(Some(Ok(ev))) => ev,
            };

            render_event(&event);
            if event.is_terminal() {
                reached_idle = matches!(event, OpenCodeEvent::SessionIdle { .. });
                break;
            }
        }

        let (diffs, _raw) = self
            .client
            .session_diff_with_raw(&session.id)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "fetching diff failed; assuming no changes");
                (Vec::new(), String::new())
            });

        Ok(SessionOutcome {
            session_id: session.id,
            diffs,
            reached_idle,
        })
    }

    pub async fn shutdown(self) {
        self.manager.shutdown_all().await;
    }
}

fn render_event(event: &OpenCodeEvent) {
    match event {
        OpenCodeEvent::ServerConnected => tracing::debug!("opencode: server connected"),
        OpenCodeEvent::ServerHeartbeat => {}
        OpenCodeEvent::MessagePartDelta { properties }
        | OpenCodeEvent::MessagePartUpdated { properties } => {
            if let Some(text) = extract_text_delta(properties) {
                // Print text deltas verbatim so the user sees the agent
                // thinking in real time. We stay on stdout (not tracing)
                // to avoid the log prefix breaking up the flow.
                print!("{text}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            } else if let Some(tool) = extract_tool_name(properties) {
                tracing::info!(tool = %tool, "opencode: tool call");
            }
        }
        OpenCodeEvent::MessageCompleted { .. } => {
            println!();
            tracing::info!("opencode: message completed");
        }
        OpenCodeEvent::SessionIdle { .. } => {
            println!();
            tracing::info!("opencode: session idle");
        }
        OpenCodeEvent::SessionUpdated { .. } => {}
        OpenCodeEvent::Unknown { raw_type, .. } => {
            tracing::trace!(ty = %raw_type, "opencode: unknown event");
        }
    }
}

/// Best-effort extraction of a text delta from an OpenCode event payload.
/// OpenCode nests the delta a few ways depending on version; try each.
fn extract_text_delta(properties: &serde_json::Value) -> Option<&str> {
    // Shape A: { part: { text: "..." } }
    if let Some(t) = properties.pointer("/part/text").and_then(|v| v.as_str()) {
        return Some(t);
    }
    // Shape B: { delta: { text: "..." } }
    if let Some(t) = properties.pointer("/delta/text").and_then(|v| v.as_str()) {
        return Some(t);
    }
    // Shape C: { text: "..." }
    properties.get("text").and_then(|v| v.as_str())
}

fn extract_tool_name(properties: &serde_json::Value) -> Option<String> {
    let part = properties.get("part")?;
    let ty = part.get("type")?.as_str()?;
    if ty == "tool" || ty == "tool-call" || ty == "tool_use" {
        let name = part
            .get("name")
            .and_then(|v| v.as_str())
            .or_else(|| part.get("tool").and_then(|v| v.as_str()))
            .unwrap_or("unknown");
        Some(name.to_string())
    } else {
        None
    }
}

/// Minimal Basic-auth helper (OpenCode expects `opencode:<password>`).
fn basic_encode(password: &str) -> String {
    use std::io::Write;
    let mut buf = Vec::new();
    write!(buf, "opencode:{password}").unwrap();
    encode_b64(&buf)
}

fn encode_b64(input: &[u8]) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_text_delta_from_part_shape() {
        let v = json!({ "part": { "text": "hello" } });
        assert_eq!(extract_text_delta(&v), Some("hello"));
    }

    #[test]
    fn extracts_text_delta_from_delta_shape() {
        let v = json!({ "delta": { "text": "world" } });
        assert_eq!(extract_text_delta(&v), Some("world"));
    }

    #[test]
    fn extracts_tool_name() {
        let v = json!({ "part": { "type": "tool", "name": "edit" } });
        assert_eq!(extract_tool_name(&v).as_deref(), Some("edit"));
    }

    #[test]
    fn ignores_non_tool_parts() {
        let v = json!({ "part": { "type": "text", "text": "hi" } });
        assert!(extract_tool_name(&v).is_none());
    }

    #[test]
    fn basic_encode_matches_known_value() {
        assert_eq!(basic_encode("secret"), "b3BlbmNvZGU6c2VjcmV0");
    }
}
