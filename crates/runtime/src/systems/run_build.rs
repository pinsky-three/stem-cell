use crate::system_api::*;
use opencode_client::types::{BuildEvent, Part};
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use tokio::sync::broadcast;
use tracing::Instrument;

/// Global event bus: project_id → broadcast sender for build events.
/// The SSE endpoint subscribes to this; RunBuild publishes to it.
static EVENT_BUS: OnceLock<Arc<tokio::sync::RwLock<EventBus>>> = OnceLock::new();

pub type EventBus = std::collections::HashMap<uuid::Uuid, broadcast::Sender<BuildEvent>>;

pub fn event_bus() -> Arc<tokio::sync::RwLock<EventBus>> {
    EVENT_BUS
        .get_or_init(|| Arc::new(tokio::sync::RwLock::new(EventBus::new())))
        .clone()
}

/// Global ProcessManager — initialized once in main.rs, used here.
static PROCESS_MANAGER: OnceLock<opencode_client::ProcessManager> = OnceLock::new();

pub fn init_process_manager(pm: opencode_client::ProcessManager) {
    let _ = PROCESS_MANAGER.set(pm);
}

pub fn process_manager() -> &'static opencode_client::ProcessManager {
    PROCESS_MANAGER
        .get()
        .expect("ProcessManager not initialized — call init_process_manager in main")
}

/// Default system text for OpenCode (full stack): file edits only; host owns dev/preview lifecycle.
const OPENCODE_BUILD_SYSTEM_FULL: &str = concat!(
    "You are editing a repository checkout managed by an external host.\n",
    "\n",
    "Rules:\n",
    "- Make file edits only. Do not start long-lived processes: no `mise run dev`, ",
    "`npm run dev`, `vite`, `astro dev`, or similar preview servers; the host starts ",
    "and owns the dev server lifecycle.\n",
    "- Do not tell the user to open localhost URLs for preview; the host manages that.\n",
    "- Be concise. Do not repeat the same explanation or plan twice in one reply.\n",
    "- Avoid broad process-killing commands (e.g. killing all node processes) that could ",
    "stop the host-managed dev server.\n",
    "- You may fix project files so that when the host runs `mise run dev`, the app builds ",
    "and serves correctly.",
);

/// Free-tier / frontend-only projects: constrain edits to the Astro tree.
const OPENCODE_BUILD_SYSTEM_FRONTEND: &str = concat!(
    "You are editing a repository checkout managed by an external host.\n",
    "\n",
    "Rules:\n",
    "- You may **only** change files under `frontend/src/`. Do not modify Rust crates, ",
    "`specs/`, SQL, backend routes, `Cargo.toml`, or anything outside `frontend/src/`.\n",
    "- Make file edits only. Do not start long-lived processes: no `mise run dev`, ",
    "`npm run dev`, `vite`, `astro dev`, or similar preview servers; the host starts ",
    "and owns the dev server lifecycle.\n",
    "- Do not tell the user to open localhost URLs for preview; the host manages that.\n",
    "- Be concise. Do not repeat the same explanation or plan twice in one reply.\n",
    "- Avoid broad process-killing commands (e.g. killing all node processes) that could ",
    "stop the host-managed dev server.\n",
    "- You may fix frontend files so that when the host runs `mise run frontend:dev`, ",
    "the Astro app builds and serves correctly.",
);

fn effective_project_scope(raw: &str) -> &'static str {
    match raw {
        "full" => "full",
        _ => "frontend",
    }
}

/// If `STEM_CELL_OPENCODE_SYSTEM_PROMPT` is set and non-empty after trim, it replaces the
/// default; if set to whitespace-only, no system message is sent.
fn opencode_build_system_prompt(project_scope_raw: &str) -> Option<String> {
    match std::env::var("STEM_CELL_OPENCODE_SYSTEM_PROMPT") {
        Ok(s) if !s.trim().is_empty() => Some(s),
        Ok(_) => None,
        Err(_) => {
            let body = match effective_project_scope(project_scope_raw) {
                "full" => OPENCODE_BUILD_SYSTEM_FULL,
                _ => OPENCODE_BUILD_SYSTEM_FRONTEND,
            };
            Some(body.to_string())
        }
    }
}

/// Result returned by the SSE forwarder task to the main run_build flow.
/// Keeps a rich signal so the outer code can decide success vs. failure.
struct ForwarderOutcome {
    /// Accumulated assistant text/log. `None` means the forwarder never
    /// observed session completion (timeout or early drop).
    log_buf: Option<String>,
    /// Populated when OpenCode emitted a `session.error` event. Carries a
    /// short human-readable message describing the upstream model/provider
    /// failure (e.g. "provider_unavailable: Network connection lost"). When
    /// set, the build MUST NOT be marked succeeded — OpenCode idles right
    /// after these errors, which would otherwise look like a clean exit.
    provider_error: Option<String>,
}

/// Best-effort extractor for the human-readable bit of an OpenCode
/// `session.error` payload. The real error text is double-JSON-encoded
/// (the upstream provider's JSON is embedded as a string inside OpenCode's
/// own envelope), so we try several nested shapes before giving up.
fn summarize_session_error(data: &serde_json::Value) -> String {
    let err_obj = data.get("error").unwrap_or(data);
    let name = err_obj
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("UnknownError");
    let raw_message = err_obj
        .get("data")
        .and_then(|d| d.get("message"))
        .and_then(|m| m.as_str());
    if let Some(msg) = raw_message {
        // The inner `message` field is frequently another JSON document.
        if let Ok(inner) = serde_json::from_str::<serde_json::Value>(msg) {
            let code = inner.get("code").and_then(|v| v.as_i64());
            let inner_msg = inner.get("message").and_then(|v| v.as_str()).unwrap_or(msg);
            let error_type = inner
                .get("metadata")
                .and_then(|m| m.get("error_type"))
                .and_then(|v| v.as_str());
            return match (code, error_type) {
                (Some(c), Some(t)) => format!("{name}: {t} ({c}) — {inner_msg}"),
                (Some(c), None) => format!("{name}: {c} — {inner_msg}"),
                (None, Some(t)) => format!("{name}: {t} — {inner_msg}"),
                (None, None) => format!("{name}: {inner_msg}"),
            };
        }
        return format!("{name}: {msg}");
    }
    format!("{name}: {data}")
}

