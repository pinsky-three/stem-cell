use crate::system_api::*;
use opencode_client::types::{BuildEvent, Part};
use sqlx::Row;
use std::sync::{Arc, OnceLock};
use tokio::sync::broadcast;

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

#[async_trait::async_trait]
impl RunBuildSystem for super::AppSystems {
    async fn execute(
        &self,
        pool: &sqlx::PgPool,
        input: RunBuildInput,
    ) -> Result<RunBuildOutput, RunBuildError> {
        let span = tracing::info_span!("run_build", build_job_id = %input.build_job_id);
        let _enter = span.enter();

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
        let existing_session: Option<String> = build_row.get("opencode_session_id");

        // ── Load project ──────────────────────────────────────
        let project_row = sqlx::query(
            "SELECT id, slug, org_id FROM projects WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(project_id)
        .fetch_optional(pool)
        .await
        .map_err(|e: sqlx::Error| RunBuildError::BuildFailed(e.to_string()))?
        .ok_or(RunBuildError::ProjectNotFound)?;

        let org_id: uuid::Uuid = project_row.get("org_id");
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
            Some(ref sid) => client
                .get_session(sid)
                .await
                .map_err(|e| RunBuildError::AiProviderError(e.to_string()))?,
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

        // Signal from forwarder → main task when SSE is connected
        let (connected_tx, connected_rx) = tokio::sync::oneshot::channel::<()>();

        let forwarder = tokio::spawn(async move {
            use futures::StreamExt;
            let mut stream = std::pin::pin!(event_stream);
            let mut log_buf = String::new();
            let mut connected_tx = Some(connected_tx);
            let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(600);

            loop {
                let next = tokio::time::timeout_at(deadline, stream.next()).await;
                match next {
                    Err(_) => {
                        tracing::warn!("SSE forwarder timed out after 10 min");
                        break;
                    }
                    Ok(None) => {
                        tracing::info!("SSE stream closed by server");
                        break;
                    }
                    Ok(Some(event_result)) => match event_result {
                        Ok(opencode_client::OpenCodeEvent::ServerConnected) => {
                            tracing::info!("SSE connected to OpenCode event stream");
                            if let Some(tx) = connected_tx.take() {
                                let _ = tx.send(());
                            }
                        }
                        Ok(opencode_client::OpenCodeEvent::MessagePartUpdated {
                            properties,
                        }) => {
                            let text = properties
                                .get("content")
                                .and_then(|v| v.get("content"))
                                .and_then(|v| v.as_str())
                                .or_else(|| properties.get("text").and_then(|v| v.as_str()))
                                .unwrap_or("")
                                .to_string();

                            if !text.is_empty() {
                                log_buf.push_str(&text);
                                publish_event(
                                    proj_id,
                                    BuildEvent::MessageChunk {
                                        job_id: job_id_bg.clone(),
                                        text,
                                    },
                                )
                                .await;
                            }
                        }
                        Ok(opencode_client::OpenCodeEvent::MessageCompleted { .. }) => {
                            tracing::info!("OpenCode message completed, flushing logs");
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
                        Ok(opencode_client::OpenCodeEvent::Unknown { raw_type, data }) => {
                            tracing::info!(event_type = %raw_type, "SSE event from OpenCode");
                            if raw_type.starts_with("tool.") {
                                let tool = data
                                    .get("tool")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&raw_type)
                                    .to_string();
                                publish_event(
                                    proj_id,
                                    BuildEvent::ToolCall {
                                        job_id: job_id_bg.clone(),
                                        tool,
                                        args: data,
                                    },
                                )
                                .await;
                            }
                        }
                        Ok(ev) => {
                            tracing::info!(?ev, "SSE event (other)");
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "SSE stream error");
                            break;
                        }
                    },
                }
            }
            log_buf
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
        let oc_model = pm.config().default_model.as_deref();

        client
            .prompt_async(
                &session.id,
                vec![Part::Text {
                    text: prompt.clone(),
                }],
                oc_model,
            )
            .await
            .map_err(|e| RunBuildError::AiProviderError(e.to_string()))?;

        tracing::info!(session_id = %session.id, "prompt sent, waiting for completion via SSE");

        // Wait for the event forwarder to finish (it breaks on MessageCompleted)
        let assistant_content = forwarder.await.unwrap_or_default();

        // ── Collect diffs as artifacts ─────────────────────────
        let diffs = client.session_diff(&session.id).await.unwrap_or_default();

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

        tracing::info!(
            artifacts_count,
            tokens_used,
            duration_ms,
            session_id = %session.id,
            "build completed via OpenCode"
        );

        Ok(RunBuildOutput {
            artifacts_count,
            tokens_used,
            status: "succeeded".to_string(),
        })
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
