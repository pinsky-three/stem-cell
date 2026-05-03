use crate::spec::*;
use std::collections::{HashMap, HashSet};

fn is_safe_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

const VALID_TYPES: &[&str] = &[
    "uuid",
    "string",
    "text",
    "int",
    "bigint",
    "float",
    "bool",
    "timestamp",
    "decimal",
    "json",
];

pub fn validate(spec: &SystemsSpec) -> Vec<String> {
    let mut errors = Vec::new();

    if spec.version != 1 {
        errors.push(format!("unsupported version {}, expected 1", spec.version));
    }

    let integrations = validate_integrations(&spec.integrations, &mut errors);

    let mut system_names: HashSet<&str> = HashSet::new();
    for system in &spec.systems {
        if !system_names.insert(&system.name) {
            errors.push(format!("duplicate system name '{}'", system.name));
        }
        validate_system(system, &integrations, &mut errors);
    }

    errors
}

// ── Integration validation ──────────────────────────────────────────────

fn validate_integrations<'a>(
    integrations: &'a [IntegrationSpec],
    errors: &mut Vec<String>,
) -> HashMap<&'a str, HashMap<&'a str, &'a OperationSpec>> {
    let mut map: HashMap<&str, HashMap<&str, &OperationSpec>> = HashMap::new();
    let mut names: HashSet<&str> = HashSet::new();

    for integration in integrations {
        if !names.insert(&integration.name) {
            errors.push(format!("duplicate integration name '{}'", integration.name));
        }
        if !is_safe_identifier(&integration.name) {
            errors.push(format!(
                "integration '{}': name must be alphanumeric/underscore",
                integration.name
            ));
        }

        let mut op_names: HashSet<&str> = HashSet::new();
        let mut op_map: HashMap<&str, &OperationSpec> = HashMap::new();

        for op in &integration.operations {
            if !op_names.insert(&op.name) {
                errors.push(format!(
                    "integration '{}': duplicate operation '{}'",
                    integration.name, op.name
                ));
            }
            if !is_safe_identifier(&op.name) {
                errors.push(format!(
                    "integration '{}.{}': name must be alphanumeric/underscore",
                    integration.name, op.name
                ));
            }
            for p in op.input.iter().chain(op.output.iter()) {
                if !is_safe_identifier(&p.name) {
                    errors.push(format!(
                        "integration '{}.{}': param '{}' must be alphanumeric/underscore",
                        integration.name, op.name, p.name
                    ));
                }
                if !VALID_TYPES.contains(&p.ty.as_str()) {
                    errors.push(format!(
                        "integration '{}.{}.{}': unsupported type '{}'",
                        integration.name, op.name, p.name, p.ty
                    ));
                }
            }
            op_map.insert(&op.name, op);
        }
        map.insert(&integration.name, op_map);
    }

    map
}

// ── System validation ───────────────────────────────────────────────────

fn validate_system(
    system: &SystemDef,
    integrations: &HashMap<&str, HashMap<&str, &OperationSpec>>,
    errors: &mut Vec<String>,
) {
    let ctx = &system.name;

    if !is_safe_identifier(&system.name) {
        errors.push(format!(
            "system '{}': name must be alphanumeric/underscore",
            ctx
        ));
    }

    let mut input_names: HashSet<&str> = HashSet::new();
    for f in &system.input {
        if !input_names.insert(&f.name) {
            errors.push(format!("{ctx}: duplicate input field '{}'", f.name));
        }
        if !is_safe_identifier(&f.name) {
            errors.push(format!(
                "{ctx}: input '{}' must be alphanumeric/underscore",
                f.name
            ));
        }
        if !VALID_TYPES.contains(&f.ty.as_str()) {
            errors.push(format!(
                "{ctx}: input '{}' has unsupported type '{}'",
                f.name, f.ty
            ));
        }
    }

    match system.mode {
        SystemMode::Generated => {
            validate_generated_system(system, integrations, errors, ctx);
        }
        SystemMode::Contract => {
            validate_contract_system(system, errors, ctx);
        }
    }
}

