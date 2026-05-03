use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("git failed in {cwd}: git {args}: {stderr}")]
    GitFailed {
        cwd: PathBuf,
        args: String,
        status: Option<i32>,
        stderr: String,
    },
    #[error("git command timed out after {timeout_secs}s in {cwd}: git {args}")]
    GitTimeout {
        cwd: PathBuf,
        args: String,
        timeout_secs: u64,
    },
    #[error("failed to spawn git in {cwd}: {source}")]
    GitSpawn {
        cwd: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("filesystem error at {path}: {source}")]
    Fs {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "GitHub App not configured: set GITHUB_APP_ID, GITHUB_APP_PRIVATE_KEY(_PATH), and GITHUB_APP_WEBHOOK_SECRET"
    )]
    GithubNotConfigured,
    #[error("failed to sign GitHub App JWT: {0}")]
    GithubJwt(String),
    #[error("GitHub API call failed ({status}): {body}")]
    GithubApi { status: u16, body: String },
    #[error("HTTP transport error: {0}")]
    GithubTransport(String),
    #[error("response could not be parsed: {0}")]
    GithubParse(String),
    #[error("{0}")]
    InvalidInput(String),
}
