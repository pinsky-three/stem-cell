pub mod run_build;
pub mod cleanup_deployments;
mod spawn_environment;

// ── GitHub lifecycle (contract mode) ─────────────────────────────
mod github_common;
mod connect_github_installation;
mod refresh_github_installation_state;
mod connect_repo_to_project;
mod start_experiment_branch;
mod push_project_changes_to_repo;
mod open_experiment_pull_request;
mod create_repo_from_template;

/// Concrete implementation of all contract-mode system traits.
/// Each sub-module implements one trait on this struct.
#[derive(Clone)]
pub struct AppSystems;
