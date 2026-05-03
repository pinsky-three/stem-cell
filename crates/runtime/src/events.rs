use axum::Router;
use axum::extract::Path;
use axum::response::IntoResponse;
use axum::response::sse::{Event, Sse};
use axum::routing::get;
use futures::stream::Stream;
use opencode_client::types::BuildEvent;
use std::convert::Infallible;
use tokio::sync::broadcast;

use crate::systems::run_build::event_bus;

/// Mounts `GET /api/projects/{project_id}/events` — an SSE stream of
/// build events for a given project.
pub fn router() -> Router {
    Router::new().route("/api/projects/{project_id}/events", get(sse_handler))
}

async fn sse_handler(Path(project_id): Path<uuid::Uuid>) -> impl IntoResponse {
    let bus = event_bus();
    let rx = {
        let mut writers = bus.write().await;
        // Large buffer: message.chunk bursts can lag slow consumers; dropping events loses
        // build.complete and leaves the UI stale until manual refresh.
        let tx = writers
            .entry(project_id)
            .or_insert_with(|| broadcast::channel::<BuildEvent>(16_384).0);
        tx.subscribe()
    };

    let stream = make_stream(rx);
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}

fn make_stream(
    mut rx: broadcast::Receiver<BuildEvent>,
) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static {
    async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(build_event) => {
                    let data = serde_json::to_string(&build_event).unwrap_or_default();
                    let event_type = match &build_event {
                        BuildEvent::BuildStatus { .. } => "build.status",
                        BuildEvent::MessageChunk { .. } => "message.chunk",
                        BuildEvent::ToolCall { .. } => "tool.call",
                        BuildEvent::BuildComplete { .. } => "build.complete",
                        BuildEvent::BuildError { .. } => "build.error",
                        BuildEvent::DeployStatus { .. } => "deploy.status",
                    };
                    yield Ok(Event::default().event(event_type).data(data));
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "SSE consumer lagged");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    }
}
