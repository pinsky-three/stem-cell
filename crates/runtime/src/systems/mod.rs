pub mod run_build;
pub mod cleanup_deployments;
mod spawn_environment;

/// Concrete implementation of all contract-mode system traits.
/// Each sub-module implements one trait on this struct.
#[derive(Clone)]
pub struct AppSystems;
