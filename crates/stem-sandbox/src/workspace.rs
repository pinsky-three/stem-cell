use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SandboxId(String);

impl SandboxId {
    pub fn new(raw: impl Into<String>) -> Result<Self> {
        let raw = raw.into();
        if raw.is_empty()
            || !raw
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            return Err(Error::InvalidSandboxId(raw));
        }
        Ok(Self(raw))
    }

    pub fn from_uuid(id: uuid::Uuid) -> Self {
        Self(id.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SandboxId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxRoot {
    root: PathBuf,
}

impl SandboxRoot {
    pub fn temp_default() -> Self {
        Self {
            root: std::env::temp_dir(),
        }
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn work_dir(&self, id: &SandboxId) -> PathBuf {
        self.root.join(format!("stem-cell-{}", id.as_str()))
    }

    pub fn ensure_safe_child(&self, path: &Path) -> Result<()> {
        if path.starts_with(&self.root) && path.file_name().is_some() {
            Ok(())
        } else {
            Err(Error::UnsafePath {
                path: path.to_path_buf(),
            })
        }
    }
}

#[derive(Debug, Clone)]
pub struct SandboxSpec {
    pub id: SandboxId,
    pub root: SandboxRoot,
    pub port: u16,
}

impl SandboxSpec {
    pub fn new(id: SandboxId, port: u16) -> Self {
        Self {
            id,
            root: SandboxRoot::temp_default(),
            port,
        }
    }

    pub fn with_root(mut self, root: SandboxRoot) -> Self {
        self.root = root;
        self
    }

    pub fn work_dir(&self) -> PathBuf {
        self.root.work_dir(&self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_id_rejects_path_punctuation() {
        assert!(SandboxId::new("../bad").is_err());
        assert!(SandboxId::new("ok-123_abc").is_ok());
    }

    #[test]
    fn work_dir_uses_stem_cell_prefix() {
        let id = SandboxId::new("abc").unwrap();
        let root = SandboxRoot::new("/tmp");
        assert_eq!(root.work_dir(&id), PathBuf::from("/tmp/stem-cell-abc"));
    }
}
