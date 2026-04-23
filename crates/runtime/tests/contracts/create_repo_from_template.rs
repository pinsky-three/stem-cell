// @generated-stub — safe to edit; will not be overwritten if this marker is removed
use stem_cell::system_api::*;

#[test]
fn create_repo_from_template_input_roundtrips_json() {
    let input = CreateRepoFromTemplateInput {
        project_id: uuid::Uuid::new_v4(),
        github_installation_id: uuid::Uuid::new_v4(),
        template_owner: "test".to_string(),
        template_repo: "test".to_string(),
        new_owner: "test".to_string(),
        new_name: "test".to_string(),
        description: Some("test".to_string()),
        private: Some(true),
        include_all_branches: Some(true),
    };
    let json = serde_json::to_string(&input).unwrap();
    let decoded: CreateRepoFromTemplateInput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn create_repo_from_template_output_roundtrips_json() {
    let output = CreateRepoFromTemplateOutput {
        owner: "test".to_string(),
        repo: "test".to_string(),
        default_branch: "test".to_string(),
        html_url: "test".to_string(),
        repo_connection_id: uuid::Uuid::new_v4(),
        status: "test".to_string(),
    };
    let json = serde_json::to_string(&output).unwrap();
    let decoded: CreateRepoFromTemplateOutput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn create_repo_from_template_internal_error_converts() {
    let e = CreateRepoFromTemplateError::Internal("oops".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("internal"), "expected 'internal' in '{msg}'");
}

#[test]
fn error_project_not_found_converts_to_system_error() {
    let e = CreateRepoFromTemplateError::ProjectNotFound;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("ProjectNotFound"), "expected 'ProjectNotFound' in '{msg}'");
}

#[test]
fn error_installation_not_found_converts_to_system_error() {
    let e = CreateRepoFromTemplateError::InstallationNotFound;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("InstallationNotFound"), "expected 'InstallationNotFound' in '{msg}'");
}

#[test]
fn error_installation_inactive_converts_to_system_error() {
    let e = CreateRepoFromTemplateError::InstallationInactive;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("InstallationInactive"), "expected 'InstallationInactive' in '{msg}'");
}

#[test]
fn error_github_api_error_converts_to_system_error() {
    let e = CreateRepoFromTemplateError::GithubApiError("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("GithubApiError"), "expected 'GithubApiError' in '{msg}'");
}

#[test]
fn error_database_error_converts_to_system_error() {
    let e = CreateRepoFromTemplateError::DatabaseError("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("DatabaseError"), "expected 'DatabaseError' in '{msg}'");
}
