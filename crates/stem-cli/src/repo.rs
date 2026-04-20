//! Repository discovery helpers.
//!
//! The CLI can be invoked from any subdirectory of the stem-cell repo. We
//! walk upward until we find a `.git` directory so prompts are always rooted
//! at the workspace root.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// UUIDv5 namespace for stable per-repo IDs.
/// Arbitrary constant — treat as a magic cookie, never changes.
const NS_STEM_CELL: Uuid = Uuid::from_bytes([
    0x5c, 0xe1, 0x1c, 0xe1, 0x5c, 0xe1, 0x1c, 0xe1, 0x5c, 0xe1, 0x1c, 0xe1, 0x5c, 0xe1, 0x1c, 0xe1,
]);

pub struct RepoInfo {
    pub root: PathBuf,
    pub project_id: Uuid,
}

pub fn discover() -> Result<RepoInfo> {
    let cwd = std::env::current_dir().context("resolving current dir")?;
    let root = find_root(&cwd)?;
    let canonical = std::fs::canonicalize(&root).unwrap_or(root.clone());
    let project_id = Uuid::new_v5(&NS_STEM_CELL, canonical.to_string_lossy().as_bytes());
    Ok(RepoInfo {
        root: canonical,
        project_id,
    })
}

fn find_root(start: &Path) -> Result<PathBuf> {
    for ancestor in start.ancestors() {
        if ancestor.join(".git").exists() {
            return Ok(ancestor.to_path_buf());
        }
    }
    bail!(
        "could not find repository root (no .git found walking up from {})",
        start.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_id_is_stable_for_same_path() {
        let a = Uuid::new_v5(&NS_STEM_CELL, b"/tmp/example");
        let b = Uuid::new_v5(&NS_STEM_CELL, b"/tmp/example");
        assert_eq!(a, b);
    }

    #[test]
    fn project_id_differs_across_paths() {
        let a = Uuid::new_v5(&NS_STEM_CELL, b"/tmp/a");
        let b = Uuid::new_v5(&NS_STEM_CELL, b"/tmp/b");
        assert_ne!(a, b);
    }
}
