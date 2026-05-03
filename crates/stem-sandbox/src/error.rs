use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid sandbox id: {0}")]
    InvalidSandboxId(String),
    #[error("sandbox path is outside allowed root: {path}")]
    UnsafePath { path: PathBuf },
    #[error("filesystem error at {path}: {source}")]
    Fs {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("process error: {0}")]
    Process(String),
    #[error("health check timed out after {timeout_secs}s for {url}")]
    HealthTimeout { url: String, timeout_secs: u64 },
}