#[async_trait::async_trait]
impl RunBuildSystem for super::AppSystems {
    async fn execute(
        &self,
        pool: &sqlx::PgPool,
        input: RunBuildInput,
    ) -> Result<RunBuildOutput, RunBuildError> {
        // IMPORTANT: Use `.instrument(span).await` instead of
        // `let _enter = span.enter();`. Holding an Enter guard across `.await`
        // points is unsound: the task may be polled on a different thread,
        // and the guard can be dropped out of order with respect to the span's
        // real lifetime. In practice that caused tracing-subscriber to panic
        // with "tried to clone a span that already closed" as soon as a
        // follow-up build re-entered this function while a prior span's
        // handles were still being dereferenced downstream.
        let span = tracing::info_span!("run_build", build_job_id = %input.build_job_id);
        async move {

        // ── Load build job ────────────────────────────────────
        let build_row = sqlx::query(
            "SELECT id, status, prompt_summary, model, project_id, message_id, opencode_session_id \
             FROM build_jobs WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(input.build_job_id)
        .fetch_optional(pool)
        .await
        .map_err(|e: sqlx::Error| RunBuildError::BuildFailed(e.to_string()))?
        .ok_or(RunBuildError::BuildJobNotFound)?;

        let project_id: uuid::Uuid = build_row.get("project_id");
        let build_id: uuid::Uuid = build_row.get("id");
        let prompt: String = build_row.get("prompt_summary");
        let model: String = build_row.get("model");
        let existing_session: Option<String> = build_row.get("opencode_session_id");

        // ── Load project ──────────────────────────────────────
        let project_row = sqlx::query(
            "SELECT id, slug, org_id, scope FROM projects WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(project_id)
        .fetch_optional(pool)
        .await
        .map_err(|e: sqlx::Error| RunBuildError::BuildFailed(e.to_string()))?
        .ok_or(RunBuildError::ProjectNotFound)?;

        let org_id: uuid::Uuid = project_row.get("org_id");
        let project_scope: String = project_row.get("scope");
        let started = std::time::Instant::now();

        // ── Mark as running ───────────────────────────────────
        sqlx::query("UPDATE build_jobs SET status = 'running', updated_at = NOW() WHERE id = $1")
            .bind(build_id)
            .execute(pool)
            .await
            .map_err(|e: sqlx::Error| RunBuildError::BuildFailed(e.to_string()))?;

        publish_event(
            project_id,
            BuildEvent::BuildStatus {
                job_id: build_id.to_string(),
                status: "running".to_string(),
            },
        )
        .await;

        // ── Resolve project work dir ──────────────────────────
        // SpawnEnvironment clones into /tmp/stem-cell-{spawn_job_id}.
        // Find the deployment's original build_job_id to locate the checkout.
        let work_dir = resolve_work_dir_for_project(pool, project_id)
            .await
            .map_err(|e| RunBuildError::BuildFailed(e))?;
        if !work_dir.exists() {
            tokio::fs::create_dir_all(&work_dir).await.map_err(|e| {
                RunBuildError::BuildFailed(format!("mkdir {}: {e}", work_dir.display()))
            })?;
        }

        // ── Get or spawn OpenCode server ──────────────────────
        let pm = process_manager();
        let client = pm
            .get_or_spawn(project_id, &work_dir)
            .await
            .map_err(|e| RunBuildError::AiProviderError(e.to_string()))?;

        // ── Create or reuse session ───────────────────────────
        let session = match existing_session {
            Some(ref sid) => {
                // If the previous turn short-circuited on a `question`-type
                // tool, that tool is still `pending` on OpenCode's side and
                // the agent loop is blocked. New prompts to the session sit
                // silently behind it (zero tool calls, zero streamed text,
                // just heartbeats forever). Abort first; it's a no-op for
                // idle sessions and clears the blocked tool when needed.
                if let Err(e) = client.session_abort(sid).await {
                    tracing::warn!(
                        session_id = %sid,
                        error = %e,
                        "session_abort failed before follow-up prompt — continuing anyway"
                    );
                } else {
                    tracing::info!(
                        session_id = %sid,
                        "session aborted before follow-up prompt (clears any pending tool)"
                    );
                }
                client
                    .get_session(sid)
                    .await
                    .map_err(|e| RunBuildError::AiProviderError(e.to_string()))?
            }
            None => {
                let s = client
                    .create_session(Some(&format!("build-{build_id}")))
                    .await
                    .map_err(|e| RunBuildError::AiProviderError(e.to_string()))?;

                sqlx::query(
                    "UPDATE build_jobs SET opencode_session_id = $2, updated_at = NOW() WHERE id = $1",
                )
                .bind(build_id)
                .bind(&s.id)
                .execute(pool)
                .await
                .map_err(|e: sqlx::Error| RunBuildError::BuildFailed(e.to_string()))?;

                s
            }
        };

        tracing::info!(session_id = %session.id, "OpenCode session ready");

        // ── Subscribe to SSE events (background forwarder) ────
        let job_id_str = build_id.to_string();
        let proj_id = project_id;
        let auth_header = pm.config().server_password.as_ref().map(|pw| {
            format!(
                "Basic {}",
                simple_base64(format!("opencode:{pw}").as_bytes())
            )
        });

        tracing::info!(base_url = %client.base_url(), "connecting SSE event stream");

        let event_stream =
            opencode_client::sse::subscribe(client.base_url().to_owned(), auth_header.clone())
                .map_err(|e| RunBuildError::AiProviderError(e.to_string()))?;

        let pool_bg = pool.clone();
        let build_id_bg = build_id;
        let job_id_bg = job_id_str.clone();
        let session_id_bg = session.id.clone();

        // Signal from forwarder → main task when SSE is connected
        let (connected_tx, connected_rx) = tokio::sync::oneshot::channel::<()>();

        // Verbose mode dumps every non-heartbeat event at INFO level. Enable via
        //   STEM_CELL_RUN_BUILD_VERBOSE=1
        // when diagnosing silent stalls in production. Keep off by default to
        // avoid log spam at ~100 chunks/sec during heavy streaming.
        let verbose = std::env::var("STEM_CELL_RUN_BUILD_VERBOSE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        // The forwarder returns a rich ForwarderOutcome so the main task can
        // reason about WHY we exited the loop (idle vs timeout vs stream-close
        // vs fatal error) and how much work OpenCode reported.
        let forwarder = tokio::spawn(async move {
            use futures::StreamExt;
            let mut stream = std::pin::pin!(event_stream);
            let mut log_buf = String::new();
            let mut connected_tx = Some(connected_tx);
            let mut completed = false;
            let mut tool_calls_announced: HashSet<String> = HashSet::new();
            // callID → (tool_name, last_status). We log every status transition
            // so tool errors and stuck tools show up in the logs, not just
            // initial invocations.
            let mut tool_state: HashMap<String, (String, String)> = HashMap::new();
            let mut tool_errors: u32 = 0;
            let mut tool_completed: u32 = 0;
            let mut stream_errors: u32 = 0;
            let mut heartbeats: u32 = 0;
            let mut message_completed_seen = false;
            let mut session_idle_matched = false;
            // OpenCode can call its `question`/`ask` tool to request user
            // input, which stays `pending` until the user replies. Without
            // short-circuiting, the forwarder waits up to `sse_secs` while
            // the frontend input is locked (isLoading = true). We detect the
            // first such tool and exit the loop so the UI unlocks and the
            // next user message flows back to OpenCode as the tool's answer
            // via the same session.
            let mut awaiting_user_reply = false;
            let mut awaiting_question: Option<String> = None;
            // Populated when OpenCode emits a `session.error` event (upstream
            // model/provider failure). We keep the loop alive so the follow-up
            // `session.idle` still flushes, but the outer code will see this
            // value and fail the build rather than mark it succeeded with 0
            // artifacts. See `summarize_session_error` for the extraction.
            let mut provider_error: Option<String> = None;
            let forwarder_started = tokio::time::Instant::now();
            let mut last_progress = forwarder_started;
            let mut last_heartbeat_log = forwarder_started;
            let sse_secs: u64 = std::env::var("STEM_CELL_RUN_BUILD_SSE_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .filter(|&n| (60..=86_400).contains(&n))
                .unwrap_or(1800);
            let deadline =
                tokio::time::Instant::now() + tokio::time::Duration::from_secs(sse_secs);
            // Reason the loop exits — surfaced in the summary log so we can
            // see at a glance why a build ended.
            let mut exit_reason = "unknown";

            tracing::info!(
                session_id = %session_id_bg,
                sse_timeout_secs = sse_secs,
                verbose,
                "SSE forwarder started"
            );

            loop {
                // Short-circuit when OpenCode's `question`/`ask` tool is
                // awaiting a user reply. Its state never advances on its own,
                // so waiting for session.idle would block until the 30 min
                // deadline. Exit cleanly so the UI unlocks; the next user
                // message will flow into the same session as the tool's reply.
                if awaiting_user_reply {
                    exit_reason = "awaiting_user_reply";
                    completed = true;
                    tracing::info!(
                        session_id = %session_id_bg,
                        log_chars = log_buf.len(),
                        tool_calls = tool_calls_announced.len(),
                        tool_completed,
                        tool_errors,
                        question = ?awaiting_question,
                        "exiting forwarder: OpenCode is awaiting user reply"
                    );
                    if !log_buf.is_empty() {
                        let _ = sqlx::query(
                            "UPDATE build_jobs SET logs = $2, updated_at = NOW() WHERE id = $1",
                        )
                        .bind(build_id_bg)
                        .bind(&log_buf)
                        .execute(&pool_bg)
                        .await;
                    }
                    break;
                }

                // Progress heartbeat: every 30 s log activity counters so we
                // can see silent stalls in production without verbose mode.
                let now = tokio::time::Instant::now();
                if now.duration_since(last_heartbeat_log).as_secs() >= 30 {
                    last_heartbeat_log = now;
                    tracing::info!(
                        session_id = %session_id_bg,
                        elapsed_secs = forwarder_started.elapsed().as_secs(),
                        idle_secs = now.duration_since(last_progress).as_secs(),
                        log_chars = log_buf.len(),
                        tool_calls = tool_calls_announced.len(),
                        tool_completed,
                        tool_errors,
                        heartbeats,
                        stream_errors,
                        "SSE forwarder progress"
                    );
                }

                let next = tokio::time::timeout_at(deadline, stream.next()).await;
                match next {
                    Err(_) => {
                        exit_reason = "deadline_timeout";
                        tracing::warn!(
                            session_id = %session_id_bg,
                            timeout_secs = sse_secs,
                            idle_for_secs = last_progress.elapsed().as_secs(),
                            log_chars = log_buf.len(),
                            tool_calls = tool_calls_announced.len(),
                            "SSE forwarder timed out"
                        );
                        break;
                    }
                    Ok(None) => {
                        exit_reason = "stream_closed_by_server";
                        tracing::info!(
                            session_id = %session_id_bg,
                            elapsed_secs = forwarder_started.elapsed().as_secs(),
                            log_chars = log_buf.len(),
                            "SSE stream closed by server"
                        );
                        break;
                    }
                    Ok(Some(event_result)) => match event_result {
                        Ok(opencode_client::OpenCodeEvent::ServerConnected) => {
                            tracing::info!("SSE connected to OpenCode event stream");
                            if let Some(tx) = connected_tx.take() {
                                let _ = tx.send(());
                            }
                        }
                        Ok(opencode_client::OpenCodeEvent::ServerHeartbeat) => {
                            heartbeats = heartbeats.saturating_add(1);
                        }
                        Ok(opencode_client::OpenCodeEvent::MessagePartDelta { properties }) => {
                            let field = properties
                                .get("field")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            if !matches!(field, "text" | "reasoning") {
                                continue;
                            }
                            // Delta events are the authoritative streamed
                            // source for text/reasoning. We intentionally
                            // ignore `message.part.updated` snapshots for the
                            // same content (see below), so there is nothing
                            // to reconcile here.
                            if let Some(delta) = properties.get("delta").and_then(|v| v.as_str()) {
                                if !delta.is_empty() {
                                    log_buf.push_str(delta);
                                    last_progress = tokio::time::Instant::now();
                                    publish_event(
                                        proj_id,
                                        BuildEvent::MessageChunk {
                                            job_id: job_id_bg.clone(),
                                            text: delta.to_string(),
                                        },
                                    )
                                    .await;
                                }
                            }
                        }
                        Ok(opencode_client::OpenCodeEvent::MessagePartUpdated {
                            properties,
                        }) => {
                            if let Some(part) = properties.get("part") {
                                if part.get("type").and_then(|t| t.as_str()) == Some("tool") {
                                    if let Some(call_id) =
                                        part.get("callID").and_then(|v| v.as_str())
                                    {
                                        let tool_name = part
                                            .get("tool")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("tool")
                                            .to_string();
                                        let status = part
                                            .get("state")
                                            .and_then(|s| s.get("status"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();

                                        // Log status transitions so tool errors and
                                        // stuck tools are visible in the host logs.
                                        let prev = tool_state
                                            .insert(
                                                call_id.to_string(),
                                                (tool_name.clone(), status.clone()),
                                            )
                                            .map(|(_, s)| s)
                                            .unwrap_or_default();
                                        if status != prev && !status.is_empty() {
                                            let is_error = status == "error"
                                                || part
                                                    .get("state")
                                                    .and_then(|s| s.get("error"))
                                                    .is_some();
                                            if is_error {
                                                tool_errors = tool_errors.saturating_add(1);
                                                let err = part
                                                    .get("state")
                                                    .and_then(|s| s.get("error"))
                                                    .cloned()
                                                    .unwrap_or(serde_json::Value::Null);
                                                tracing::error!(
                                                    session_id = %session_id_bg,
                                                    call_id,
                                                    tool = %tool_name,
                                                    status = %status,
                                                    error = %err,
                                                    "OpenCode tool reported error"
                                                );
                                            } else if status == "completed" {
                                                tool_completed = tool_completed.saturating_add(1);
                                                tracing::debug!(
                                                    call_id,
                                                    tool = %tool_name,
                                                    "tool completed"
                                                );
                                            } else if verbose {
                                                tracing::info!(
                                                    call_id,
                                                    tool = %tool_name,
                                                    prev_status = %prev,
                                                    status = %status,
                                                    "tool state transition"
                                                );
                                            }
                                        }

                                        // Detect OpenCode's interactive tools
                                        // that block the agent loop waiting
                                        // for a user reply. Treat them as
                                        // "soft completion" so the frontend
                                        // unlocks and the next user message
                                        // reaches OpenCode as the answer.
                                        if !awaiting_user_reply
                                            && status == "pending"
                                            && matches!(
                                                tool_name.as_str(),
                                                "question" | "ask" | "user_input"
                                            )
                                        {
                                            awaiting_user_reply = true;
                                            awaiting_question = part
                                                .get("state")
                                                .and_then(|s| s.get("input"))
                                                .and_then(|v| {
                                                    v.get("question")
                                                        .or_else(|| v.get("prompt"))
                                                        .or_else(|| v.get("text"))
                                                })
                                                .and_then(|v| v.as_str())
                                                .map(str::to_string);
                                            // Surface the question as an
                                            // assistant chunk so it renders
                                            // in the chat panel — the tool
                                            // input is not in the text stream
                                            // otherwise.
                                            if let Some(q) = awaiting_question.as_deref() {
                                                let suffix =
                                                    if log_buf.ends_with('\n') || log_buf.is_empty() {
                                                        ""
                                                    } else {
                                                        "\n\n"
                                                    };
                                                let rendered = format!("{suffix}{q}");
                                                log_buf.push_str(&rendered);
                                                publish_event(
                                                    proj_id,
                                                    BuildEvent::MessageChunk {
                                                        job_id: job_id_bg.clone(),
                                                        text: rendered,
                                                    },
                                                )
                                                .await;
                                            }
                                            tracing::info!(
                                                session_id = %session_id_bg,
                                                call_id,
                                                tool = %tool_name,
                                                question = ?awaiting_question,
                                                "OpenCode is awaiting user reply — short-circuiting build"
                                            );
                                        }

                                        if tool_calls_announced.insert(call_id.to_string()) {
                                            let args = part
                                                .get("state")
                                                .and_then(|s| s.get("input"))
                                                .cloned()
                                                .unwrap_or(serde_json::json!({}));
                                            tracing::info!(
                                                session_id = %session_id_bg,
                                                call_id,
                                                tool = %tool_name,
                                                status = %status,
                                                "OpenCode tool call"
                                            );
                                            publish_event(
                                                proj_id,
                                                BuildEvent::ToolCall {
                                                    job_id: job_id_bg.clone(),
                                                    tool: tool_name,
                                                    args,
                                                },
                                            )
                                            .await;
                                            last_progress = tokio::time::Instant::now();
                                        }
                                    }
                                    continue;
                                }

                                // Text and reasoning parts are streamed via
                                // `message.part.delta` events, which are the
                                // authoritative source. OpenCode also emits
                                // `message.part.updated` snapshots for the same
                                // content — any diff math here risks double-
                                // emission whenever delta's `partID` and
                                // updated's `part.id` disagree or when a
                                // snapshot arrives before the equivalent
                                // deltas finish. We deliberately ignore
                                // text/reasoning snapshots: the tool branch
                                // above is the only thing we consume from
                                // `message.part.updated`.
                            }
                        }
                        Ok(opencode_client::OpenCodeEvent::MessageCompleted { properties }) => {
                            message_completed_seen = true;
                            exit_reason = "message_completed";
                            let msg_id = properties
                                .get("info")
                                .and_then(|v| v.get("id"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            tracing::info!(
                                session_id = %session_id_bg,
                                message_id = %msg_id,
                                log_chars = log_buf.len(),
                                tool_calls = tool_calls_announced.len(),
                                tool_completed,
                                tool_errors,
                                "OpenCode message completed, flushing logs"
                            );
                            completed = true;
                            if !log_buf.is_empty() {
                                let _ = sqlx::query(
                                    "UPDATE build_jobs SET logs = $2, updated_at = NOW() WHERE id = $1",
                                )
                                .bind(build_id_bg)
                                .bind(&log_buf)
                                .execute(&pool_bg)
                                .await;
                            }
                            break;
                        }
                        Ok(opencode_client::OpenCodeEvent::SessionIdle { properties }) => {
                            let sid = properties
                                .get("sessionID")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let matches_session = sid == session_id_bg.as_str();
                            if matches_session {
                                session_idle_matched = true;
                                exit_reason = "session_idle";
                                tracing::info!(
                                    session_id = %session_id_bg,
                                    log_chars = log_buf.len(),
                                    tool_calls = tool_calls_announced.len(),
                                    tool_completed,
                                    tool_errors,
                                    elapsed_secs = forwarder_started.elapsed().as_secs(),
                                    "OpenCode session idle, flushing logs"
                                );
                                completed = true;
                                if !log_buf.is_empty() {
                                    let _ = sqlx::query(
                                        "UPDATE build_jobs SET logs = $2, updated_at = NOW() WHERE id = $1",
                                    )
                                    .bind(build_id_bg)
                                    .bind(&log_buf)
                                    .execute(&pool_bg)
                                    .await;
                                }
                                break;
                            } else if verbose {
                                tracing::info!(
                                    other_session = %sid,
                                    "session.idle for different session — ignoring"
                                );
                            }
                        }
                        Ok(opencode_client::OpenCodeEvent::SessionUpdated { properties }) => {
                            if verbose {
                                tracing::info!(?properties, "session.updated");
                            }
                        }
                        Ok(opencode_client::OpenCodeEvent::Unknown { raw_type, data }) => {
                            // Surface potentially-fatal events: session errors,
                            // permission prompts, rate-limit notices, etc. These
                            // previously went to DEBUG and were invisible.
                            let is_session_error = raw_type == "session.error"
                                || raw_type.ends_with(".error")
                                || raw_type.contains("abort");
                            if is_session_error {
                                let summary = summarize_session_error(&data);
                                tracing::error!(
                                    session_id = %session_id_bg,
                                    event_type = %raw_type,
                                    summary = %summary,
                                    payload = %data,
                                    "OpenCode error event"
                                );
                                if provider_error.is_none() {
                                    provider_error = Some(summary.clone());
                                }
                                // Surface the provider error as a chat chunk so
                                // the user sees something actionable, not just
                                // a silent failure.
                                let chunk = format!("\n[provider error] {summary}\n");
                                log_buf.push_str(&chunk);
                                publish_event(
                                    proj_id,
                                    BuildEvent::MessageChunk {
                                        job_id: job_id_bg.clone(),
                                        text: chunk,
                                    },
                                )
                                .await;
                            } else if raw_type.starts_with("tool.") {
                                let tool = data
                                    .get("tool")
                                    .or_else(|| data.get("toolName"))
                                    .or_else(|| data.get("name"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_else(|| raw_type.strip_prefix("tool.").unwrap_or(&raw_type))
                                    .to_string();
                                tracing::info!(
                                    session_id = %session_id_bg,
                                    event_type = %raw_type,
                                    tool = %tool,
                                    "OpenCode tool event (unknown shape)"
                                );
                                publish_event(
                                    proj_id,
                                    BuildEvent::ToolCall {
                                        job_id: job_id_bg.clone(),
                                        tool,
                                        args: data,
                                    },
                                )
                                .await;
                            } else if raw_type != "server.heartbeat" {
                                if verbose {
                                    tracing::info!(
                                        event_type = %raw_type,
                                        payload = %data,
                                        "SSE event from OpenCode (unhandled)"
                                    );
                                } else {
                                    tracing::debug!(event_type = %raw_type, "SSE event from OpenCode (unhandled)");
                                }
                            }
                        }
                        Ok(ev) => {
                            if verbose {
                                tracing::info!(?ev, "SSE event (other)");
                            } else {
                                tracing::debug!(?ev, "SSE event (other)");
                            }
                        }
                        Err(e) => {
                            // Transient body-decode errors happen on long idle
                            // periods; the underlying EventSource auto-retries
                            // with Last-Event-ID, so keep polling. The overall
                            // `sse_secs` deadline still bounds the wait.
                            stream_errors = stream_errors.saturating_add(1);
                            tracing::warn!(
                                session_id = %session_id_bg,
                                error = %e,
                                stream_errors,
                                elapsed_secs = forwarder_started.elapsed().as_secs(),
                                "SSE stream error (continuing — EventSource will retry)"
                            );
                            continue;
                        }
                    },
                }
            }
            // One-shot summary that is easy to grep for when triaging builds.
            // All counters are captured; `exit_reason` is the primary diagnosis.
            tracing::info!(
                session_id = %session_id_bg,
                exit_reason,
                completed,
                message_completed_seen,
                session_idle_matched,
                log_chars = log_buf.len(),
                tool_calls = tool_calls_announced.len(),
                tool_completed,
                tool_errors,
                stream_errors,
                heartbeats,
                elapsed_secs = forwarder_started.elapsed().as_secs(),
                "SSE forwarder exit summary"
            );

            ForwarderOutcome {
                log_buf: if completed { Some(log_buf) } else { None },
                provider_error,
            }
        });

        // Wait up to 15s for the SSE stream to connect before sending prompt
        match tokio::time::timeout(
            tokio::time::Duration::from_secs(15),
            connected_rx,
        )
        .await
        {
            Ok(Ok(())) => tracing::info!("SSE stream ready, sending prompt"),
            Ok(Err(_)) => {
                tracing::warn!("SSE forwarder dropped before connecting, sending prompt anyway");
            }
            Err(_) => {
                tracing::warn!("SSE connect timed out after 15s, sending prompt anyway");
            }
        }

        // ── Send the prompt (fire-and-forget) ────────────────
        // Model is configured server-side via OPENCODE_CONFIG_CONTENT;
        // the per-request model field uses a different format (object)
        // and is only needed for overrides, so we omit it.
        let oc_system = opencode_build_system_prompt(project_scope.as_str());
        tracing::info!(
            session_id = %session.id,
            prompt_chars = prompt.len(),
            system_chars = oc_system.as_deref().map(|s| s.len()).unwrap_or(0),
            project_scope = %project_scope,
            model = %model,
            "sending OpenCode prompt"
        );
        client
            .prompt_async(
                &session.id,
                vec![Part::Text {
                    text: prompt.clone(),
                }],
                None,
                oc_system.as_deref(),
            )
            .await
            .map_err(|e| {
                tracing::error!(
                    session_id = %session.id,
                    error = %e,
                    "OpenCode prompt_async failed"
                );
                RunBuildError::AiProviderError(e.to_string())
            })?;

        tracing::info!(session_id = %session.id, "prompt sent, waiting for completion via SSE");

        // Wait for the event forwarder to finish (session idle or legacy message.completed).
        let forwarder_outcome = forwarder.await.unwrap_or(ForwarderOutcome {
            log_buf: None,
            provider_error: Some("forwarder task panicked".to_string()),
        });
        let provider_error = forwarder_outcome.provider_error.clone();
        let sse_observed_idle = forwarder_outcome.log_buf.is_some();
        if !sse_observed_idle {
            tracing::warn!(
                session_id = %session.id,
                "SSE forwarder did not observe session idle — \
                 OpenCode may not have processed the prompt"
            );
            // Fall through: try to collect whatever diffs exist anyway.
        }

        let assistant_content = forwarder_outcome.log_buf.unwrap_or_default();

        // Preview of the assistant reply — truncated to keep logs readable but
        // long enough to catch "I was unable to …" style graceful failures
        // that the UI would otherwise swallow.
        let assistant_preview: String = assistant_content.chars().take(400).collect();
        tracing::info!(
            session_id = %session.id,
            assistant_chars = assistant_content.len(),
            assistant_preview = %assistant_preview,
            "OpenCode assistant reply"
        );

        // ── Collect diffs as artifacts ─────────────────────────
        let diffs = match client.session_diff(&session.id).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(
                    session_id = %session.id,
                    error = %e,
                    "session_diff request failed — treating as empty diff set"
                );
                Vec::new()
            }
        };
        // Log a sample of paths so we can see what OpenCode actually touched.
        let diff_paths_preview: Vec<&str> = diffs
            .iter()
            .take(40)
            .map(|d| d.path.as_str())
            .collect();
        tracing::info!(
            session_id = %session.id,
            diff_count = diffs.len(),
            paths = ?diff_paths_preview,
            truncated = diffs.len() > diff_paths_preview.len(),
            "collected OpenCode session diff"
        );

        // Guard: upstream provider error (e.g. 502 "Network connection lost"
        // from the model backend). OpenCode emits `session.error` and then
        // idles immediately — without this branch the build would be marked
        // "succeeded" with 0 artifacts, trip the demo gate, and silently
        // hide the real failure. Fail loudly and keep any partial diffs
        // for diagnostics (artifacts are persisted below only when the
        // branch above does NOT fire).
        if let Some(err_msg) = provider_error.as_ref() {
            tracing::error!(
                session_id = %session.id,
                provider_error = %err_msg,
                diff_count = diffs.len(),
                "aborting run_build: OpenCode reported a provider error"
            );

            let _ = sqlx::query(
                "UPDATE build_jobs \
                 SET status = 'failed', error_message = $2, duration_ms = $3, \
                     updated_at = NOW() \
                 WHERE id = $1",
            )
            .bind(build_id)
            .bind(err_msg)
            .bind(started.elapsed().as_millis() as i64)
            .execute(pool)
            .await;

            publish_event(
                project_id,
                BuildEvent::BuildError {
                    job_id: build_id.to_string(),
                    error: err_msg.clone(),
                },
            )
            .await;

            return Err(RunBuildError::AiProviderError(err_msg.clone()));
        }

        // Guard against a false-success cascade: if we never saw session-idle
        // AND OpenCode produced no diffs, the prompt almost certainly did not
        // run to completion (common cause: reqwest body-decode drop on a long
        // stream). Marking this "succeeded" would wrongly trip the demo gate,
        // trigger a no-op Vite restart, and hide the failure from the user.
        // Fail loudly instead so the UI can surface an actionable error.
        if !sse_observed_idle && diffs.is_empty() {
            let err_msg = "OpenCode did not report completion and produced no \
                changes — SSE stream likely dropped mid-build. Retry the prompt.";
            tracing::error!(
                session_id = %session.id,
                %err_msg,
                "aborting run_build without marking succeeded"
            );

            let _ = sqlx::query(
                "UPDATE build_jobs \
                 SET status = 'failed', error_message = $2, duration_ms = $3, \
                     updated_at = NOW() \
                 WHERE id = $1",
            )
            .bind(build_id)
            .bind(err_msg)
            .bind(started.elapsed().as_millis() as i64)
            .execute(pool)
            .await;

            publish_event(
                project_id,
                BuildEvent::BuildError {
                    job_id: build_id.to_string(),
                    error: err_msg.to_string(),
                },
            )
            .await;

            return Err(RunBuildError::AiProviderError(err_msg.to_string()));
        }

        let mut artifacts_count: i32 = 0;
        for diff in &diffs {
            let hash = format!("{:x}", fnv_hash(diff.path.as_bytes()));
            let size = (diff.additions + diff.deletions) as i64;
            let lang = detect_language(&diff.path);

            sqlx::query(
                "INSERT INTO artifacts \
                     (id, file_path, content_hash, size_bytes, language, \
                      build_job_id, project_id, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, NOW(), NOW())",
            )
            .bind(uuid::Uuid::new_v4())
            .bind(&diff.path)
            .bind(&hash)
            .bind(size)
            .bind(lang.as_deref())
            .bind(build_id)
            .bind(project_id)
            .execute(pool)
            .await
            .map_err(|e: sqlx::Error| RunBuildError::BuildFailed(e.to_string()))?;

            artifacts_count += 1;
        }

        let duration_ms = started.elapsed().as_millis() as i64;
        let tokens_used: i64 = 0; // token tracking not available in async mode

        // ── Persist assistant message ─────────────────────────
        if !assistant_content.is_empty() {
            let message_id: uuid::Uuid = build_row.get("message_id");
            let conv_row =
                sqlx::query("SELECT conversation_id, author_id FROM messages WHERE id = $1")
                    .bind(message_id)
                    .fetch_optional(pool)
                    .await
                    .map_err(|e: sqlx::Error| RunBuildError::BuildFailed(e.to_string()))?;

            if let Some(conv) = conv_row {
                let conversation_id: uuid::Uuid = conv.get("conversation_id");
                let author_id: uuid::Uuid = conv.get("author_id");

                sqlx::query(
                    "INSERT INTO messages \
                         (id, role, content, sort_order, has_attachment, \
                          conversation_id, author_id, created_at, updated_at) \
                     VALUES ($1, 'assistant', $2, \
                             (SELECT COALESCE(MAX(sort_order),0)+1 FROM messages WHERE conversation_id = $3), \
                             false, $3, $4, NOW(), NOW())",
                )
                .bind(uuid::Uuid::new_v4())
                .bind(&assistant_content)
                .bind(conversation_id)
                .bind(author_id)
                .execute(pool)
                .await
                .map_err(|e: sqlx::Error| RunBuildError::BuildFailed(e.to_string()))?;
            }
        }

        // ── Mark build as succeeded ───────────────────────────
        sqlx::query(
            "UPDATE build_jobs \
             SET status = 'succeeded', tokens_used = $2, duration_ms = $3, updated_at = NOW() \
             WHERE id = $1",
        )
        .bind(build_id)
        .bind(tokens_used)
        .bind(duration_ms)
        .execute(pool)
        .await
        .map_err(|e: sqlx::Error| RunBuildError::BuildFailed(e.to_string()))?;

        // ── Demo gate: mark project scope as "free" after first real build ──
        // Only count turns that actually changed files. OpenCode often replies
        // with a clarifying question (e.g. "¿podrías darme más contexto?") and
        // produces zero diffs — those must NOT consume the free-tier quota,
        // otherwise the user can't even answer the question.
        if model != "opencode-repair" && artifacts_count > 0 {
            sqlx::query(
                "UPDATE projects SET scope = 'free', updated_at = NOW() \
                 WHERE id = $1 AND scope != 'free'",
            )
            .bind(project_id)
            .execute(pool)
            .await
            .map_err(|e: sqlx::Error| RunBuildError::BuildFailed(e.to_string()))?;
            tracing::info!(%project_id, artifacts_count, "project scope set to 'free' (demo limit)");
        } else if model != "opencode-repair" {
            tracing::info!(
                %project_id,
                "skip demo-gate: build produced 0 artifacts (likely a clarifying question)"
            );
        }

        // ── Record usage ──────────────────────────────────────
        sqlx::query(
            "INSERT INTO usage_records \
                 (id, kind, quantity, description, org_id, project_id, created_at, updated_at) \
             VALUES ($1, 'build', $2, $3, $4, $5, NOW(), NOW())",
        )
        .bind(uuid::Uuid::new_v4())
        .bind(tokens_used)
        .bind(format!(
            "Build job {} — {} artifacts",
            build_id, artifacts_count
        ))
        .bind(org_id)
        .bind(project_id)
        .execute(pool)
        .await
        .map_err(|e: sqlx::Error| RunBuildError::BuildFailed(e.to_string()))?;

        publish_event(
            project_id,
            BuildEvent::BuildComplete {
                job_id: build_id.to_string(),
                status: "succeeded".to_string(),
                artifacts_count,
                tokens_used,
            },
        )
        .await;

        // Only restart the dev server when files actually changed. A 0-artifact
        // turn (clarifying question, thinking-only reply, webfetch-only research)
        // doesn't need a Vite restart — and restarting causes the iframe to
        // show a 502 Bad Gateway for ~5 s for no reason.
        if model != "opencode-repair" && artifacts_count > 0 {
            super::spawn_environment::restart_deployment_after_opencode_build(pool, project_id)
                .await;
        } else if model == "opencode-repair" {
            tracing::debug!(
                %project_id,
                "skip deploy restart after opencode-repair — outer restart loop owns recovery"
            );
        } else {
            tracing::info!(
                %project_id,
                "skip deploy restart: build produced 0 artifacts"
            );
        }

        tracing::info!(
            artifacts_count,
            tokens_used,
            duration_ms,
            session_id = %session.id,
            model = %model,
            "build completed via OpenCode"
        );

        Ok(RunBuildOutput {
            artifacts_count,
            tokens_used,
            status: "succeeded".to_string(),
        })

        }
        .instrument(span)
        .await
    }
}

// ── Helpers ───────────────────────────────────────────────────────

async fn publish_event(project_id: uuid::Uuid, event: BuildEvent) {
    let bus = event_bus();
    let readers = bus.read().await;
    if let Some(tx) = readers.get(&project_id) {
        let _ = tx.send(event);
    }
}

/// Finds the project's working directory by looking at the deployment's
/// original build_job_id (SpawnEnvironment clones into /tmp/stem-cell-{job_id}).
/// Falls back to /tmp/stem-cell-projects/{slug} if no deployment exists.
async fn resolve_work_dir_for_project(
    pool: &sqlx::PgPool,
    project_id: uuid::Uuid,
) -> std::result::Result<std::path::PathBuf, String> {
    // Find the deployment's build_job_id — that's where the code lives
    let deploy_row = sqlx::query(
        "SELECT build_job_id FROM deployments \
         WHERE project_id = $1 AND deleted_at IS NULL \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("query deployments: {e}"))?;

    if let Some(row) = deploy_row {
        let spawn_job_id: uuid::Uuid = row.get("build_job_id");
        let dir = std::path::PathBuf::from(format!("/tmp/stem-cell-{spawn_job_id}"));
        if dir.exists() {
            return Ok(dir);
        }
    }

    // Spawn in progress (no deployment row yet): env checkout is /tmp/stem-cell-{spawn job id}
    let spawn_row = sqlx::query(
        "SELECT id FROM build_jobs \
         WHERE project_id = $1 AND model = 'container' \
           AND deployment_id IS NULL AND status = 'running' AND deleted_at IS NULL \
         ORDER BY created_at ASC LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("query spawn build_job: {e}"))?;

    if let Some(row) = spawn_row {
        let spawn_id: uuid::Uuid = row.get("id");
        let dir = std::path::PathBuf::from(format!("/tmp/stem-cell-{spawn_id}"));
        if dir.exists() {
            return Ok(dir);
        }
    }

    // Fallback: use slug-based directory
    let slug_row = sqlx::query("SELECT slug FROM projects WHERE id = $1")
        .bind(project_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("query project slug: {e}"))?;

    let slug: String = slug_row
        .map(|r| r.get("slug"))
        .unwrap_or_else(|| project_id.to_string());

    let base =
        std::env::var("OPENCODE_WORKDIR_BASE").unwrap_or_else(|_| "/tmp/stem-cell-projects".into());
    Ok(std::path::PathBuf::from(base).join(slug))
}

fn fnv_hash(data: &[u8]) -> u128 {
    let mut hash: u128 = 0x6c62272e07bb0142_62b821756295c58d;
    for &byte in data {
        hash ^= byte as u128;
        hash = hash.wrapping_mul(0x0000000001000000_000000000000013B);
    }
    hash
}

fn detect_language(path: &str) -> Option<String> {
    let ext = path.rsplit('.').next()?;
    match ext {
        "ts" | "tsx" => Some("typescript".into()),
        "js" | "jsx" => Some("javascript".into()),
        "rs" => Some("rust".into()),
        "py" => Some("python".into()),
        "css" => Some("css".into()),
        "html" => Some("html".into()),
        "json" => Some("json".into()),
        "yaml" | "yml" => Some("yaml".into()),
        "md" => Some("markdown".into()),
        "sql" => Some("sql".into()),
        "toml" => Some("toml".into()),
        _ => None,
    }
}

fn simple_base64(input: &[u8]) -> String {
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
