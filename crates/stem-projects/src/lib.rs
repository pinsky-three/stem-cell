//! Pure-logic filesystem primitives shared by the stem-cell runtime and the
//! `stem` CLI.
//!
//! This crate deliberately knows nothing about:
//! - Databases (no `sqlx`)
//! - HTTP servers (no `axum`)
//! - OpenCode process management (no `opencode-client`)
//!
//! It only knows how to put a project on disk (clone a template or repo)
//! and how to run the toolchain-install bootstrap against it. Both the
//! runtime's `SpawnEnvironment` system and the CLI's `stem init` /
//! `stem clone` commands depend on it so there is exactly one
//! implementation of "how do we materialize a stem-cell project".

pub mod clone;
pub mod error;
pub mod install;
pub mod manifest;
pub mod patch;
pub mod template;

pub use clone::{CloneOpts, clone_repo};
pub use error::{Error, Result};
pub use install::{InstallOpts, install_toolchain};
pub use manifest::ProjectManifest;
pub use patch::astro_port_patch_snippet;
pub use template::{DEFAULT_TEMPLATE_URL, init_from_template};

/// Typed wrapper around a project checkout on disk. Returned by
/// `clone_repo` so callers can't confuse a project path with an arbitrary
/// `PathBuf` (install/patch operations only make sense against a real
/// project root).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPath(pub std::path::PathBuf);

impl ProjectPath {
    pub fn as_path(&self) -> &std::path::Path {
        &self.0
    }

    pub fn into_inner(self) -> std::path::PathBuf {
        self.0
    }
}

impl AsRef<std::path::Path> for ProjectPath {
    fn as_ref(&self) -> &std::path::Path {
        &self.0
    }
}
