//! Structured error taxonomy. Callers match on these variants; the
//! runtime maps them to its own `SpawnEnvironmentError`, the CLI bubbles
//! them up via `anyhow`.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("git clone failed: {0}")]
    CloneFailed(String),

    #[error("toolchain install failed at phase `{phase}` (exit {exit_code})")]
    InstallFailed { phase: String, exit_code: i32 },

    #[error("template not found: {0}")]
    TemplateNotFound(String),

    #[error("invalid project path `{path}`: {reason}")]
    InvalidPath { path: String, reason: String },

    #[error("manifest parse error: {0}")]
    ManifestParse(String),

    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
