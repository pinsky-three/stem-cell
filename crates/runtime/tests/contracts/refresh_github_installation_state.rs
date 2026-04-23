// @generated-stub — safe to edit; will not be overwritten if this marker is removed
use stem_cell::system_api::*;

#[test]
fn refresh_github_installation_state_input_roundtrips_json() {
    let input = RefreshGithubInstallationStateInput {
        github_installation_id: uuid::Uuid::new_v4(),
    };
    let json = serde_json::to_string(&input).unwrap();
    let decoded: RefreshGithubInstallationStateInput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn refresh_github_installation_state_output_roundtrips_json() {
    let output = RefreshGithubInstallationStateOutput {
        status: "test".to_string(),
        active: true,
        permissions: "test".to_string(),
    };
    let json = serde_json::to_string(&output).unwrap();
    let decoded: RefreshGithubInstallationStateOutput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn refresh_github_installation_state_internal_error_converts() {
    let e = RefreshGithubInstallationStateError::Internal("oops".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("internal"), "expected 'internal' in '{msg}'");
}

#[test]
fn error_installation_not_found_converts_to_system_error() {
    let e = RefreshGithubInstallationStateError::InstallationNotFound;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("InstallationNotFound"), "expected 'InstallationNotFound' in '{msg}'");
}

#[test]
fn error_github_api_error_converts_to_system_error() {
    let e = RefreshGithubInstallationStateError::GithubApiError("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("GithubApiError"), "expected 'GithubApiError' in '{msg}'");
}

#[test]
fn error_database_error_converts_to_system_error() {
    let e = RefreshGithubInstallationStateError::DatabaseError("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("DatabaseError"), "expected 'DatabaseError' in '{msg}'");
}
