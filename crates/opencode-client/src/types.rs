use serde::{Deserialize, Serialize};

// ── Session ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

// ── Message parts ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Part {
    #[serde(rename = "text")]
    Text { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageRequest {
    pub parts: Vec<Part>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(rename = "messageID", skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(rename = "noReply", skip_serializing_if = "Option::is_none")]
    pub no_reply: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageInfo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageResponse {
    pub info: MessageInfo,
    #[serde(default)]
    pub parts: Vec<serde_json::Value>,
}

// ── Diffs ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub additions: i64,
    #[serde(default)]
    pub deletions: i64,
    #[serde(default)]
    pub diff: Option<String>,
}

// ── SSE events ─────────────────────────────────────────────────

/// Wrapper for events received on the `GET /event` SSE stream.
/// OpenCode sends many event types; we capture the ones we care about
/// and pass through the rest as `Unknown`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OpenCodeEvent {
    ServerConnected,
    MessagePartUpdated {
        #[serde(default)]
        properties: serde_json::Value,
    },
    MessageCompleted {
        #[serde(default)]
        properties: serde_json::Value,
    },
    SessionUpdated {
        #[serde(default)]
        properties: serde_json::Value,
    },
    Unknown {
        raw_type: String,
        data: serde_json::Value,
    },
}

impl OpenCodeEvent {
    pub fn parse(event_type: &str, data: &str) -> Self {
        match event_type {
            "server.connected" => Self::ServerConnected,
            "message.part.updated" => Self::MessagePartUpdated {
                properties: serde_json::from_str(data).unwrap_or_default(),
            },
            "message.completed" => Self::MessageCompleted {
                properties: serde_json::from_str(data).unwrap_or_default(),
            },
            "session.updated" => Self::SessionUpdated {
                properties: serde_json::from_str(data).unwrap_or_default(),
            },
            other => Self::Unknown {
                raw_type: other.to_string(),
                data: serde_json::from_str(data).unwrap_or_default(),
            },
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::MessageCompleted { .. })
    }
}

// ── Health ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    #[serde(default)]
    pub status: String,
}

// ── Build events (our bridge layer) ────────────────────────────

/// Events we forward from the backend to the frontend SSE stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum BuildEvent {
    BuildStatus {
        job_id: String,
        status: String,
    },
    MessageChunk {
        job_id: String,
        text: String,
    },
    ToolCall {
        job_id: String,
        tool: String,
        #[serde(default)]
        args: serde_json::Value,
    },
    BuildComplete {
        job_id: String,
        status: String,
        artifacts_count: i32,
        tokens_used: i64,
    },
    BuildError {
        job_id: String,
        error: String,
    },
}
