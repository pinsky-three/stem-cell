use serde::{Deserialize, Deserializer};

// ── Top-level spec ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemsSpec {
    pub version: u32,
    #[serde(default)]
    pub integrations: Vec<IntegrationSpec>,
    pub systems: Vec<SystemDef>,
}

// ── Integrations ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationSpec {
    pub name: String,
    pub operations: Vec<OperationSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationSpec {
    pub name: String,
    pub input: Vec<ParamSpec>,
    pub output: Vec<ParamSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

// ── System mode ──────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemMode {
    #[default]
    Generated,
    Contract,
}

// ── Systems ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemDef {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub mode: SystemMode,
    pub input: Vec<InputField>,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default)]
    pub result: Vec<ResultField>,
    #[serde(default)]
    pub output: Vec<OutputField>,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputField {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub required: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultField {
    pub name: String,
    pub from: String,
}

/// Typed output field for contract-mode systems (name + type, no binding).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputField {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

// ── Steps (internally-tagged enum) ──────────────────────────────────────
// NOTE: deny_unknown_fields is intentionally omitted from variant structs
// because serde does not strip the `kind` tag before variant deserialization.

#[derive(Debug, Deserialize)]
#[serde(tag = "kind")]
pub enum Step {
    #[serde(rename = "load_one")]
    LoadOne(LoadOneStep),
    #[serde(rename = "load_many")]
    LoadMany(LoadManyStep),
    #[serde(rename = "create")]
    Create(CreateStep),
    #[serde(rename = "update")]
    Update(UpdateStep),
    #[serde(rename = "delete")]
    Delete(DeleteStep),
    #[serde(rename = "guard")]
    Guard(GuardStep),
    #[serde(rename = "branch")]
    Branch(BranchStep),
    #[serde(rename = "call_integration")]
    CallIntegration(CallIntegrationStep),
    #[serde(rename = "emit_event")]
    EmitEvent(EmitEventStep),
}

#[derive(Debug, Deserialize)]
pub struct LoadOneStep {
    pub entity: String,
    pub by: String,
    #[serde(rename = "as")]
    pub binding: String,
    pub not_found: String,
}

#[derive(Debug, Deserialize)]
pub struct LoadManyStep {
    pub entity: String,
    pub filter: FilterDef,
    #[serde(rename = "as")]
    pub binding: String,
}

#[derive(Debug, Deserialize)]
pub struct FilterDef {
    pub field: String,
    pub from: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateStep {
    pub entity: String,
    pub set: Vec<FieldMapping>,
    #[serde(default, rename = "as")]
    pub binding: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateStep {
    pub entity: String,
    pub target: String,
    pub set: Vec<FieldMapping>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteStep {
    pub entity: String,
    pub target: String,
}

#[derive(Debug, Deserialize)]
pub struct GuardStep {
    pub check: Condition,
    pub error: String,
}

#[derive(Debug, Deserialize)]
pub struct BranchStep {
    pub check: Condition,
    pub then: Vec<Step>,
    #[serde(default, rename = "else")]
    pub otherwise: Option<Vec<Step>>,
}

#[derive(Debug, Deserialize)]
pub struct CallIntegrationStep {
    pub integration: String,
    pub operation: String,
    pub input: Vec<IntegrationInput>,
    #[serde(rename = "as")]
    pub binding: String,
}

#[derive(Debug, Deserialize)]
pub struct IntegrationInput {
    pub param: String,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub value: Option<serde_yaml::Value>,
}

#[derive(Debug, Deserialize)]
pub struct EmitEventStep {
    pub event: String,
    pub payload: Vec<EventPayloadField>,
}

#[derive(Debug, Deserialize)]
pub struct EventPayloadField {
    pub field: String,
    pub from: String,
}

// ── Shared sub-types ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct FieldMapping {
    pub field: String,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default, deserialize_with = "preserve_yaml_null")]
    pub value: Option<serde_yaml::Value>,
}

fn preserve_yaml_null<'de, D>(deserializer: D) -> Result<Option<serde_yaml::Value>, D::Error>
where
    D: Deserializer<'de>,
{
    serde_yaml::Value::deserialize(deserializer).map(Some)
}

/// Guard / branch condition.
/// Exactly one of `equals`, `not_equals`, `equals_field`, `not_equals_field`
/// must be set (enforced by validation, not serde).
#[derive(Debug, Deserialize)]
pub struct Condition {
    pub field: String,
    #[serde(default)]
    pub equals: Option<serde_yaml::Value>,
    #[serde(default)]
    pub not_equals: Option<serde_yaml::Value>,
    #[serde(default)]
    pub equals_field: Option<String>,
    #[serde(default)]
    pub not_equals_field: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> SystemsSpec {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn parse_minimal_spec() {
        let spec = parse(
            r#"
version: 1
systems: []
"#,
        );
        assert_eq!(spec.version, 1);
        assert!(spec.integrations.is_empty());
        assert!(spec.systems.is_empty());
    }

    #[test]
    fn parse_integration() {
        let spec = parse(
            r#"
version: 1
integrations:
  - name: "payment"
    operations:
      - name: "charge"
        input:
          - { name: "amount", type: "bigint" }
        output:
          - { name: "id", type: "string" }
systems: []
"#,
        );
        assert_eq!(spec.integrations.len(), 1);
        assert_eq!(spec.integrations[0].operations[0].input.len(), 1);
    }

    #[test]
    fn parse_load_one_step() {
        let spec = parse(
            r#"
version: 1
systems:
  - name: "Test"
    description: "test"
    input: []
    steps:
      - kind: "load_one"
        entity: "Invoice"
        by: "input.id"
        as: "inv"
        not_found: "not found"
    result: []
"#,
        );
        match &spec.systems[0].steps[0] {
            Step::LoadOne(s) => {
                assert_eq!(s.entity, "Invoice");
                assert_eq!(s.binding, "inv");
            }
            _ => panic!("expected LoadOne"),
        }
    }

    #[test]
    fn parse_load_many_step() {
        let spec = parse(
            r#"
version: 1
systems:
  - name: "Test"
    description: "test"
    input: []
    steps:
      - kind: "load_many"
        entity: "Task"
        filter: { field: "sprint_id", from: "sprint.id" }
        as: "tasks"
    result: []
"#,
        );
        match &spec.systems[0].steps[0] {
            Step::LoadMany(s) => {
                assert_eq!(s.filter.field, "sprint_id");
                assert_eq!(s.filter.from, "sprint.id");
            }
            _ => panic!("expected LoadMany"),
        }
    }

    #[test]
    fn parse_create_step() {
        let spec = parse(
            r#"
version: 1
systems:
  - name: "Test"
    description: "test"
    input: []
    steps:
      - kind: "create"
        entity: "Payment"
        set:
          - { field: "amount", from: "input.amount" }
          - { field: "status", value: "pending" }
        as: "payment"
    result: []
"#,
        );
        match &spec.systems[0].steps[0] {
            Step::Create(s) => {
                assert_eq!(s.set.len(), 2);
                assert!(s.set[0].from.is_some());
                assert!(s.set[1].value.is_some());
                assert_eq!(s.binding.as_deref(), Some("payment"));
            }
            _ => panic!("expected Create"),
        }
    }

    #[test]
    fn parse_guard_with_equals() {
        let spec = parse(
            r#"
version: 1
systems:
  - name: "Test"
    description: "test"
    input: []
    steps:
      - kind: "guard"
        check: { field: "inv.paid", equals: false }
        error: "already paid"
    result: []
"#,
        );
        match &spec.systems[0].steps[0] {
            Step::Guard(s) => {
                assert_eq!(s.check.field, "inv.paid");
                assert!(s.check.equals.is_some());
            }
            _ => panic!("expected Guard"),
        }
    }

    #[test]
    fn parse_guard_with_equals_field() {
        let spec = parse(
            r#"
version: 1
systems:
  - name: "Test"
    description: "test"
    input: []
    steps:
      - kind: "guard"
        check: { field: "comment.author_id", equals_field: "input.user_id" }
        error: "not author"
    result: []
"#,
        );
        match &spec.systems[0].steps[0] {
            Step::Guard(s) => {
                assert_eq!(s.check.equals_field.as_deref(), Some("input.user_id"));
            }
            _ => panic!("expected Guard"),
        }
    }

    #[test]
    fn parse_branch_with_else() {
        let spec = parse(
            r#"
version: 1
systems:
  - name: "Test"
    description: "test"
    input: []
    steps:
      - kind: "branch"
        check: { field: "input.flag", equals: true }
        then:
          - kind: "emit_event"
            event: "flagged"
            payload: []
        else:
          - kind: "emit_event"
            event: "unflagged"
            payload: []
    result: []
"#,
        );
        match &spec.systems[0].steps[0] {
            Step::Branch(s) => {
                assert_eq!(s.then.len(), 1);
                assert!(s.otherwise.is_some());
                assert_eq!(s.otherwise.as_ref().unwrap().len(), 1);
            }
            _ => panic!("expected Branch"),
        }
    }

    #[test]
    fn parse_call_integration_step() {
        let spec = parse(
            r#"
version: 1
systems:
  - name: "Test"
    description: "test"
    input: []
    steps:
      - kind: "call_integration"
        integration: "pay"
        operation: "charge"
        input:
          - { param: "amount", from: "input.amount" }
          - { param: "note", value: "hello" }
        as: "result"
    result: []
"#,
        );
        match &spec.systems[0].steps[0] {
            Step::CallIntegration(s) => {
                assert_eq!(s.integration, "pay");
                assert_eq!(s.input.len(), 2);
            }
            _ => panic!("expected CallIntegration"),
        }
    }

    #[test]
    fn parse_delete_step() {
        let spec = parse(
            r#"
version: 1
systems:
  - name: "Test"
    description: "test"
    input: []
    steps:
      - kind: "delete"
        entity: "Comment"
        target: "comment.id"
    result: []
"#,
        );
        match &spec.systems[0].steps[0] {
            Step::Delete(s) => assert_eq!(s.entity, "Comment"),
            _ => panic!("expected Delete"),
        }
    }

    #[test]
    fn parse_update_step() {
        let spec = parse(
            r#"
version: 1
systems:
  - name: "Test"
    description: "test"
    input: []
    steps:
      - kind: "update"
        entity: "Invoice"
        target: "inv.id"
        set:
          - { field: "paid", value: true }
    result: []
"#,
        );
        match &spec.systems[0].steps[0] {
            Step::Update(s) => {
                assert_eq!(s.target, "inv.id");
                assert_eq!(s.set.len(), 1);
            }
            _ => panic!("expected Update"),
        }
    }

    #[test]
    fn reject_unknown_top_level_key() {
        let result: Result<SystemsSpec, _> = serde_yaml::from_str(
            r#"
version: 1
systems: []
extra: "bad"
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn parse_empty_result() {
        let spec = parse(
            r#"
version: 1
systems:
  - name: "Noop"
    description: "does nothing"
    input: []
    steps: []
    result: []
"#,
        );
        assert!(spec.systems[0].result.is_empty());
    }

    #[test]
    fn parse_contract_mode() {
        let spec = parse(
            r#"
version: 1
systems:
  - name: "PurchaseProduct"
    mode: "contract"
    description: "buy a product"
    input:
      - { name: "buyer_id", type: "uuid", required: true }
    output:
      - { name: "order_id", type: "uuid" }
    errors:
      - "BuyerNotFound"
      - "PaymentFailed(String)"
"#,
        );
        assert_eq!(spec.systems[0].mode, SystemMode::Contract);
        assert_eq!(spec.systems[0].output.len(), 1);
        assert_eq!(spec.systems[0].errors.len(), 2);
        assert!(spec.systems[0].steps.is_empty());
    }

    #[test]
    fn default_mode_is_generated() {
        let spec = parse(
            r#"
version: 1
systems:
  - name: "Test"
    description: "test"
    input: []
    steps: []
"#,
        );
        assert_eq!(spec.systems[0].mode, SystemMode::Generated);
    }

    #[test]
    fn parse_full_systems_yaml() {
        let yaml = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/systems.yaml"
        ))
        .unwrap();
        let spec: SystemsSpec = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(spec.version, 1);
        assert_eq!(spec.integrations.len(), 3);
        assert_eq!(spec.systems.len(), 7);
    }
}
