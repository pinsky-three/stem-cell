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

/// Per-request model reference. OpenCode's `/session/:id/message` and
/// `/session/:id/prompt_async` endpoints expect the `model` field to be
/// an object `{ providerID, modelID }` — **not** a string. Passing a
/// plain string yields HTTP 400 (`invalid_type: expected object`).
///
/// Use `ModelRef::parse("openrouter/minimax/minimax-m2.5")` to split on
/// the first `/`: the prefix becomes `providerID`, the remainder is the
/// `modelID` (which may itself contain `/`, e.g. `minimax/minimax-m2.5`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelRef {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    #[serde(rename = "modelID")]
    pub model_id: String,
}

impl ModelRef {
    /// Parses a `provider/model...` string. Returns `None` when the
    /// input has no `/` separator (OpenCode requires both halves).
    pub fn parse(s: &str) -> Option<Self> {
        let (provider, model) = s.split_once('/')?;
        if provider.is_empty() || model.is_empty() {
            return None;
        }
        Some(Self {
            provider_id: provider.to_string(),
            model_id: model.to_string(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageRequest {
    pub parts: Vec<Part>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
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
    // OpenCode has used different key names for the touched file across
    // versions (`path`, `file`, `filePath`, `file_path`). Accept all of
    // them so a rename upstream does not silently produce diffs with an
    // empty `path` (which then lands as junk rows in `artifacts`).
    #[serde(default, alias = "file", alias = "filePath", alias = "file_path")]
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
    /// Keep-alive from OpenCode (`server.heartbeat`); safe to ignore.
    ServerHeartbeat,
    MessagePartUpdated {
        #[serde(default)]
        properties: serde_json::Value,
    },
    MessagePartDelta {
        #[serde(default)]
        properties: serde_json::Value,
    },
    MessageCompleted {
        #[serde(default)]
        properties: serde_json::Value,
    },
    /// Fired when the session returns to idle (prompt / agent loop finished).
    SessionIdle {
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

/// OpenCode's project `/event` stream uses Hono `writeSSE({ data })` only, so the SSE
/// event name is usually `message`. The real bus type lives in JSON: `{ "type", "properties" }`.
fn try_parse_bus_payload(data: &str) -> Option<(String, serde_json::Value)> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    let obj = v.as_object()?;

    if let Some(payload) = obj.get("payload").and_then(|p| p.as_object()) {
        let t = payload.get("type")?.as_str()?;
        let props = payload
            .get("properties")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        return Some((t.to_string(), props));
    }

    let t = obj.get("type")?.as_str()?;
    let props = obj
        .get("properties")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Some((t.to_string(), props))
}

fn from_bus_type(bus_type: &str, properties: serde_json::Value) -> OpenCodeEvent {
    match bus_type {
        "server.connected" => OpenCodeEvent::ServerConnected,
        "server.heartbeat" => OpenCodeEvent::ServerHeartbeat,
        "message.part.updated" => OpenCodeEvent::MessagePartUpdated { properties },
        "message.part.delta" => OpenCodeEvent::MessagePartDelta { properties },
        "message.completed" => OpenCodeEvent::MessageCompleted { properties },
        "session.updated" => OpenCodeEvent::SessionUpdated { properties },
        "session.idle" => OpenCodeEvent::SessionIdle { properties },
        "session.status" => {
            if let Some(st) = properties.get("status") {
                if st.get("type").and_then(|t| t.as_str()) == Some("idle") {
                    return OpenCodeEvent::SessionIdle { properties };
                }
            }
            OpenCodeEvent::Unknown {
                raw_type: bus_type.to_string(),
                data: properties,
            }
        }
        other => OpenCodeEvent::Unknown {
            raw_type: other.to_string(),
            data: properties,
        },
    }
}

impl OpenCodeEvent {
    pub fn parse(event_type: &str, data: &str) -> Self {
        if let Some((bus_type, properties)) = try_parse_bus_payload(data) {
            return from_bus_type(&bus_type, properties);
        }

        match event_type {
            "server.connected" => Self::ServerConnected,
            "message.part.updated" => Self::MessagePartUpdated {
                properties: serde_json::from_str(data).unwrap_or_default(),
            },
            "message.part.delta" => Self::MessagePartDelta {
                properties: serde_json::from_str(data).unwrap_or_default(),
            },
            "message.completed" => Self::MessageCompleted {
                properties: serde_json::from_str(data).unwrap_or_default(),
            },
            "session.updated" => Self::SessionUpdated {
                properties: serde_json::from_str(data).unwrap_or_default(),
            },
            "session.idle" => Self::SessionIdle {
                properties: serde_json::from_str(data).unwrap_or_default(),
            },
            other => Self::Unknown {
                raw_type: other.to_string(),
                data: serde_json::from_str(data).unwrap_or_default(),
            },
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::MessageCompleted { .. } | Self::SessionIdle { .. }
        )
    }
}

#[cfg(test)]
mod model_ref_tests {
    use super::*;

    #[test]
    fn parses_simple_provider_model() {
        let m = ModelRef::parse("anthropic/claude-3-5-sonnet").expect("ok");
        assert_eq!(m.provider_id, "anthropic");
        assert_eq!(m.model_id, "claude-3-5-sonnet");
    }

    #[test]
    fn parses_nested_model_id_preserving_slashes() {
        // openrouter routes models as `<owner>/<model>` inside modelID.
        let m = ModelRef::parse("openrouter/minimax/minimax-m2.5").expect("ok");
        assert_eq!(m.provider_id, "openrouter");
        assert_eq!(m.model_id, "minimax/minimax-m2.5");
    }

    #[test]
    fn rejects_string_without_slash() {
        assert!(ModelRef::parse("just-a-name").is_none());
    }

    #[test]
    fn rejects_empty_halves() {
        assert!(ModelRef::parse("/model").is_none());
        assert!(ModelRef::parse("provider/").is_none());
    }

    #[test]
    fn serializes_with_camelcase_id_suffix() {
        let m = ModelRef {
            provider_id: "openrouter".into(),
            model_id: "minimax/minimax-m2.5".into(),
        };
        let json = serde_json::to_string(&m).expect("ser");
        assert!(json.contains(r#""providerID":"openrouter""#));
        assert!(json.contains(r#""modelID":"minimax/minimax-m2.5""#));
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
    /// Deploy lifecycle phases (clone, install, healthy, opencode_starting).
    /// Emitted by SpawnEnvironment so the frontend can show progress before OpenCode begins.
    DeployStatus {
        job_id: String,
        project_id: String,
        phase: String,
        message: String,
    },
}
