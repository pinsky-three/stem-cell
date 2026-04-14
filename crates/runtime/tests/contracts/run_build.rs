use stem_cell::system_api::*;

#[test]
fn run_build_input_roundtrips_json() {
    let input = RunBuildInput {
        build_job_id: uuid::Uuid::new_v4(),
    };
    let json = serde_json::to_string(&input).unwrap();
    let decoded: RunBuildInput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn run_build_output_roundtrips_json() {
    let output = RunBuildOutput {
        artifacts_count: 1,
        tokens_used: 100,
        status: "test".to_string(),
    };
    let json = serde_json::to_string(&output).unwrap();
    let decoded: RunBuildOutput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn run_build_internal_error_converts() {
    let e = RunBuildError::Internal("oops".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("internal"), "expected 'internal' in '{msg}'");
}

#[test]
fn error_build_job_not_found_converts_to_system_error() {
    let e = RunBuildError::BuildJobNotFound;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("BuildJobNotFound"), "expected 'BuildJobNotFound' in '{msg}'");
}

#[test]
fn error_project_not_found_converts_to_system_error() {
    let e = RunBuildError::ProjectNotFound;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("ProjectNotFound"), "expected 'ProjectNotFound' in '{msg}'");
}

#[test]
fn error_ai_provider_error_converts_to_system_error() {
    let e = RunBuildError::AiProviderError("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("AiProviderError"), "expected 'AiProviderError' in '{msg}'");
}

#[test]
fn error_build_failed_converts_to_system_error() {
    let e = RunBuildError::BuildFailed("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("BuildFailed"), "expected 'BuildFailed' in '{msg}'");
}

// ── OpenCode types tests ────────────────────────────────────────

#[test]
fn opencode_event_parse_message_part_updated() {
    let data = r#"{"content":{"content":"hello"}}"#;
    let event = opencode_client::OpenCodeEvent::parse("message.part.updated", data);
    match event {
        opencode_client::OpenCodeEvent::MessagePartUpdated { properties } => {
            assert!(properties.get("content").is_some());
        }
        _ => panic!("expected MessagePartUpdated"),
    }
}

#[test]
fn opencode_event_parse_message_completed_is_terminal() {
    let event = opencode_client::OpenCodeEvent::parse("message.completed", "{}");
    assert!(event.is_terminal());
}

#[test]
fn opencode_event_parse_unknown_fallback() {
    let event = opencode_client::OpenCodeEvent::parse("tool.execute", r#"{"tool":"edit"}"#);
    match event {
        opencode_client::OpenCodeEvent::Unknown { raw_type, data } => {
            assert_eq!(raw_type, "tool.execute");
            assert_eq!(data.get("tool").unwrap().as_str().unwrap(), "edit");
        }
        _ => panic!("expected Unknown"),
    }
}

#[test]
fn opencode_event_server_connected() {
    let event = opencode_client::OpenCodeEvent::parse("server.connected", "");
    assert!(matches!(event, opencode_client::OpenCodeEvent::ServerConnected));
}

#[test]
fn build_event_serialization() {
    let event = opencode_client::BuildEvent::BuildComplete {
        job_id: "abc-123".into(),
        status: "succeeded".into(),
        artifacts_count: 5,
        tokens_used: 1200,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("build_complete"));
    assert!(json.contains("abc-123"));

    let chunk = opencode_client::BuildEvent::MessageChunk {
        job_id: "def-456".into(),
        text: "hello world".into(),
    };
    let json = serde_json::to_string(&chunk).unwrap();
    assert!(json.contains("message_chunk"));
    assert!(json.contains("hello world"));
}

#[test]
fn process_manager_config_defaults() {
    let config = opencode_client::ProcessManagerConfig::default();
    assert_eq!(config.port_base, 14000);
    assert_eq!(config.port_range, 200);
    assert!(config.server_password.is_none());
    assert!(config.default_model.is_none());
}

#[test]
fn opencode_client_new_builds_without_auth() {
    let client = opencode_client::OpenCodeClient::new(14099, None);
    assert!(client.is_ok());
    let client = client.unwrap();
    assert!(client.base_url().contains("14099"));
}

#[test]
fn opencode_client_new_builds_with_auth() {
    let client = opencode_client::OpenCodeClient::new(14100, Some("secret"));
    assert!(client.is_ok());
}

#[test]
fn event_bus_is_singleton() {
    let bus1 = stem_cell::systems::run_build::event_bus();
    let bus2 = stem_cell::systems::run_build::event_bus();
    assert!(std::sync::Arc::ptr_eq(&bus1, &bus2));
}
