//! Template bootstrap — pulls a seed repository and writes an initial
//! manifest.
//!
//! Phase 1 only supports the `clone-from-git-URL` strategy because
//! that's what the runtime already uses (`DEFAULT_REPO_URL` in
//! `spawn_environment.rs` points at `stem-cell-shrank`). Built-in
//! bundles, template registries, and local fixtures can land later
//! without breaking this signature.

use crate::clone::{CloneOpts, clone_repo};
use crate::error::Result;
use crate::manifest::ProjectManifest;
use crate::{ProjectPath, error::Error};
use std::path::Path;

/// Canonical template URL used by the runtime. Kept in sync with
/// `DEFAULT_REPO_URL` in `crates/runtime/src/systems/spawn_environment.rs`.
pub const DEFAULT_TEMPLATE_URL: &str = "https://github.com/pinsky-three/stem-cell-shrank";

pub struct TemplateOutcome {
    pub path: ProjectPath,
    pub manifest: ProjectManifest,
}

/// Scaffolds a new project at `dest` by cloning `template_url` (or
/// `DEFAULT_TEMPLATE_URL` when `None`) and writing a `stem.yaml`
/// manifest recording where it came from.
pub async fn init_from_template(
    name: &str,
    dest: &Path,
    template_url: Option<&str>,
) -> Result<TemplateOutcome> {
    if dest.exists() && !is_empty_dir(dest).await? {
        return Err(Error::InvalidPath {
            path: dest.display().to_string(),
            reason: "destination exists and is not empty".into(),
        });
    }

    let url = template_url.unwrap_or(DEFAULT_TEMPLATE_URL);
    let path = clone_repo(
        url,
        dest,
        CloneOpts {
            progress: true,
            ..Default::default()
        },
    )
    .await?;

    let manifest = ProjectManifest {
        name: name.to_string(),
        template: Some(url.to_string()),
        stem_version: Some(env!("CARGO_PKG_VERSION").to_string()),
    };
    manifest.save(path.as_path()).await?;

    Ok(TemplateOutcome { path, manifest })
}

async fn is_empty_dir(p: &Path) -> Result<bool> {
    if !p.is_dir() {
        return Ok(false);
    }
    let mut entries = tokio::fs::read_dir(p).await?;
    Ok(entries.next_entry().await?.is_none())
}
