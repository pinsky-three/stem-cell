// @generated-stub — safe to edit; will not be overwritten if this marker is removed
use stem_cell::system_api::*;

#[test]
fn spawn_environment_input_roundtrips_json() {
    let input = SpawnEnvironmentInput {
        org_id: uuid::Uuid::new_v4(),
        user_id: uuid::Uuid::new_v4(),
        prompt: "test".to_string(),
        project_id: Some(uuid::Uuid::new_v4()),
    };
    let json = serde_json::to_string(&input).unwrap();
    let decoded: SpawnEnvironmentInput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn spawn_environment_output_roundtrips_json() {
    let output = SpawnEnvironmentOutput {
        project_id: "test".to_string(),
        job_id: "test".to_string(),
        status: "test".to_string(),
    };
    let json = serde_json::to_string(&output).unwrap();
    let decoded: SpawnEnvironmentOutput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn spawn_environment_internal_error_converts() {
    let e = SpawnEnvironmentError::Internal("oops".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("internal"), "expected 'internal' in '{msg}'");
}

#[test]
fn error_org_not_found_converts_to_system_error() {
    let e = SpawnEnvironmentError::OrgNotFound;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("OrgNotFound"), "expected 'OrgNotFound' in '{msg}'");
}

#[test]
fn error_user_not_found_converts_to_system_error() {
    let e = SpawnEnvironmentError::UserNotFound;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("UserNotFound"), "expected 'UserNotFound' in '{msg}'");
}

#[test]
fn error_spawn_failed_converts_to_system_error() {
    let e = SpawnEnvironmentError::SpawnFailed("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("SpawnFailed"), "expected 'SpawnFailed' in '{msg}'");
}

#[test]
fn error_database_error_converts_to_system_error() {
    let e = SpawnEnvironmentError::DatabaseError("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("DatabaseError"), "expected 'DatabaseError' in '{msg}'");
}
