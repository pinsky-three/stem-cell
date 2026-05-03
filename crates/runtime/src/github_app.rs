//! Runtime compatibility facade for shared GitHub App primitives.
//!
//! The implementation lives in `stem-git` so the web runtime and CLI can use
//! the same JWT, installation-token, webhook, and Git remote handling.

pub use stem_git::github::*;
