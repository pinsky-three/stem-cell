//! Sandbox workspace and process primitives shared by runtime and CLI.
//!
//! The crate owns deterministic paths, command construction, health probes,
//! and cleanup safety. It intentionally avoids databases, HTTP servers, and
//! application-specific deployment records.

pub mod cleanup;
pub mod command;
pub mod error;
pub mod health;
pub mod workspace;

pub use cleanup::{kill_process_tree, remove_sandbox_dir};
pub use command::{ContainerNetwork, ContainerRunSpec, ProcessRunSpec};
pub use error::{Error, Result};
pub use health::{HealthStatus, wait_for_http_ok, wait_until_port_released};
pub use workspace::{SandboxId, SandboxRoot, SandboxSpec};
