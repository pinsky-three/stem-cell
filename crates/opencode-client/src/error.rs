#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("OpenCode server not healthy after {elapsed_ms}ms")]
    HealthTimeout { elapsed_ms: u64 },

    #[error("failed to spawn OpenCode process: {0}")]
    SpawnFailed(String),

    #[error("OpenCode server returned {status}: {body}")]
    ApiError {
        status: reqwest::StatusCode,
        body: String,
    },

    #[error("SSE stream error: {0}")]
    SseError(String),

    #[error("port allocation exhausted (tried {base}..{base}+{range})")]
    PortExhausted { base: u16, range: u16 },

    #[error("project {0} has no running OpenCode instance")]
    NoInstance(uuid::Uuid),
}

pub type Result<T> = std::result::Result<T, Error>;
