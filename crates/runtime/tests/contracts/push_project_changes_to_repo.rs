// @generated-stub — safe to edit; will not be overwritten if this marker is removed
use stem_cell::system_api::*;

#[test]
fn push_project_changes_to_repo_input_roundtrips_json() {
    let input = PushProjectChangesToRepoInput {
        project_id: uuid::Uuid::new_v4(),
        branch_name: "test".to_string(),
        commit_message: "test".to_string(),
    };
    let json = serde_json::to_string(&input).unwrap();
    let decoded: PushProjectChangesToRepoInput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn push_project_changes_to_repo_output_roundtrips_json() {
    let output = PushProjectChangesToRepoOutput {
        commit_sha: "test".to_string(),
        status: "test".to_string(),
    };
    let json = serde_json::to_string(&output).unwrap();
    let decoded: PushProjectChangesToRepoOutput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn push_project_changes_to_repo_internal_error_converts() {
    let e = PushProjectChangesToRepoError::Internal("oops".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("internal"), "expected 'internal' in '{msg}'");
}

#[test]
fn error_project_not_found_converts_to_system_error() {
    let e = PushProjectChangesToRepoError::ProjectNotFound;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("ProjectNotFound"), "expected 'ProjectNotFound' in '{msg}'");
}

#[test]
fn error_repo_not_connected_converts_to_system_error() {
    let e = PushProjectChangesToRepoError::RepoNotConnected;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("RepoNotConnected"), "expected 'RepoNotConnected' in '{msg}'");
}

#[test]
fn error_installation_inactive_converts_to_system_error() {
    let e = PushProjectChangesToRepoError::InstallationInactive;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("InstallationInactive"), "expected 'InstallationInactive' in '{msg}'");
}

#[test]
fn error_nothing_to_push_converts_to_system_error() {
    let e = PushProjectChangesToRepoError::NothingToPush;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("NothingToPush"), "expected 'NothingToPush' in '{msg}'");
}

#[test]
fn error_push_failed_converts_to_system_error() {
    let e = PushProjectChangesToRepoError::PushFailed("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("PushFailed"), "expected 'PushFailed' in '{msg}'");
}

#[test]
fn error_github_api_error_converts_to_system_error() {
    let e = PushProjectChangesToRepoError::GithubApiError("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("GithubApiError"), "expected 'GithubApiError' in '{msg}'");
}
