// @generated-stub — safe to edit; will not be overwritten if this marker is removed
use stem_cell::system_api::*;

#[test]
fn start_experiment_branch_input_roundtrips_json() {
    let input = StartExperimentBranchInput {
        project_id: uuid::Uuid::new_v4(),
        base_sha: Some("test".to_string()),
    };
    let json = serde_json::to_string(&input).unwrap();
    let decoded: StartExperimentBranchInput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn start_experiment_branch_output_roundtrips_json() {
    let output = StartExperimentBranchOutput {
        branch_name: "test".to_string(),
        base_branch: "test".to_string(),
        base_sha: "test".to_string(),
        status: "test".to_string(),
    };
    let json = serde_json::to_string(&output).unwrap();
    let decoded: StartExperimentBranchOutput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn start_experiment_branch_internal_error_converts() {
    let e = StartExperimentBranchError::Internal("oops".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("internal"), "expected 'internal' in '{msg}'");
}

#[test]
fn error_project_not_found_converts_to_system_error() {
    let e = StartExperimentBranchError::ProjectNotFound;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("ProjectNotFound"), "expected 'ProjectNotFound' in '{msg}'");
}

#[test]
fn error_repo_not_connected_converts_to_system_error() {
    let e = StartExperimentBranchError::RepoNotConnected;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("RepoNotConnected"), "expected 'RepoNotConnected' in '{msg}'");
}

#[test]
fn error_installation_inactive_converts_to_system_error() {
    let e = StartExperimentBranchError::InstallationInactive;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("InstallationInactive"), "expected 'InstallationInactive' in '{msg}'");
}

#[test]
fn error_github_api_error_converts_to_system_error() {
    let e = StartExperimentBranchError::GithubApiError("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("GithubApiError"), "expected 'GithubApiError' in '{msg}'");
}
