pub mod cleanup_deployments;
pub mod run_build;
mod spawn_environment;

// ── GitHub lifecycle (contract mode) ─────────────────────────────
mod connect_github_installation;
mod connect_repo_to_project;
mod create_repo_from_template;
mod github_common;
mod open_experiment_pull_request;
mod push_project_changes_to_repo;
mod refresh_github_installation_state;
mod start_experiment_branch;

/// Concrete implementation of all contract-mode system traits.
/// Each sub-module implements one trait on this struct.
#[derive(Clone)]
pub struct AppSystems;
