// @generated-stub — safe to edit; will not be overwritten if this marker is removed
use stem_cell::system_api::*;

#[test]
fn connect_github_installation_input_roundtrips_json() {
    let input = ConnectGithubInstallationInput {
        org_id: uuid::Uuid::new_v4(),
        installer_user_id: Some(uuid::Uuid::new_v4()),
        installation_id: 100,
        account_login: "test".to_string(),
        target_type: "test".to_string(),
        permissions: "test".to_string(),
        status: Some("test".to_string()),
    };
    let json = serde_json::to_string(&input).unwrap();
    let decoded: ConnectGithubInstallationInput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn connect_github_installation_output_roundtrips_json() {
    let output = ConnectGithubInstallationOutput {
        github_installation_id: uuid::Uuid::new_v4(),
        status: "test".to_string(),
        active: true,
    };
    let json = serde_json::to_string(&output).unwrap();
    let decoded: ConnectGithubInstallationOutput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn connect_github_installation_internal_error_converts() {
    let e = ConnectGithubInstallationError::Internal("oops".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("internal"), "expected 'internal' in '{msg}'");
}

#[test]
fn error_org_not_found_converts_to_system_error() {
    let e = ConnectGithubInstallationError::OrgNotFound;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("OrgNotFound"), "expected 'OrgNotFound' in '{msg}'");
}

#[test]
fn error_invalid_target_type_converts_to_system_error() {
    let e = ConnectGithubInstallationError::InvalidTargetType;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("InvalidTargetType"), "expected 'InvalidTargetType' in '{msg}'");
}

#[test]
fn error_installation_unverified_converts_to_system_error() {
    let e = ConnectGithubInstallationError::InstallationUnverified("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("InstallationUnverified"), "expected 'InstallationUnverified' in '{msg}'");
}

#[test]
fn error_database_error_converts_to_system_error() {
    let e = ConnectGithubInstallationError::DatabaseError("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("DatabaseError"), "expected 'DatabaseError' in '{msg}'");
}