fn validate_generated_system(
    system: &SystemDef,
    integrations: &HashMap<&str, HashMap<&str, &OperationSpec>>,
    errors: &mut Vec<String>,
    ctx: &str,
) {
    if !system.output.is_empty() {
        errors.push(format!(
            "{ctx}: generated-mode systems must not have 'output' (use 'result' instead)"
        ));
    }
    if !system.errors.is_empty() {
        errors.push(format!(
            "{ctx}: generated-mode systems must not have 'errors'"
        ));
    }

    let mut bindings: HashSet<String> = HashSet::new();
    bindings.insert("input".into());

    validate_steps(&system.steps, &mut bindings, integrations, errors, ctx);

    for r in &system.result {
        if !is_safe_identifier(&r.name) {
            errors.push(format!(
                "{ctx}: result '{}' must be alphanumeric/underscore",
                r.name
            ));
        }
        if !bindings.contains(r.from.as_str()) {
            errors.push(format!(
                "{ctx}: result '{}' references unknown binding '{}'",
                r.name, r.from
            ));
        }
    }
}

fn validate_contract_system(system: &SystemDef, errors: &mut Vec<String>, ctx: &str) {
    if !system.steps.is_empty() {
        errors.push(format!(
            "{ctx}: contract-mode systems must not have 'steps'"
        ));
    }
    if !system.result.is_empty() {
        errors.push(format!(
            "{ctx}: contract-mode systems must not have 'result' (use 'output' instead)"
        ));
    }

    let mut output_names: HashSet<&str> = HashSet::new();
    for f in &system.output {
        if !output_names.insert(&f.name) {
            errors.push(format!("{ctx}: duplicate output field '{}'", f.name));
        }
        if !is_safe_identifier(&f.name) {
            errors.push(format!(
                "{ctx}: output '{}' must be alphanumeric/underscore",
                f.name
            ));
        }
        if !VALID_TYPES.contains(&f.ty.as_str()) {
            errors.push(format!(
                "{ctx}: output '{}' has unsupported type '{}'",
                f.name, f.ty
            ));
        }
    }

    for (i, variant) in system.errors.iter().enumerate() {
        let (name, _) = parse_error_variant(variant);
        if !is_safe_identifier(&name) {
            errors.push(format!(
                "{ctx}: error variant #{} '{}' must be alphanumeric/underscore",
                i, name
            ));
        }
    }
}

/// Parse an error variant string like `"NotFound"` or `"PaymentFailed(String)"`.
/// Returns `(name, has_string_payload)`.
pub fn parse_error_variant(variant: &str) -> (String, bool) {
    if let Some(paren) = variant.find('(') {
        let name = variant[..paren].to_string();
        (name, true)
    } else {
        (variant.to_string(), false)
    }
}

// ── Step validation (recursive for branch) ──────────────────────────────

