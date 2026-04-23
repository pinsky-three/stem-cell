// @generated-stub — safe to edit; will not be overwritten if this marker is removed
use stem_cell::system_api::*;

#[test]
fn open_experiment_pull_request_input_roundtrips_json() {
    let input = OpenExperimentPullRequestInput {
        project_id: uuid::Uuid::new_v4(),
        branch_name: "test".to_string(),
        title: "test".to_string(),
        body: Some("test".to_string()),
    };
    let json = serde_json::to_string(&input).unwrap();
    let decoded: OpenExperimentPullRequestInput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn open_experiment_pull_request_output_roundtrips_json() {
    let output = OpenExperimentPullRequestOutput {
        pr_number: 1,
        pr_url: "test".to_string(),
        status: "test".to_string(),
    };
    let json = serde_json::to_string(&output).unwrap();
    let decoded: OpenExperimentPullRequestOutput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn open_experiment_pull_request_internal_error_converts() {
    let e = OpenExperimentPullRequestError::Internal("oops".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("internal"), "expected 'internal' in '{msg}'");
}

#[test]
fn error_project_not_found_converts_to_system_error() {
    let e = OpenExperimentPullRequestError::ProjectNotFound;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("ProjectNotFound"), "expected 'ProjectNotFound' in '{msg}'");
}

#[test]
fn error_repo_not_connected_converts_to_system_error() {
    let e = OpenExperimentPullRequestError::RepoNotConnected;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("RepoNotConnected"), "expected 'RepoNotConnected' in '{msg}'");
}

#[test]
fn error_installation_inactive_converts_to_system_error() {
    let e = OpenExperimentPullRequestError::InstallationInactive;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("InstallationInactive"), "expected 'InstallationInactive' in '{msg}'");
}

#[test]
fn error_pull_request_failed_converts_to_system_error() {
    let e = OpenExperimentPullRequestError::PullRequestFailed("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("PullRequestFailed"), "expected 'PullRequestFailed' in '{msg}'");
}

#[test]
fn error_github_api_error_converts_to_system_error() {
    let e = OpenExperimentPullRequestError::GithubApiError("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("GithubApiError"), "expected 'GithubApiError' in '{msg}'");
}
