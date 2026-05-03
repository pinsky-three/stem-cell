use clap::Parser;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::{fs, io::Write};

// ── CLI ─────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "systems-codegen",
    about = "Materialize impl stubs and test scaffolds from systems.yaml"
)]
struct Cli {
    /// Path to systems.yaml
    #[arg(long, default_value = "specs/systems.yaml")]
    spec: PathBuf,

    /// Runtime crate root (where src/ and tests/ live)
    #[arg(long, default_value = "crates/runtime")]
    runtime: PathBuf,

    /// Binary crate name for test imports
    #[arg(long, default_value = "stem_cell")]
    crate_name: String,

    /// Overwrite existing stubs (even without @generated-stub marker)
    #[arg(long)]
    force: bool,
}

// ── Minimal spec types ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct SystemsSpec {
    #[allow(dead_code)]
    version: u32,
    #[serde(default)]
    integrations: Vec<IntegrationSpec>,
    #[serde(default)]
    systems: Vec<SystemDef>,
}

#[derive(Deserialize)]
struct IntegrationSpec {
    name: String,
    operations: Vec<OperationSpec>,
}

#[derive(Deserialize)]
struct OperationSpec {
    name: String,
    input: Vec<ParamSpec>,
    output: Vec<ParamSpec>,
}

#[derive(Deserialize)]
struct ParamSpec {
    name: String,
    #[serde(rename = "type")]
    ty: String,
}

#[derive(Deserialize)]
struct SystemDef {
    name: String,
    #[allow(dead_code)]
    description: String,
    #[serde(default)]
    mode: SystemMode,
    input: Vec<InputField>,
    #[serde(default)]
    output: Vec<OutputField>,
    #[serde(default)]
    errors: Vec<String>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
enum SystemMode {
    #[default]
    Generated,
    Contract,
}

#[derive(Deserialize)]
struct InputField {
    name: String,
    #[serde(rename = "type")]
    ty: String,
    required: bool,
}

#[derive(Deserialize)]
struct OutputField {
    name: String,
    #[serde(rename = "type")]
    ty: String,
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.extend(c.to_lowercase());
    }
    result
}

fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect()
}

fn default_value_for(ty: &str) -> &str {
    match ty {
        "uuid" => "uuid::Uuid::new_v4()",
        "string" | "text" => "\"test\".to_string()",
        "int" => "1",
        "bigint" => "100",
        "float" => "1.0",
        "bool" => "true",
        "timestamp" => "chrono::Utc::now()",
        "decimal" => "rust_decimal::Decimal::ZERO",
        "json" => "serde_json::json!({})",
        _ => "Default::default()",
    }
}

fn parse_error_variant(variant: &str) -> (&str, bool) {
    if let Some(paren) = variant.find('(') {
        (&variant[..paren], true)
    } else {
        (variant, false)
    }
}

const STUB_MARKER: &str = "// @generated-stub";

fn should_write(path: &Path, force: bool) -> bool {
    if !path.exists() {
        return true;
    }
    if force {
        return true;
    }
    if let Ok(content) = fs::read_to_string(path) {
        content.starts_with(STUB_MARKER)
    } else {
        false
    }
}

fn write_if_allowed(path: &Path, content: &str, force: bool) -> bool {
    if !should_write(path, force) {
        eprintln!("  skip (exists, no marker): {}", path.display());
        return false;
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let mut f =
        fs::File::create(path).unwrap_or_else(|e| panic!("cannot create {}: {e}", path.display()));
    f.write_all(content.as_bytes()).unwrap();
    eprintln!("  wrote: {}", path.display());
    true
}

// ── Main ────────────────────────────────────────────────────────────────

fn main() {
    let cli = Cli::parse();

    let yaml = fs::read_to_string(&cli.spec)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", cli.spec.display()));
    let spec: SystemsSpec =
        serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("cannot parse YAML: {e}"));

    let contract_systems: Vec<&SystemDef> = spec
        .systems
        .iter()
        .filter(|s| s.mode == SystemMode::Contract)
        .collect();

    if contract_systems.is_empty() && spec.integrations.is_empty() {
        eprintln!("No contract systems or integrations found — nothing to generate.");
        return;
    }

    let src_dir = cli.runtime.join("src");
    let tests_dir = cli.runtime.join("tests");

    eprintln!("=== Contract system stubs ===");
    generate_system_stubs(&contract_systems, &src_dir, &cli.crate_name, cli.force);

    let integrations_flat = src_dir.join("integrations.rs");
    if integrations_flat.exists() {
        eprintln!("\n=== Integration stubs ===");
        eprintln!("  skip: src/integrations.rs already exists (flat file)");
        eprintln!("  hint: integration stubs are generated into src/integrations/ only when");
        eprintln!("        src/integrations.rs does not exist. Delete the flat file to switch.");
    } else {
        eprintln!("\n=== Integration stubs ===");
        generate_integration_stubs(&spec.integrations, &src_dir, &cli.crate_name, cli.force);
    }

    eprintln!("\n=== Contract tests ===");
    generate_contract_tests(
        &contract_systems,
        &spec.integrations,
        &tests_dir,
        &cli.crate_name,
        cli.force,
    );

    eprintln!("\n=== Systems module registry ===");
    generate_systems_mod(&contract_systems, &src_dir, cli.force);

    eprintln!("\nDone. Run `cargo check` to verify.");
}

