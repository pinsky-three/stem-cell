use crate::error::{Error, Result};
use crate::redaction::Redactor;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct GitRunner {
    default_timeout: Duration,
    redactor: Redactor,
}

impl Default for GitRunner {
    fn default() -> Self {
        Self {
            default_timeout: DEFAULT_TIMEOUT,
            redactor: Redactor::new(),
        }
    }
}

impl GitRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    pub fn with_redactor(mut self, redactor: Redactor) -> Self {
        self.redactor = redactor;
        self
    }

    pub async fn run(&self, opts: GitRunOptions<'_>) -> Result<GitCommandOutput> {
        let timeout = opts.timeout.unwrap_or(self.default_timeout);
        let args_display = sanitize_args(opts.args);

        let mut cmd = Command::new("git");
        cmd.args(opts.args)
            .current_dir(opts.cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "echo");

        for (key, value) in opts.extra_env {
            cmd.env(key, value);
        }

        let cwd = opts.cwd.to_path_buf();
        let child = cmd.output();
        let output = tokio::time::timeout(timeout, child)
            .await
            .map_err(|_| Error::GitTimeout {
                cwd: cwd.clone(),
                args: self.redactor.redact(&args_display),
                timeout_secs: timeout.as_secs(),
            })?
            .map_err(|source| Error::GitSpawn {
                cwd: cwd.clone(),
                source,
            })?;

        let stdout = self
            .redactor
            .redact(&String::from_utf8_lossy(&output.stdout));
        let stderr = self
            .redactor
            .redact(&String::from_utf8_lossy(&output.stderr));
        let status = output.status.code();

        if opts.check && !output.status.success() {
            return Err(Error::GitFailed {
                cwd,
                args: self.redactor.redact(&args_display),
                status,
                stderr: stderr.trim().to_string(),
            });
        }

        Ok(GitCommandOutput {
            status,
            stdout,
            stderr,
        })
    }

    pub async fn run_ok(&self, cwd: &Path, args: &[&str]) -> Result<()> {
        self.run(GitRunOptions::new(cwd, args)).await?;
        Ok(())
    }

    pub async fn output(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        let out = self.run(GitRunOptions::new(cwd, args)).await?;
        Ok(out.stdout)
    }
}

#[derive(Debug, Clone)]
pub struct GitRunOptions<'a> {
    pub cwd: &'a Path,
    pub args: &'a [&'a str],
    pub timeout: Option<Duration>,
    pub extra_env: &'a [(&'a str, &'a str)],
    pub check: bool,
}

impl<'a> GitRunOptions<'a> {
    pub fn new(cwd: &'a Path, args: &'a [&'a str]) -> Self {
        Self {
            cwd,
            args,
            timeout: None,
            extra_env: &[],
            check: true,
        }
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn extra_env(mut self, extra_env: &'a [(&'a str, &'a str)]) -> Self {
        self.extra_env = extra_env;
        self
    }

    pub fn check(mut self, check: bool) -> Self {
        self.check = check;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommandOutput {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Default)]
pub struct CloneOptions {
    pub progress: bool,
    pub branch: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CommitOptions<'a> {
    pub message: &'a str,
    pub author_name: &'a str,
    pub author_email: &'a str,
    pub allow_empty: bool,
}

#[derive(Debug, Clone)]
pub struct PushOptions<'a> {
    pub remote_url: &'a str,
    pub branch_name: &'a str,
    pub force_with_lease: bool,
    pub redactor: Redactor,
}

pub async fn run_git(cwd: &Path, args: &[&str]) -> Result<()> {
    GitRunner::new().run_ok(cwd, args).await
}

pub async fn run_git_output(cwd: &Path, args: &[&str]) -> Result<String> {
    GitRunner::new().output(cwd, args).await
}

pub async fn clone_repository(repo_url: &str, dest: &Path, opts: CloneOptions) -> Result<PathBuf> {
    if dest.join(".git").exists() {
        tracing::info!(
            repo_url = %repo_url,
            dest = %dest.display(),
            idempotent_skip = true,
            "repo already cloned"
        );
        return Ok(dest.to_path_buf());
    }

    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| Error::Fs {
                path: parent.to_path_buf(),
                source,
            })?;
    }

    let mut owned_args = vec!["clone".to_string()];
    if opts.progress {
        owned_args.push("--progress".to_string());
    }
    if let Some(branch) = opts.branch {
        owned_args.push("--branch".to_string());
        owned_args.push(branch);
        owned_args.push("--single-branch".to_string());
    }
    owned_args.push(repo_url.to_string());
    owned_args.push(dest.to_string_lossy().to_string());

    let borrowed: Vec<&str> = owned_args.iter().map(String::as_str).collect();
    GitRunner::new().run_ok(Path::new("."), &borrowed).await?;

    tracing::info!(
        repo_url = %repo_url,
        dest = %dest.display(),
        idempotent_skip = false,
        "clone_repository: complete"
    );
    Ok(dest.to_path_buf())
}

pub async fn commit_all(cwd: &Path, opts: CommitOptions<'_>) -> Result<()> {
    run_git(cwd, &["add", "-A"]).await?;

    let mut owned_args = vec![
        "-c".to_string(),
        format!("user.name={}", opts.author_name),
        "-c".to_string(),
        format!("user.email={}", opts.author_email),
        "commit".to_string(),
    ];
    if opts.allow_empty {
        owned_args.push("--allow-empty".to_string());
    }
    owned_args.push("-m".to_string());
    owned_args.push(opts.message.to_string());

    let borrowed: Vec<&str> = owned_args.iter().map(String::as_str).collect();
    run_git(cwd, &borrowed).await
}

pub async fn push_head(cwd: &Path, opts: PushOptions<'_>) -> Result<()> {
    let refspec = format!("HEAD:refs/heads/{}", opts.branch_name);
    let mut owned_args = vec!["push".to_string()];
    if opts.force_with_lease {
        owned_args.push("--force-with-lease".to_string());
    }
    owned_args.push(opts.remote_url.to_string());
    owned_args.push(refspec);

    let borrowed: Vec<&str> = owned_args.iter().map(String::as_str).collect();
    GitRunner::new()
        .with_redactor(opts.redactor)
        .run_ok(cwd, &borrowed)
        .await
}

pub async fn rev_parse_head(cwd: &Path) -> Result<String> {
    Ok(run_git_output(cwd, &["rev-parse", "HEAD"])
        .await?
        .trim()
        .to_string())
}

fn sanitize_args(args: &[&str]) -> String {
    args.iter()
        .map(|arg| {
            if arg.contains("x-access-token:") {
                "<redacted-url>"
            } else {
                *arg
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn clone_is_idempotent_when_git_dir_exists() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("proj");
        tokio::fs::create_dir_all(dest.join(".git")).await.unwrap();

        let path = clone_repository(
            "file:///nonexistent-should-not-run",
            &dest,
            CloneOptions::default(),
        )
        .await
        .unwrap();

        assert_eq!(path, dest);
    }

    #[tokio::test]
    async fn commit_and_rev_parse_head_work_in_temp_repo() {
        let dir = tempdir().unwrap();
        run_git(dir.path(), &["init"]).await.unwrap();
        tokio::fs::write(dir.path().join("README.md"), "hello\n")
            .await
            .unwrap();
        commit_all(
            dir.path(),
            CommitOptions {
                message: "init",
                author_name: "Stem Test",
                author_email: "test@example.com",
                allow_empty: false,
            },
        )
        .await
        .unwrap();

        let sha = rev_parse_head(dir.path()).await.unwrap();
        assert!(sha.len() >= 40);
    }
}
