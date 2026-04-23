// @generated-stub — safe to edit; will not be overwritten if this marker is removed
use stem_cell::system_api::*;

#[test]
fn connect_repo_to_project_input_roundtrips_json() {
    let input = ConnectRepoToProjectInput {
        project_id: uuid::Uuid::new_v4(),
        github_installation_id: uuid::Uuid::new_v4(),
        owner: "test".to_string(),
        repo: "test".to_string(),
        default_branch: Some("test".to_string()),
    };
    let json = serde_json::to_string(&input).unwrap();
    let decoded: ConnectRepoToProjectInput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn connect_repo_to_project_output_roundtrips_json() {
    let output = ConnectRepoToProjectOutput {
        repo_connection_id: uuid::Uuid::new_v4(),
        default_branch: "test".to_string(),
        status: "test".to_string(),
    };
    let json = serde_json::to_string(&output).unwrap();
    let decoded: ConnectRepoToProjectOutput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn connect_repo_to_project_internal_error_converts() {
    let e = ConnectRepoToProjectError::Internal("oops".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("internal"), "expected 'internal' in '{msg}'");
}

#[test]
fn error_project_not_found_converts_to_system_error() {
    let e = ConnectRepoToProjectError::ProjectNotFound;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("ProjectNotFound"), "expected 'ProjectNotFound' in '{msg}'");
}

#[test]
fn error_installation_not_found_converts_to_system_error() {
    let e = ConnectRepoToProjectError::InstallationNotFound;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("InstallationNotFound"), "expected 'InstallationNotFound' in '{msg}'");
}

#[test]
fn error_installation_inactive_converts_to_system_error() {
    let e = ConnectRepoToProjectError::InstallationInactive;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("InstallationInactive"), "expected 'InstallationInactive' in '{msg}'");
}

#[test]
fn error_repo_not_accessible_converts_to_system_error() {
    let e = ConnectRepoToProjectError::RepoNotAccessible("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("RepoNotAccessible"), "expected 'RepoNotAccessible' in '{msg}'");
}

#[test]
fn error_database_error_converts_to_system_error() {
    let e = ConnectRepoToProjectError::DatabaseError("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("DatabaseError"), "expected 'DatabaseError' in '{msg}'");
}