// ── System stubs ────────────────────────────────────────────────────────

fn generate_system_stubs(systems: &[&SystemDef], src_dir: &Path, _crate_name: &str, force: bool) {
    let systems_dir = src_dir.join("systems");

    for system in systems {
        let snake = to_snake_case(&system.name);
        let path = systems_dir.join(format!("{snake}.rs"));

        let input_fields: String = system
            .input
            .iter()
            .map(|f| format!("        &input.{},", f.name))
            .collect::<Vec<_>>()
            .join("\n");

        let output_fields: String = system
            .output
            .iter()
            .map(|f| {
                let default = default_value_for(&f.ty);
                format!("            {}: {default},", f.name)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let _log_fields: String = system
            .input
            .iter()
            .filter(|f| f.required)
            .take(3)
            .map(|f| {
                let n = &f.name;
                match f.ty.as_str() {
                    "uuid" | "string" | "text" => format!("{n} = %input.{n}"),
                    _ => format!("{n} = input.{n}"),
                }
            })
            .collect::<Vec<_>>()
            .join(", ");

        let content = format!(
            r#"{STUB_MARKER} — safe to edit; will not be overwritten if this marker is removed
use crate::system_api::*;

#[async_trait::async_trait]
impl {name}System for super::AppSystems {{
    async fn execute(
        &self,
        _pool: &sqlx::PgPool,
        input: {name}Input,
    ) -> Result<{name}Output, {name}Error> {{
        tracing::info!("{snake}.execute called (stub)");

        let _ = (
{input_fields}
        );

        // TODO: implement {name} business logic
        Ok({name}Output {{
{output_fields}
        }})
    }}
}}
"#,
            name = system.name,
        );

        write_if_allowed(&path, &content, force);
    }
}

// ── Integration stubs ───────────────────────────────────────────────────

fn generate_integration_stubs(
    integrations: &[IntegrationSpec],
    src_dir: &Path,
    _crate_name: &str,
    force: bool,
) {
    let integrations_dir = src_dir.join("integrations");

    for integration in integrations {
        let path = integrations_dir.join(format!("{}.rs", integration.name));

        let mut methods = Vec::new();

        for op in &integration.operations {
            let pascal_integration = to_pascal_case(&integration.name);
            let pascal_op = to_pascal_case(&op.name);
            let composite = format!("{pascal_integration}{pascal_op}");
            let method_name = format!("{}_{}", integration.name, op.name);

            let output_fields: String = op
                .output
                .iter()
                .map(|p| {
                    let default = default_value_for(&p.ty);
                    format!("            {}: {default},", p.name)
                })
                .collect::<Vec<_>>()
                .join("\n");

            methods.push(format!(
                r#"    async fn {method_name}(
        &self,
        input: {composite}Input,
    ) -> Result<{composite}Output, IntegrationError> {{
        tracing::info!(
            "integration {int_name}.{op_name} called (stub)"
        );

        let _ = &input;

        // TODO: implement real {int_name}.{op_name} call
        Ok({composite}Output {{
{output_fields}
        }})
    }}"#,
                int_name = integration.name,
                op_name = op.name,
            ));
        }

        let trait_name = to_pascal_case(&integration.name);
        let all_methods = methods.join("\n\n");

        let content = format!(
            r#"{STUB_MARKER} — safe to edit; will not be overwritten if this marker is removed
use crate::system_api::*;

#[async_trait::async_trait]
impl {trait_name} for crate::integrations::AppIntegrations {{
{all_methods}
}}
"#
        );

        write_if_allowed(&path, &content, force);
    }
}

// ── Contract tests ──────────────────────────────────────────────────────

fn generate_contract_tests(
    systems: &[&SystemDef],
    integrations: &[IntegrationSpec],
    tests_dir: &Path,
    crate_name: &str,
    force: bool,
) {
    let contracts_dir = tests_dir.join("contracts");

    for system in systems {
        let snake = to_snake_case(&system.name);
        let path = contracts_dir.join(format!("{snake}.rs"));

        let input_constructor: String = system
            .input
            .iter()
            .map(|f| {
                let default = default_value_for(&f.ty);
                if f.required {
                    format!("        {}: {default},", f.name)
                } else {
                    format!("        {}: Some({default}),", f.name)
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        let output_constructor: String = system
            .output
            .iter()
            .map(|f| {
                let default = default_value_for(&f.ty);
                format!("        {}: {default},", f.name)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let error_tests: String = system
            .errors
            .iter()
            .map(|variant_str| {
                let (name, has_payload) = parse_error_variant(variant_str);
                let test_name = format!("error_{}_converts_to_system_error", to_snake_case(name));
                let ident = name;
                if has_payload {
                    format!(
                        r#"
#[test]
fn {test_name}() {{
    let e = {sys_name}Error::{ident}("test".into());
    let se: SystemError = e.into();
    let msg = format!("{{se}}");
    assert!(msg.contains("{ident}"), "expected '{ident}' in '{{msg}}'");
}}"#,
                        sys_name = system.name,
                    )
                } else {
                    format!(
                        r#"
#[test]
fn {test_name}() {{
    let e = {sys_name}Error::{ident};
    let se: SystemError = e.into();
    let msg = format!("{{se}}");
    assert!(msg.contains("{ident}"), "expected '{ident}' in '{{msg}}'");
}}"#,
                        sys_name = system.name,
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        let content = format!(
            r#"{STUB_MARKER} — safe to edit; will not be overwritten if this marker is removed
use {crate_name}::system_api::*;

#[test]
fn {snake}_input_roundtrips_json() {{
    let input = {name}Input {{
{input_constructor}
    }};
    let json = serde_json::to_string(&input).unwrap();
    let decoded: {name}Input = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}}

#[test]
fn {snake}_output_roundtrips_json() {{
    let output = {name}Output {{
{output_constructor}
    }};
    let json = serde_json::to_string(&output).unwrap();
    let decoded: {name}Output = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}}

#[test]
fn {snake}_internal_error_converts() {{
    let e = {name}Error::Internal("oops".into());
    let se: SystemError = e.into();
    let msg = format!("{{se}}");
    assert!(msg.contains("internal"), "expected 'internal' in '{{msg}}'");
}}
{error_tests}
"#,
            name = system.name,
        );

        write_if_allowed(&path, &content, force);
    }

    // Integration contract tests
    for integration in integrations {
        let path = contracts_dir.join(format!("{}_integration.rs", integration.name));

        let mut tests = Vec::new();
        for op in &integration.operations {
            let pascal_integration = to_pascal_case(&integration.name);
            let pascal_op = to_pascal_case(&op.name);
            let composite = format!("{pascal_integration}{pascal_op}");

            let input_fields: String = op
                .input
                .iter()
                .map(|p| {
                    let default = default_value_for(&p.ty);
                    format!("        {}: {default},", p.name)
                })
                .collect::<Vec<_>>()
                .join("\n");

            let output_fields: String = op
                .output
                .iter()
                .map(|p| {
                    let default = default_value_for(&p.ty);
                    format!("        {}: {default},", p.name)
                })
                .collect::<Vec<_>>()
                .join("\n");

            let test_name = format!("{}_{}_io_roundtrips", integration.name, op.name);

            tests.push(format!(
                r#"
#[test]
fn {test_name}() {{
    let input = {composite}Input {{
{input_fields}
    }};
    let _ = format!("{{input:?}}");

    let output = {composite}Output {{
{output_fields}
    }};
    let _ = format!("{{output:?}}");
}}"#
            ));
        }

        let all_tests = tests.join("\n");
        let content = format!("{STUB_MARKER}\nuse {crate_name}::system_api::*;\n{all_tests}\n");
        write_if_allowed(&path, &content, force);
    }

    // Generate main.rs entry point for tests/contracts/ test crate
    if !systems.is_empty() || !integrations.is_empty() {
        let main_path = contracts_dir.join("main.rs");
        let mut entries: Vec<String> = systems
            .iter()
            .map(|s| format!("mod {};", to_snake_case(&s.name)))
            .collect();
        for integration in integrations {
            entries.push(format!("mod {}_integration;", integration.name));
        }
        let content = format!("{STUB_MARKER}\n{}\n", entries.join("\n"));
        write_if_allowed(&main_path, &content, true);
    }
}

// ── Systems module registry ─────────────────────────────────────────────

fn generate_systems_mod(systems: &[&SystemDef], src_dir: &Path, force: bool) {
    if systems.is_empty() {
        return;
    }

    let systems_dir = src_dir.join("systems");
    let mod_path = systems_dir.join("mod.rs");

    let mod_entries: String = systems
        .iter()
        .map(|s| format!("mod {};", to_snake_case(&s.name)))
        .collect::<Vec<_>>()
        .join("\n");

    let content = format!(
        r#"{STUB_MARKER}
{mod_entries}

/// Concrete implementation of all contract-mode system traits.
/// Each sub-module implements one trait on this struct.
#[derive(Clone)]
pub struct AppSystems;
"#
    );

    write_if_allowed(&mod_path, &content, force);
}
