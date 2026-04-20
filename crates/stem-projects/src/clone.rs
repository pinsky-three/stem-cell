//! Idempotent `git clone` wrapper.
//!
//! Mirrors the behaviour of the inline shell snippet the runtime's
//! `run_subprocess_setup` used to execute:
//!
//! ```text
//! if [ -d "{dir}/.git" ]; then echo 'repo already cloned'; else git clone {repo} "{dir}"; fi
//! ```
//!
//! Moving it to Rust buys us:
//! - Typed errors (`Error::CloneFailed`) instead of bash exit codes.
//! - Tracing spans with structured fields so dashboards can aggregate.
//! - A hook for Phase 2 to plumb through a GitHub OAuth token without
//!   touching any call sites.

use crate::ProjectPath;
use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::process::Command;

#[derive(Debug, Clone, Default)]
pub struct CloneOpts {
    /// If true, `--progress` is passed to git clone (noisier output,
    /// useful for user-facing CLIs; the runtime keeps it off).
    pub progress: bool,

    /// Optional single-branch checkout. Omit for full history.
    pub branch: Option<String>,

    /// Reserved for Phase 2: an OAuth/token-based credential supplier.
    /// Today this is always `None`; when wired, the token will be
    /// injected via `GIT_ASKPASS` or an `x-access-token` URL rewrite.
    #[doc(hidden)]
    pub auth: Option<()>,
}

/// Clones `repo_url` into `dest` if (and only if) `dest/.git` does not
/// already exist. Returns the project path either way.
///
/// Idempotency is intentional: the runtime re-invokes this on every
/// deploy so operators can retry safely, and the CLI's `stem init` does
/// the same when resuming an interrupted scaffold.
pub async fn clone_repo(
    repo_url: &str,
    dest: &Path,
    opts: CloneOpts,
) -> Result<ProjectPath> {
    let dest = PathBuf::from(dest);
    let git_dir = dest.join(".git");
    let started = Instant::now();

    if git_dir.exists() {
        tracing::info!(
            repo_url = %repo_url,
            dest = %dest.display(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            idempotent_skip = true,
            "repo already cloned"
        );
        return Ok(ProjectPath(dest));
    }

    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await?;
    }

    let mut cmd = Command::new("git");
    cmd.arg("clone");
    if opts.progress {
        cmd.arg("--progress");
    }
    if let Some(ref branch) = opts.branch {
        cmd.args(["--branch", branch, "--single-branch"]);
    }
    cmd.arg(repo_url);
    cmd.arg(&dest);
    // Silence the interactive credential prompt — if the token isn't in
    // the URL (or the future GIT_ASKPASS hook), fail fast instead of
    // blocking a server-side process on stdin.
    cmd.env("GIT_TERMINAL_PROMPT", "0");

    tracing::info!(
        repo_url = %repo_url,
        dest = %dest.display(),
        "subprocess: clone and toolchain install"
    );

    let output = cmd.output().await.map_err(|e| {
        Error::CloneFailed(format!("failed to spawn git: {e}"))
    })?;

    let elapsed_ms = started.elapsed().as_millis() as u64;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        tracing::error!(
            repo_url = %repo_url,
            dest = %dest.display(),
            elapsed_ms,
            status = ?output.status.code(),
            stderr = %stderr,
            "git clone failed"
        );
        return Err(Error::CloneFailed(format!(
            "git clone {repo_url} -> {}: exit {:?}: {}",
            dest.display(),
            output.status.code(),
            stderr.trim()
        )));
    }

    tracing::info!(
        repo_url = %repo_url,
        dest = %dest.display(),
        elapsed_ms,
        idempotent_skip = false,
        "clone_repo: complete"
    );

    Ok(ProjectPath(dest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Ensures a pre-existing `.git` directory short-circuits the clone
    /// without invoking git (`repo_url = "nonexistent"` would fail if we
    /// actually tried to clone).
    #[tokio::test]
    async fn clone_is_idempotent_when_git_dir_exists() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("proj");
        tokio::fs::create_dir_all(dest.join(".git")).await.unwrap();

        let path = clone_repo(
            "file:///nonexistent-should-not-be-invoked",
            &dest,
            CloneOpts::default(),
        )
        .await
        .expect("idempotent path");

        assert_eq!(path.as_path(), dest.as_path());
    }

    #[tokio::test]
    async fn clone_from_local_bare_repo_succeeds() {
        // Set up a bare repo with one commit so `git clone` has something to pull.
        let ws = tempdir().unwrap();
        let bare = ws.path().join("src.git");
        run_ok(&["git", "init", "--bare", bare.to_str().unwrap()]).await;

        // Seed the bare repo via a throwaway work tree.
        let seed = ws.path().join("seed");
        run_ok(&["git", "init", seed.to_str().unwrap()]).await;
        tokio::fs::write(seed.join("README.md"), "hello\n").await.unwrap();
        run_in(&seed, &["git", "add", "."]).await;
        run_in(&seed, &["git", "-c", "user.email=a@b", "-c", "user.name=t", "commit", "-m", "init"]).await;
        run_in(&seed, &["git", "remote", "add", "origin", bare.to_str().unwrap()]).await;
        run_in(&seed, &["git", "push", "origin", "HEAD:refs/heads/main"]).await;

        let dest = ws.path().join("dst");
        let path = clone_repo(bare.to_str().unwrap(), &dest, CloneOpts::default())
            .await
            .expect("clone ok");

        assert!(path.as_path().join(".git").exists());
        assert!(path.as_path().join("README.md").exists());
    }

    async fn run_ok(argv: &[&str]) {
        let status = Command::new(argv[0])
            .args(&argv[1..])
            .status()
            .await
            .unwrap();
        assert!(status.success(), "{argv:?}");
    }

    async fn run_in(cwd: &Path, argv: &[&str]) {
        let status = Command::new(argv[0])
            .args(&argv[1..])
            .current_dir(cwd)
            .status()
            .await
            .unwrap();
        assert!(status.success(), "{argv:?}");
    }
}