fn validate_steps(
    steps: &[Step],
    bindings: &mut HashSet<String>,
    integrations: &HashMap<&str, HashMap<&str, &OperationSpec>>,
    errors: &mut Vec<String>,
    ctx: &str,
) {
    for step in steps {
        match step {
            Step::LoadOne(s) => {
                validate_path(&s.by, bindings, errors, ctx, "load_one.by");
                if !is_safe_identifier(&s.binding) {
                    errors.push(format!(
                        "{ctx}: load_one binding '{}' must be alphanumeric/underscore",
                        s.binding
                    ));
                }
                if !bindings.insert(s.binding.clone()) {
                    errors.push(format!("{ctx}: duplicate binding '{}'", s.binding));
                }
            }
            Step::LoadMany(s) => {
                if !is_safe_identifier(&s.filter.field) {
                    errors.push(format!(
                        "{ctx}: load_many filter field '{}' must be alphanumeric/underscore",
                        s.filter.field
                    ));
                }
                validate_path(
                    &s.filter.from,
                    bindings,
                    errors,
                    ctx,
                    "load_many.filter.from",
                );
                if !is_safe_identifier(&s.binding) {
                    errors.push(format!(
                        "{ctx}: load_many binding '{}' must be alphanumeric/underscore",
                        s.binding
                    ));
                }
                if !bindings.insert(s.binding.clone()) {
                    errors.push(format!("{ctx}: duplicate binding '{}'", s.binding));
                }
            }
            Step::Create(s) => {
                for m in &s.set {
                    validate_field_mapping(m, bindings, errors, ctx, "create.set");
                }
                if let Some(ref b) = s.binding {
                    if !is_safe_identifier(b) {
                        errors.push(format!(
                            "{ctx}: create binding '{b}' must be alphanumeric/underscore"
                        ));
                    }
                    if !bindings.insert(b.clone()) {
                        errors.push(format!("{ctx}: duplicate binding '{b}'"));
                    }
                }
            }
            Step::Update(s) => {
                validate_path(&s.target, bindings, errors, ctx, "update.target");
                for m in &s.set {
                    validate_field_mapping(m, bindings, errors, ctx, "update.set");
                }
            }
            Step::Delete(s) => {
                validate_path(&s.target, bindings, errors, ctx, "delete.target");
            }
            Step::Guard(s) => {
                validate_condition(&s.check, bindings, errors, ctx);
            }
            Step::Branch(s) => {
                validate_condition(&s.check, bindings, errors, ctx);
                let mut branch_bindings = bindings.clone();
                validate_steps(&s.then, &mut branch_bindings, integrations, errors, ctx);
                if let Some(ref else_steps) = s.otherwise {
                    let mut else_bindings = bindings.clone();
                    validate_steps(else_steps, &mut else_bindings, integrations, errors, ctx);
                }
            }
            Step::CallIntegration(s) => {
                match integrations.get(s.integration.as_str()) {
                    None => {
                        errors.push(format!("{ctx}: unknown integration '{}'", s.integration));
                    }
                    Some(ops) => {
                        if !ops.contains_key(s.operation.as_str()) {
                            errors.push(format!(
                                "{ctx}: unknown operation '{}.{}'",
                                s.integration, s.operation
                            ));
                        }
                    }
                }
                for inp in &s.input {
                    validate_integration_input(inp, bindings, errors, ctx);
                }
                if !is_safe_identifier(&s.binding) {
                    errors.push(format!(
                        "{ctx}: call_integration binding '{}' must be alphanumeric/underscore",
                        s.binding
                    ));
                }
                if !bindings.insert(s.binding.clone()) {
                    errors.push(format!("{ctx}: duplicate binding '{}'", s.binding));
                }
            }
            Step::EmitEvent(s) => {
                if !is_safe_identifier(&s.event) {
                    errors.push(format!(
                        "{ctx}: event name '{}' must be alphanumeric/underscore",
                        s.event
                    ));
                }
                for p in &s.payload {
                    if !is_safe_identifier(&p.field) {
                        errors.push(format!(
                            "{ctx}: event payload field '{}' must be alphanumeric/underscore",
                            p.field
                        ));
                    }
                    validate_path(&p.from, bindings, errors, ctx, "emit_event.payload.from");
                }
            }
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Validates a `binding.field` dotted path.
fn validate_path(
    path: &str,
    bindings: &HashSet<String>,
    errors: &mut Vec<String>,
    ctx: &str,
    location: &str,
) {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.len() != 2 {
        errors.push(format!(
            "{ctx}: {location} '{path}' must be 'binding.field'"
        ));
        return;
    }
    if !bindings.contains(parts[0]) {
        errors.push(format!(
            "{ctx}: {location} references unknown binding '{}'",
            parts[0]
        ));
    }
    if !is_safe_identifier(parts[1]) {
        errors.push(format!(
            "{ctx}: {location} field part '{}' must be alphanumeric/underscore",
            parts[1]
        ));
    }
}

fn validate_condition(
    cond: &Condition,
    bindings: &HashSet<String>,
    errors: &mut Vec<String>,
    ctx: &str,
) {
    validate_path(&cond.field, bindings, errors, ctx, "condition.field");

    let count = cond.equals.is_some() as u8
        + cond.not_equals.is_some() as u8
        + cond.equals_field.is_some() as u8
        + cond.not_equals_field.is_some() as u8;

    if count == 0 {
        errors.push(format!(
            "{ctx}: condition on '{}' has no comparator (need equals, not_equals, equals_field, or not_equals_field)",
            cond.field
        ));
    } else if count > 1 {
        errors.push(format!(
            "{ctx}: condition on '{}' has multiple comparators (pick exactly one)",
            cond.field
        ));
    }

    if let Some(ref f) = cond.equals_field {
        validate_path(f, bindings, errors, ctx, "condition.equals_field");
    }
    if let Some(ref f) = cond.not_equals_field {
        validate_path(f, bindings, errors, ctx, "condition.not_equals_field");
    }
}

fn validate_field_mapping(
    m: &FieldMapping,
    bindings: &HashSet<String>,
    errors: &mut Vec<String>,
    ctx: &str,
    location: &str,
) {
    if !is_safe_identifier(&m.field) {
        errors.push(format!(
            "{ctx}: {location} field '{}' must be alphanumeric/underscore",
            m.field
        ));
    }
    match (&m.from, &m.value) {
        (None, None) => {
            errors.push(format!(
                "{ctx}: {location} field '{}' needs either 'from' or 'value'",
                m.field
            ));
        }
        (Some(_), Some(_)) => {
            errors.push(format!(
                "{ctx}: {location} field '{}' has both 'from' and 'value' (pick one)",
                m.field
            ));
        }
        (Some(path), None) => {
            validate_path(path, bindings, errors, ctx, &format!("{location}.from"));
        }
        (None, Some(_)) => {}
    }
}

fn validate_integration_input(
    inp: &IntegrationInput,
    bindings: &HashSet<String>,
    errors: &mut Vec<String>,
    ctx: &str,
) {
    if !is_safe_identifier(&inp.param) {
        errors.push(format!(
            "{ctx}: integration param '{}' must be alphanumeric/underscore",
            inp.param
        ));
    }
    match (&inp.from, &inp.value) {
        (None, None) => {
            errors.push(format!(
                "{ctx}: integration param '{}' needs either 'from' or 'value'",
                inp.param
            ));
        }
        (Some(_), Some(_)) => {
            errors.push(format!(
                "{ctx}: integration param '{}' has both 'from' and 'value' (pick one)",
                inp.param
            ));
        }
        (Some(path), None) => {
            validate_path(path, bindings, errors, ctx, "call_integration.input.from");
        }
        (None, Some(_)) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::SystemsSpec;

    fn parse(yaml: &str) -> SystemsSpec {
        serde_yaml::from_str(yaml).unwrap()
    }

    const VALID: &str = r#"
version: 1
integrations:
  - name: "provider"
    operations:
      - name: "call"
        input:
          - { name: "x", type: "string" }
        output:
          - { name: "y", type: "string" }
systems:
  - name: "DoThing"
    description: "test"
    input:
      - { name: "item_id", type: "uuid", required: true }
    steps:
      - kind: "load_one"
        entity: "Item"
        by: "input.item_id"
        as: "item"
        not_found: "not found"
      - kind: "guard"
        check: { field: "item.active", equals: true }
        error: "inactive"
      - kind: "emit_event"
        event: "thing_done"
        payload:
          - { field: "item_id", from: "item.id" }
    result:
      - { name: "item", from: "item" }
"#;

    #[test]
    fn valid_spec_passes() {
        assert!(validate(&parse(VALID)).is_empty());
    }

    #[test]
    fn wrong_version() {
        let yaml = VALID.replace("version: 1", "version: 2");
        let errs = validate(&parse(&yaml));
        assert!(errs.iter().any(|e| e.contains("version")));
    }

    #[test]
    fn duplicate_system_name() {
        let yaml = r#"
version: 1
systems:
  - name: "A"
    description: "a"
    input: []
    steps: []
    result: []
  - name: "A"
    description: "b"
    input: []
    steps: []
    result: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("duplicate system")));
    }

    #[test]
    fn duplicate_integration_name() {
        let yaml = r#"
version: 1
integrations:
  - name: "x"
    operations: []
  - name: "x"
    operations: []
systems: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("duplicate integration")));
    }

    #[test]
    fn duplicate_operation_name() {
        let yaml = r#"
version: 1
integrations:
  - name: "x"
    operations:
      - name: "op"
        input: []
        output: []
      - name: "op"
        input: []
        output: []
systems: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("duplicate operation")));
    }

    #[test]
    fn unsafe_system_name() {
        let yaml = r#"
version: 1
systems:
  - name: "bad name"
    description: "x"
    input: []
    steps: []
    result: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("alphanumeric")));
    }

    #[test]
    fn invalid_input_type() {
        let yaml = r#"
version: 1
systems:
  - name: "T"
    description: "x"
    input:
      - { name: "a", type: "integer", required: true }
    steps: []
    result: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("unsupported type")));
    }

    #[test]
    fn condition_missing_comparator() {
        let yaml = r#"
version: 1
systems:
  - name: "T"
    description: "x"
    input: []
    steps:
      - kind: "guard"
        check: { field: "x.y" }
        error: "fail"
    result: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("no comparator")));
    }

    #[test]
    fn condition_multiple_comparators() {
        let yaml = r#"
version: 1
systems:
  - name: "T"
    description: "x"
    input:
      - { name: "a", type: "uuid", required: true }
    steps:
      - kind: "load_one"
        entity: "X"
        by: "input.a"
        as: "x"
        not_found: "nope"
      - kind: "guard"
        check: { field: "x.y", equals: true, not_equals: false }
        error: "fail"
    result: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("multiple comparators")));
    }

    #[test]
    fn field_mapping_missing_source() {
        let yaml = r#"
version: 1
systems:
  - name: "T"
    description: "x"
    input: []
    steps:
      - kind: "create"
        entity: "X"
        set:
          - { field: "a" }
    result: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs
            .iter()
            .any(|e| e.contains("needs either 'from' or 'value'")));
    }

    #[test]
    fn field_mapping_both_sources() {
        let yaml = r#"
version: 1
systems:
  - name: "T"
    description: "x"
    input:
      - { name: "v", type: "string", required: true }
    steps:
      - kind: "create"
        entity: "X"
        set:
          - { field: "a", from: "input.v", value: "nope" }
    result: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("both 'from' and 'value'")));
    }

    #[test]
    fn duplicate_binding() {
        let yaml = r#"
version: 1
systems:
  - name: "T"
    description: "x"
    input:
      - { name: "a", type: "uuid", required: true }
      - { name: "b", type: "uuid", required: true }
    steps:
      - kind: "load_one"
        entity: "X"
        by: "input.a"
        as: "item"
        not_found: "nope"
      - kind: "load_one"
        entity: "Y"
        by: "input.b"
        as: "item"
        not_found: "nope"
    result: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("duplicate binding")));
    }

    #[test]
    fn result_unknown_binding() {
        let yaml = r#"
version: 1
systems:
  - name: "T"
    description: "x"
    input: []
    steps: []
    result:
      - { name: "x", from: "ghost" }
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("unknown binding")));
    }

    #[test]
    fn unknown_integration() {
        let yaml = r#"
version: 1
systems:
  - name: "T"
    description: "x"
    input: []
    steps:
      - kind: "call_integration"
        integration: "ghost"
        operation: "op"
        input: []
        as: "r"
    result: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("unknown integration")));
    }

    #[test]
    fn unknown_operation() {
        let yaml = r#"
version: 1
integrations:
  - name: "svc"
    operations: []
systems:
  - name: "T"
    description: "x"
    input: []
    steps:
      - kind: "call_integration"
        integration: "svc"
        operation: "ghost"
        input: []
        as: "r"
    result: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("unknown operation")));
    }

    #[test]
    fn invalid_from_path() {
        let yaml = r#"
version: 1
systems:
  - name: "T"
    description: "x"
    input: []
    steps:
      - kind: "load_one"
        entity: "X"
        by: "no_dot"
        as: "x"
        not_found: "nope"
    result: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("binding.field")));
    }

    #[test]
    fn contract_mode_rejects_steps() {
        let yaml = r#"
version: 1
systems:
  - name: "T"
    mode: "contract"
    description: "x"
    input: []
    steps:
      - kind: "guard"
        check: { field: "x.y", equals: true }
        error: "fail"
    output: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("must not have 'steps'")));
    }

    #[test]
    fn contract_mode_rejects_result() {
        let yaml = r#"
version: 1
systems:
  - name: "T"
    mode: "contract"
    description: "x"
    input: []
    output: []
    result:
      - { name: "x", from: "y" }
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("must not have 'result'")));
    }

    #[test]
    fn generated_mode_rejects_output() {
        let yaml = r#"
version: 1
systems:
  - name: "T"
    description: "x"
    input: []
    steps: []
    output:
      - { name: "x", type: "string" }
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("must not have 'output'")));
    }

    #[test]
    fn contract_mode_valid() {
        let yaml = r#"
version: 1
systems:
  - name: "Buy"
    mode: "contract"
    description: "buy"
    input:
      - { name: "id", type: "uuid", required: true }
    output:
      - { name: "order_id", type: "uuid" }
    errors:
      - "NotFound"
      - "PaymentFailed(String)"
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
    }

    #[test]
    fn validates_full_systems_yaml() {
        let yaml = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/systems.yaml"
        ))
        .unwrap();
        let spec: SystemsSpec = serde_yaml::from_str(&yaml).unwrap();
        let errs = validate(&spec);
        assert!(errs.is_empty(), "validation errors: {errs:?}");
    }
}
