use crate::error::{Error, Result};
use crate::workspace::SandboxRoot;
use std::path::Path;
use std::time::Duration;

const KILL_GRACE: Duration = Duration::from_secs(5);

pub async fn kill_process_tree(pid: i32) {
    #[cfg(unix)]
    {
        use tokio::process::Command;

        let pgid_kill = Command::new("kill")
            .args(["-TERM", &format!("-{pid}")])
            .output()
            .await;
        let direct_kill = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output()
            .await;

        if pgid_kill.is_err() && direct_kill.is_err() {
            tracing::debug!(pid, "SIGTERM failed; process may already be gone");
            return;
        }

        tokio::time::sleep(KILL_GRACE).await;

        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .output()
            .await;
        let _ = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .output()
            .await;
    }

    #[cfg(not(unix))]
    {
        tracing::warn!(pid, "process kill not supported on this platform");
    }
}

pub async fn remove_sandbox_dir(root: &SandboxRoot, path: &Path) -> Result<()> {
    root.ensure_safe_child(path)?;
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => {
            tracing::info!(path = %path.display(), "removed sandbox directory");
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::Fs {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{SandboxId, SandboxRoot};

    #[tokio::test]
    async fn remove_rejects_path_outside_root() {
        let root = SandboxRoot::new("/tmp/stem-test-root");
        let err = remove_sandbox_dir(&root, Path::new("/tmp/other")).await;
        assert!(matches!(err, Err(Error::UnsafePath { .. })));
    }

    #[tokio::test]
    async fn remove_accepts_generated_child() {
        let tmp = tempfile::tempdir().unwrap();
        let root = SandboxRoot::new(tmp.path());
        let id = SandboxId::new("abc").unwrap();
        let dir = root.work_dir(&id);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        remove_sandbox_dir(&root, &dir).await.unwrap();
        assert!(!dir.exists());
    }
}
