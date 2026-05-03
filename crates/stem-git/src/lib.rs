//! Shared Git and GitHub App primitives for Stem Cell.
//!
//! This crate deliberately avoids database and web-framework dependencies. It
//! is safe to use from the runtime, CLI, and lower-level project materializers.

pub mod error;
pub mod git;
pub mod github;
pub mod redaction;

pub use error::{Error, Result};
pub use git::{
    CloneOptions, CommitOptions, GitCommandOutput, GitRunOptions, GitRunner, PushOptions,
    clone_repository, run_git, run_git_output,
};
pub use redaction::Redactor;
