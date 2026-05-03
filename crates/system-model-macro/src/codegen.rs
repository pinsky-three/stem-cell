use crate::spec::*;
use crate::validate::parse_error_variant;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::HashMap;

// ── Helpers ─────────────────────────────────────────────────────────────

fn map_type(ty: &str) -> TokenStream {
    match ty {
        "uuid" => quote! { uuid::Uuid },
        "string" | "text" => quote! { String },
        "int" => quote! { i32 },
        "bigint" => quote! { i64 },
        "float" => quote! { f64 },
        "bool" => quote! { bool },
        "timestamp" => quote! { chrono::DateTime<chrono::Utc> },
        "decimal" => quote! { rust_decimal::Decimal },
        "json" => quote! { serde_json::Value },
        _ => unreachable!("unsupported type '{ty}' should have been caught by validation"),
    }
}

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

/// Converts a `binding.field` path to Rust tokens: `binding.field`.
fn path_to_tokens(path: &str) -> TokenStream {
    let parts: Vec<&str> = path.split('.').collect();
    let binding = format_ident!("{}", parts[0]);
    let field = format_ident!("{}", parts[1]);
    quote! { #binding.#field }
}

/// Converts a `binding.field` path to tokens, adding `.clone()`.
fn path_to_cloned_tokens(path: &str) -> TokenStream {
    let inner = path_to_tokens(path);
    quote! { #inner.clone() }
}

fn yaml_value_to_comparison_tokens(value: &serde_yaml::Value) -> TokenStream {
    match value {
        serde_yaml::Value::Bool(b) => quote! { #b },
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                let lit = proc_macro2::Literal::i64_unsuffixed(i);
                quote! { #lit }
            } else if let Some(f) = n.as_f64() {
                let lit = proc_macro2::Literal::f64_unsuffixed(f);
                quote! { #lit }
            } else {
                unreachable!()
            }
        }
        serde_yaml::Value::String(s) => quote! { #s },
        serde_yaml::Value::Null => quote! { None },
        _ => unreachable!("unsupported YAML value in condition"),
    }
}

fn yaml_value_to_assignment_tokens(value: &serde_yaml::Value) -> TokenStream {
    match value {
        serde_yaml::Value::Bool(b) => quote! { #b },
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                let lit = proc_macro2::Literal::i64_unsuffixed(i);
                quote! { #lit }
            } else if let Some(f) = n.as_f64() {
                let lit = proc_macro2::Literal::f64_unsuffixed(f);
                quote! { #lit }
            } else {
                unreachable!()
            }
        }
        serde_yaml::Value::String(s) => quote! { #s.to_string() },
        serde_yaml::Value::Null => quote! { None },
        _ => unreachable!("unsupported YAML value in assignment"),
    }
}

/// Integration + operation names → PascalCase composite: `PaymentProviderCreateCharge`.
fn integration_op_pascal(integration: &str, operation: &str) -> String {
    format!(
        "{}{}",
        to_pascal_case(integration),
        to_pascal_case(operation)
    )
}

// ── Binding type tracking ───────────────────────────────────────────────

#[derive(Clone)]
enum BindingKind {
    Entity(String),
    EntityVec(String),
    IntegrationOutput(String, String),
}

// ── Top-level generator ─────────────────────────────────────────────────

pub fn generate(spec: &SystemsSpec) -> TokenStream {
    let error_types = generate_error_types();
    let integration_code = generate_all_integrations(spec);
    let event_code = generate_all_events(spec);

    let generated_systems: Vec<TokenStream> = spec
        .systems
        .iter()
        .filter(|s| s.mode == SystemMode::Generated)
        .map(|s| generate_system(s, spec))
        .collect();

    let contract_systems: Vec<TokenStream> = spec
        .systems
        .iter()
        .filter(|s| s.mode == SystemMode::Contract)
        .map(generate_contract_system)
        .collect();

    let contract_registry = generate_systems_registry(spec);
    let router_code = generate_systems_router(spec);

    quote! {
        pub mod system_api {
            use super::*;

            #error_types
            #integration_code
            #event_code
            #(#generated_systems)*
            #(#contract_systems)*
            #contract_registry
            #router_code
        }
    }
}

// ── Contract system codegen ─────────────────────────────────────────────

fn generate_contract_system(system: &SystemDef) -> TokenStream {
    let name = &system.name;
    let snake = to_snake_case(name);

    let input_struct = generate_input_struct(system);
    let output_struct = generate_contract_output_struct(system);
    let error_enum = generate_contract_error_enum(system);
    let trait_def = generate_contract_trait(system);
    let executor_wrapper = generate_contract_executor(system);

    let _snake_ident = format_ident!("{}", snake);

    quote! {
        #input_struct
        #output_struct
        #error_enum
        #trait_def
        #executor_wrapper
    }
}

fn generate_contract_output_struct(system: &SystemDef) -> TokenStream {
    let name = format_ident!("{}Output", system.name);

    if system.output.is_empty() {
        return quote! {
            #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
            pub struct #name;
        };
    }

    let fields: Vec<TokenStream> = system
        .output
        .iter()
        .map(|f| {
            let fname = format_ident!("{}", f.name);
            let ftype = map_type(&f.ty);
            quote! { pub #fname: #ftype }
        })
        .collect();

    quote! {
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
        pub struct #name {
            #(#fields,)*
        }
    }
}

fn generate_contract_error_enum(system: &SystemDef) -> TokenStream {
    let enum_name = format_ident!("{}Error", system.name);

    let mut variants = Vec::new();
    let mut display_arms = Vec::new();
    let mut from_arms = Vec::new();

    for variant_str in &system.errors {
        let (name_str, has_payload) = parse_error_variant(variant_str);
        let variant_ident = format_ident!("{}", name_str);

        if has_payload {
            variants.push(quote! { #variant_ident(String) });
            display_arms.push(quote! {
                Self::#variant_ident(msg) => write!(f, "{}: {msg}", stringify!(#variant_ident))
            });
            from_arms.push(quote! {
                #enum_name::#variant_ident(msg) => SystemError::Domain(
                    format!("{}: {msg}", stringify!(#variant_ident))
                )
            });
        } else {
            variants.push(quote! { #variant_ident });
            display_arms.push(quote! {
                Self::#variant_ident => write!(f, stringify!(#variant_ident))
            });
            from_arms.push(quote! {
                #enum_name::#variant_ident => SystemError::Domain(
                    stringify!(#variant_ident).into()
                )
            });
        }
    }

    // Always include an Internal(String) variant
    variants.push(quote! { Internal(String) });
    display_arms.push(quote! {
        Self::Internal(msg) => write!(f, "internal: {msg}")
    });
    from_arms.push(quote! {
        #enum_name::Internal(msg) => SystemError::Domain(format!("internal: {msg}"))
    });

    quote! {
        #[derive(Debug)]
        pub enum #enum_name {
            #(#variants,)*
        }

        impl std::fmt::Display for #enum_name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    #(#display_arms,)*
                }
            }
        }

        impl From<#enum_name> for SystemError {
            fn from(e: #enum_name) -> Self {
                match e {
                    #(#from_arms,)*
                }
            }
        }
    }
}

fn generate_contract_trait(system: &SystemDef) -> TokenStream {
    let trait_name = format_ident!("{}System", system.name);
    let input_name = format_ident!("{}Input", system.name);
    let output_name = format_ident!("{}Output", system.name);
    let error_name = format_ident!("{}Error", system.name);

    quote! {
        #[async_trait::async_trait]
        pub trait #trait_name: Send + Sync {
            async fn execute(
                &self,
                pool: &sqlx::PgPool,
                input: #input_name,
            ) -> Result<#output_name, #error_name>;
        }
    }
}

fn generate_contract_executor(system: &SystemDef) -> TokenStream {
    let fn_name = format_ident!("execute_{}", to_snake_case(&system.name));
    let trait_name = format_ident!("{}System", system.name);
    let input_name = format_ident!("{}Input", system.name);
    let output_name = format_ident!("{}Output", system.name);

    quote! {
        pub async fn #fn_name<S: #trait_name>(
            pool: &sqlx::PgPool,
            system: &S,
            input: #input_name,
        ) -> Result<#output_name, SystemError> {
            <S as #trait_name>::execute(system, pool, input)
                .await
                .map_err(Into::into)
        }
    }
}

// ── Contract systems registry ───────────────────────────────────────────

fn generate_systems_registry(spec: &SystemsSpec) -> TokenStream {
    let contract_systems: Vec<&SystemDef> = spec
        .systems
        .iter()
        .filter(|s| s.mode == SystemMode::Contract)
        .collect();

    if contract_systems.is_empty() {
        return quote! {};
    }

    let trait_bounds: Vec<TokenStream> = contract_systems
        .iter()
        .map(|s| {
            let trait_name = format_ident!("{}System", s.name);
            quote! { #trait_name }
        })
        .collect();

    quote! {
        pub trait SystemsRegistry: #(#trait_bounds)+* + Send + Sync + Clone {}
        impl<T: #(#trait_bounds)+* + Send + Sync + Clone> SystemsRegistry for T {}
    }
}

// ── Router + handler generation ─────────────────────────────────────────

fn generate_systems_router(spec: &SystemsSpec) -> TokenStream {
    if spec.systems.is_empty() {
        return quote! {};
    }

    let has_integrations = !spec.integrations.is_empty();
    let has_contracts = spec.systems.iter().any(|s| s.mode == SystemMode::Contract);
    let any_uses_integrations = has_integrations
        && spec
            .systems
            .iter()
            .filter(|s| s.mode == SystemMode::Generated)
            .any(|s| steps_use_integration(&s.steps));

    let mut handler_fns = Vec::new();
    let mut route_registrations = Vec::new();

    for system in &spec.systems {
        let snake = to_snake_case(&system.name);
        let handler_name = format_ident!("handle_{}", snake);
        let executor_name = format_ident!("execute_{}", snake);
        let input_type = format_ident!("{}Input", system.name);
        let route_path = format!("/api/systems/{}", snake);

        match system.mode {
            SystemMode::Generated => {
                let result_type = format_ident!("{}Result", system.name);
                let uses_integrations = has_integrations && steps_use_integration(&system.steps);
                let uses_events = steps_use_events(&system.steps);

                let execute_args = {
                    let mut args = vec![quote! { &state.pool }];
                    if uses_integrations {
                        args.push(quote! { &state.integrations });
                    }
                    if uses_events {
                        args.push(quote! { &NoopEventBus });
                    }
                    args.push(quote! { input });
                    args
                };

                handler_fns.push(router_handler_fn(
                    &handler_name,
                    &input_type,
                    &result_type,
                    &executor_name,
                    &execute_args,
                    has_integrations,
                    has_contracts,
                ));
                route_registrations.push(router_route_registration(
                    &route_path,
                    &handler_name,
                    has_integrations,
                    has_contracts,
                ));
            }
            SystemMode::Contract => {
                let output_type = format_ident!("{}Output", system.name);
                let execute_args = vec![
                    quote! { &state.pool },
                    quote! { &state.systems },
                    quote! { input },
                ];

                handler_fns.push(router_handler_fn(
                    &handler_name,
                    &input_type,
                    &output_type,
                    &executor_name,
                    &execute_args,
                    has_integrations,
                    has_contracts,
                ));
                route_registrations.push(router_route_registration(
                    &route_path,
                    &handler_name,
                    has_integrations,
                    has_contracts,
                ));
            }
        }
    }

    generate_router_scaffold(
        has_integrations,
        has_contracts,
        any_uses_integrations,
        &handler_fns,
        &route_registrations,
    )
}

fn router_handler_fn(
    handler_name: &proc_macro2::Ident,
    input_type: &proc_macro2::Ident,
    result_type: &proc_macro2::Ident,
    executor_name: &proc_macro2::Ident,
    execute_args: &[TokenStream],
    has_integrations: bool,
    has_contracts: bool,
) -> TokenStream {
    let generics = router_generics(has_integrations, has_contracts);
    let state_type = router_state_type(has_integrations, has_contracts);

    quote! {
        async fn #handler_name #generics (
            axum::extract::State(state): axum::extract::State<#state_type>,
            axum::Json(input): axum::Json<#input_type>,
        ) -> Result<axum::Json<#result_type>, SystemError> {
            let result = #executor_name(#(#execute_args),*).await?;
            Ok(axum::Json(result))
        }
    }
}

fn router_route_registration(
    route_path: &str,
    handler_name: &proc_macro2::Ident,
    has_integrations: bool,
    has_contracts: bool,
) -> TokenStream {
    let turbofish = router_turbofish(has_integrations, has_contracts);
    quote! {
        .route(#route_path, axum::routing::post(#handler_name #turbofish))
    }
}

fn router_generics(has_integrations: bool, has_contracts: bool) -> TokenStream {
    match (has_integrations, has_contracts) {
        (true, true) => quote! { <I: IntegrationRegistry + 'static, S: SystemsRegistry + 'static> },
        (true, false) => quote! { <I: IntegrationRegistry + 'static> },
        (false, true) => quote! { <S: SystemsRegistry + 'static> },
        (false, false) => quote! {},
    }
}

fn router_state_type(has_integrations: bool, has_contracts: bool) -> TokenStream {
    match (has_integrations, has_contracts) {
        (true, true) => quote! { SystemsState<I, S> },
        (true, false) => quote! { SystemsState<I> },
        (false, true) => quote! { SystemsState<S> },
        (false, false) => quote! { SystemsState },
    }
}

fn router_turbofish(has_integrations: bool, has_contracts: bool) -> TokenStream {
    match (has_integrations, has_contracts) {
        (true, true) => quote! { ::<I, S> },
        (true, false) => quote! { ::<I> },
        (false, true) => quote! { ::<S> },
        (false, false) => quote! {},
    }
}

fn generate_router_scaffold(
    has_integrations: bool,
    has_contracts: bool,
    any_uses_integrations: bool,
    handler_fns: &[TokenStream],
    route_registrations: &[TokenStream],
) -> TokenStream {
    let (state_generics, state_fields, router_fn_generics, router_params, state_init) = match (
        has_integrations,
        has_contracts,
    ) {
        (true, true) => (
            quote! { <I: IntegrationRegistry, S: SystemsRegistry> },
            if any_uses_integrations {
                quote! { pub pool: sqlx::PgPool, pub integrations: I, pub systems: S, }
            } else {
                quote! { pub pool: sqlx::PgPool, _integrations: std::marker::PhantomData<I>, pub systems: S, }
            },
            quote! { <I: IntegrationRegistry + Clone + 'static, S: SystemsRegistry + 'static> },
            if any_uses_integrations {
                quote! { pool: sqlx::PgPool, integrations: I, systems: S }
            } else {
                quote! { pool: sqlx::PgPool, systems: S }
            },
            if any_uses_integrations {
                quote! { SystemsState { pool, integrations, systems } }
            } else {
                quote! { SystemsState { pool, _integrations: std::marker::PhantomData, systems } }
            },
        ),
        (true, false) => {
            let int_field = if any_uses_integrations {
                quote! { pub integrations: I, }
            } else {
                quote! { _integrations: std::marker::PhantomData<I>, }
            };
            let int_param = if any_uses_integrations {
                quote! { pool: sqlx::PgPool, integrations: I }
            } else {
                quote! { pool: sqlx::PgPool }
            };
            let int_init = if any_uses_integrations {
                quote! { SystemsState { pool, integrations } }
            } else {
                quote! { SystemsState { pool, _integrations: std::marker::PhantomData } }
            };
            (
                quote! { <I: IntegrationRegistry> },
                quote! { pub pool: sqlx::PgPool, #int_field },
                quote! { <I: IntegrationRegistry + Clone + 'static> },
                int_param,
                int_init,
            )
        }
        (false, true) => (
            quote! { <S: SystemsRegistry> },
            quote! { pub pool: sqlx::PgPool, pub systems: S, },
            quote! { <S: SystemsRegistry + 'static> },
            quote! { pool: sqlx::PgPool, systems: S },
            quote! { SystemsState { pool, systems } },
        ),
        (false, false) => (
            quote! {},
            quote! { pub pool: sqlx::PgPool, },
            quote! {},
            quote! { pool: sqlx::PgPool },
            quote! { SystemsState { pool } },
        ),
    };

    quote! {
        #[derive(Clone)]
        pub struct SystemsState #state_generics {
            #state_fields
        }

        pub fn router #router_fn_generics (
            #router_params,
        ) -> axum::Router {
            let state = #state_init;
            axum::Router::new()
                #(#route_registrations)*
                .with_state(state)
        }

        #(#handler_fns)*
    }
}

// ── Error types ─────────────────────────────────────────────────────────

fn generate_error_types() -> TokenStream {
    quote! {
        #[derive(Debug)]
        pub enum SystemError {
            NotFound(String),
            GuardFailed(String),
            Database(sqlx::Error),
            Integration(IntegrationError),
            Domain(String),
        }

        impl std::fmt::Display for SystemError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    Self::NotFound(m) => write!(f, "not found: {m}"),
                    Self::GuardFailed(m) => write!(f, "guard failed: {m}"),
                    Self::Database(e) => write!(f, "database error: {e}"),
                    Self::Integration(e) => write!(f, "integration error: {}.{}: {}", e.integration, e.operation, e.message),
                    Self::Domain(m) => write!(f, "domain error: {m}"),
                }
            }
        }

        impl axum::response::IntoResponse for SystemError {
            fn into_response(self) -> axum::response::Response {
                let (status, msg) = match &self {
                    Self::NotFound(m) => (axum::http::StatusCode::NOT_FOUND, m.clone()),
                    Self::GuardFailed(m) => (axum::http::StatusCode::UNPROCESSABLE_ENTITY, m.clone()),
                    Self::Database(e) => {
                        tracing::error!(error = %e, "system database error");
                        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "internal server error".to_string())
                    }
                    Self::Integration(e) => (
                        axum::http::StatusCode::BAD_GATEWAY,
                        format!("{}.{}: {}", e.integration, e.operation, e.message),
                    ),
                    Self::Domain(m) => (axum::http::StatusCode::UNPROCESSABLE_ENTITY, m.clone()),
                };
                (status, axum::Json(serde_json::json!({"error": msg}))).into_response()
            }
        }

        #[derive(Debug)]
        pub struct IntegrationError {
            pub integration: String,
            pub operation: String,
            pub message: String,
        }
    }
}

// ── Integration code generation ─────────────────────────────────────────

fn generate_all_integrations(spec: &SystemsSpec) -> TokenStream {
    if spec.integrations.is_empty() {
        return quote! {};
    }

    let mut io_structs = Vec::new();
    let mut traits = Vec::new();
    let trait_bounds: Vec<TokenStream> = spec
        .integrations
        .iter()
        .map(|i| {
            let trait_name = format_ident!("{}", to_pascal_case(&i.name));
            quote! { #trait_name }
        })
        .collect();

    for integration in &spec.integrations {
        let trait_name = format_ident!("{}", to_pascal_case(&integration.name));
        let mut methods = Vec::new();

        for op in &integration.operations {
            let composite = integration_op_pascal(&integration.name, &op.name);
            let input_name = format_ident!("{}Input", composite);
            let output_name = format_ident!("{}Output", composite);
            let method_name = format_ident!("{}_{}", integration.name, op.name);

            let input_fields: Vec<TokenStream> = op
                .input
                .iter()
                .map(|p| {
                    let name = format_ident!("{}", p.name);
                    let ty = map_type(&p.ty);
                    quote! { pub #name: #ty }
                })
                .collect();

            let output_fields: Vec<TokenStream> = op
                .output
                .iter()
                .map(|p| {
                    let name = format_ident!("{}", p.name);
                    let ty = map_type(&p.ty);
                    quote! { pub #name: #ty }
                })
                .collect();

            io_structs.push(quote! {
                #[derive(Debug, Clone)]
                pub struct #input_name {
                    #(#input_fields,)*
                }

                #[derive(Debug, Clone)]
                pub struct #output_name {
                    #(#output_fields,)*
                }
            });

            methods.push(quote! {
                async fn #method_name(
                    &self,
                    input: #input_name,
                ) -> Result<#output_name, IntegrationError>;
            });
        }

        traits.push(quote! {
            #[async_trait::async_trait]
            pub trait #trait_name: Send + Sync {
                #(#methods)*
            }
        });
    }

    quote! {
        #(#io_structs)*
        #(#traits)*

        pub trait IntegrationRegistry: #(#trait_bounds)+* + Send + Sync {}
        impl<T: #(#trait_bounds)+* + Send + Sync> IntegrationRegistry for T {}
    }
}

// ── Event code generation ───────────────────────────────────────────────

fn generate_all_events(spec: &SystemsSpec) -> TokenStream {
    let mut seen: HashMap<String, Vec<TokenStream>> = HashMap::new();
    let mut event_order: Vec<String> = Vec::new();

    for system in spec
        .systems
        .iter()
        .filter(|s| s.mode == SystemMode::Generated)
    {
        collect_events_from_steps(&system.steps, &mut seen, &mut event_order);
    }

    if event_order.is_empty() {
        return quote! {
            #[async_trait::async_trait]
            pub trait EventBus: Send + Sync {}

            pub struct NoopEventBus;
            #[async_trait::async_trait]
            impl EventBus for NoopEventBus {}
        };
    }

    let mut structs = Vec::new();
    let mut trait_methods = Vec::new();
    let mut noop_methods = Vec::new();

    for event_name in &event_order {
        let pascal = to_pascal_case(event_name);
        let struct_name = format_ident!("{}Event", pascal);
        let method_name = format_ident!("emit_{}", event_name);

        let fields = &seen[event_name];

        structs.push(quote! {
            #[derive(Debug, Clone, serde::Serialize)]
            pub struct #struct_name {
                #(#fields,)*
            }
        });

        trait_methods.push(quote! {
            async fn #method_name(&self, event: #struct_name);
        });

        noop_methods.push(quote! {
            async fn #method_name(&self, _event: #struct_name) {}
        });
    }

    quote! {
        #(#structs)*

        #[async_trait::async_trait]
        pub trait EventBus: Send + Sync {
            #(#trait_methods)*
        }

        pub struct NoopEventBus;
        #[async_trait::async_trait]
        impl EventBus for NoopEventBus {
            #(#noop_methods)*
        }
    }
}

fn collect_events_from_steps(
    steps: &[Step],
    seen: &mut HashMap<String, Vec<TokenStream>>,
    order: &mut Vec<String>,
) {
    for step in steps {
        match step {
            Step::EmitEvent(s) => {
                if !seen.contains_key(&s.event) {
                    let fields: Vec<TokenStream> = s
                        .payload
                        .iter()
                        .map(|p| {
                            let name = format_ident!("{}", p.field);
                            // Event payload field types are inferred as String
                            // for simplicity; the Rust compiler ensures correctness
                            // because the generated code passes the actual binding values.
                            // We use serde_json::Value as a universal carrier.
                            quote! { pub #name: serde_json::Value }
                        })
                        .collect();
                    seen.insert(s.event.clone(), fields);
                    order.push(s.event.clone());
                }
            }
            Step::Branch(s) => {
                collect_events_from_steps(&s.then, seen, order);
                if let Some(ref else_steps) = s.otherwise {
                    collect_events_from_steps(else_steps, seen, order);
                }
            }
            _ => {}
        }
    }
}

// ── System code generation ──────────────────────────────────────────────

fn generate_system(system: &SystemDef, spec: &SystemsSpec) -> TokenStream {
    let input_struct = generate_input_struct(system);
    let (executor_body, bindings) = generate_executor_body(system, spec);
    let result_struct = generate_result_struct(system, &bindings);
    let executor_fn = generate_executor_fn(system, &executor_body, spec);

    quote! {
        #input_struct
        #result_struct
        #executor_fn
    }
}

fn generate_input_struct(system: &SystemDef) -> TokenStream {
    let name = format_ident!("{}Input", system.name);
    let fields: Vec<TokenStream> = system
        .input
        .iter()
        .map(|f| {
            let fname = format_ident!("{}", f.name);
            let ftype = map_type(&f.ty);
            if f.required {
                quote! { pub #fname: #ftype }
            } else {
                quote! { pub #fname: Option<#ftype> }
            }
        })
        .collect();

    quote! {
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
        pub struct #name {
            #(#fields,)*
        }
    }
}

fn generate_result_struct(
    system: &SystemDef,
    bindings: &HashMap<String, BindingKind>,
) -> TokenStream {
    let name = format_ident!("{}Result", system.name);

    if system.result.is_empty() {
        return quote! {
            #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
            pub struct #name;
        };
    }

    let fields: Vec<TokenStream> = system
        .result
        .iter()
        .map(|r| {
            let fname = format_ident!("{}", r.name);
            match bindings.get(&r.from) {
                Some(BindingKind::Entity(entity)) => {
                    let ty = format_ident!("{}", entity);
                    quote! { pub #fname: #ty }
                }
                Some(BindingKind::EntityVec(entity)) => {
                    let ty = format_ident!("{}", entity);
                    quote! { pub #fname: Vec<#ty> }
                }
                Some(BindingKind::IntegrationOutput(integration, operation)) => {
                    let ty =
                        format_ident!("{}Output", integration_op_pascal(integration, operation));
                    quote! { pub #fname: #ty }
                }
                None => {
                    let msg = format!(
                        "result field '{}' references untracked binding '{}'",
                        r.name, r.from
                    );
                    quote! { compile_error!(#msg); }
                }
            }
        })
        .collect();

    quote! {
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
        pub struct #name {
            #(#fields,)*
        }
    }
}

fn generate_executor_fn(
    system: &SystemDef,
    body: &[TokenStream],
    _spec: &SystemsSpec,
) -> TokenStream {
    let fn_name = format_ident!("execute_{}", to_snake_case(&system.name));
    let input_name = format_ident!("{}Input", system.name);
    let result_name = format_ident!("{}Result", system.name);

    let result_fields: Vec<TokenStream> = system
        .result
        .iter()
        .map(|r| {
            let name = format_ident!("{}", r.name);
            let from = format_ident!("{}", r.from);
            quote! { #name: #from }
        })
        .collect();

    let result_expr = if system.result.is_empty() {
        quote! { #result_name }
    } else {
        quote! {
            #result_name {
                #(#result_fields,)*
            }
        }
    };

    let has_integrations = steps_use_integration(&system.steps);
    let has_events = steps_use_events(&system.steps);

    let (i_bound, i_param, _i_arg) = if has_integrations {
        (
            quote! { I: IntegrationRegistry, },
            quote! { integrations: &I, },
            quote! { integrations, },
        )
    } else {
        (quote! {}, quote! {}, quote! {})
    };

    let (e_bound, e_param, _e_arg) = if has_events {
        (
            quote! { E: EventBus, },
            quote! { events: &E, },
            quote! { events, },
        )
    } else {
        (quote! {}, quote! {}, quote! {})
    };

    quote! {
        #[allow(unused_variables, unused_mut)]
        pub async fn #fn_name<#i_bound #e_bound>(
            pool: &sqlx::PgPool,
            #i_param
            #e_param
            input: #input_name,
        ) -> Result<#result_name, SystemError> {
            #(#body)*

            Ok(#result_expr)
        }
    }
}

// ── Step introspection ──────────────────────────────────────────────────

fn steps_use_integration(steps: &[Step]) -> bool {
    steps.iter().any(|s| match s {
        Step::CallIntegration(_) => true,
        Step::Branch(b) => {
            steps_use_integration(&b.then)
                || b.otherwise
                    .as_ref()
                    .is_some_and(|e| steps_use_integration(e))
        }
        _ => false,
    })
}

fn steps_use_events(steps: &[Step]) -> bool {
    steps.iter().any(|s| match s {
        Step::EmitEvent(_) => true,
        Step::Branch(b) => {
            steps_use_events(&b.then) || b.otherwise.as_ref().is_some_and(|e| steps_use_events(e))
        }
        _ => false,
    })
}

// ── Step codegen ────────────────────────────────────────────────────────

fn generate_executor_body(
    system: &SystemDef,
    spec: &SystemsSpec,
) -> (Vec<TokenStream>, HashMap<String, BindingKind>) {
    let mut bindings: HashMap<String, BindingKind> = HashMap::new();
    let tokens = generate_steps(&system.steps, spec, &mut bindings);
    (tokens, bindings)
}

fn generate_steps(
    steps: &[Step],
    spec: &SystemsSpec,
    bindings: &mut HashMap<String, BindingKind>,
) -> Vec<TokenStream> {
    steps
        .iter()
        .map(|step| generate_step(step, spec, bindings))
        .collect()
}

fn generate_step(
    step: &Step,
    spec: &SystemsSpec,
    bindings: &mut HashMap<String, BindingKind>,
) -> TokenStream {
    match step {
        Step::LoadOne(s) => generate_load_one(s, bindings),
        Step::LoadMany(s) => generate_load_many(s, bindings),
        Step::Create(s) => generate_create(s, bindings),
        Step::Update(s) => generate_update(s, bindings),
        Step::Delete(s) => generate_delete(s),
        Step::Guard(s) => generate_guard(s),
        Step::Branch(s) => generate_branch(s, spec, bindings),
        Step::CallIntegration(s) => generate_call_integration(s, bindings),
        Step::EmitEvent(s) => generate_emit_event(s),
    }
}

fn generate_load_one(
    step: &LoadOneStep,
    bindings: &mut HashMap<String, BindingKind>,
) -> TokenStream {
    let binding = format_ident!("{}", step.binding);
    let repo = format_ident!("Sqlx{}Repository", step.entity);
    let by_tokens = path_to_cloned_tokens(&step.by);
    let not_found = &step.not_found;

    bindings.insert(
        step.binding.clone(),
        BindingKind::Entity(step.entity.clone()),
    );

    quote! {
        let #binding = {
            let repo = #repo::new(pool.clone());
            repo.find_by_id(#by_tokens)
                .await
                .map_err(SystemError::Database)?
                .ok_or_else(|| SystemError::NotFound(#not_found.into()))?
        };
    }
}

fn generate_load_many(
    step: &LoadManyStep,
    bindings: &mut HashMap<String, BindingKind>,
) -> TokenStream {
    let binding = format_ident!("{}", step.binding);
    let repo = format_ident!("Sqlx{}Repository", step.entity);
    let filter_field = &step.filter.field;
    let filter_from = path_to_cloned_tokens(&step.filter.from);

    bindings.insert(
        step.binding.clone(),
        BindingKind::EntityVec(step.entity.clone()),
    );

    quote! {
        let #binding = {
            let repo = #repo::new(pool.clone());
            let mut filters = std::collections::HashMap::new();
            filters.insert(#filter_field.to_string(), #filter_from.to_string());
            let (items, _) = repo
                .list_filtered(&filters, None, None, i64::MAX, 0)
                .await
                .map_err(SystemError::Database)?;
            items
        };
    }
}

fn generate_create(step: &CreateStep, bindings: &mut HashMap<String, BindingKind>) -> TokenStream {
    let entity = &step.entity;
    let create_type = format_ident!("Create{}", entity);
    let repo = format_ident!("Sqlx{}Repository", entity);

    let set_fields: Vec<TokenStream> = step
        .set
        .iter()
        .map(|m| {
            let field = format_ident!("{}", m.field);
            match (&m.from, &m.value) {
                (Some(path), _) => {
                    let val = path_to_cloned_tokens(path);
                    quote! { #field: #val }
                }
                (_, Some(val)) if val.is_null() => {
                    quote! { #field: None }
                }
                (_, Some(val)) => {
                    let val = yaml_value_to_assignment_tokens(val);
                    quote! { #field: #val }
                }
                _ => unreachable!(),
            }
        })
        .collect();

    if let Some(ref b) = step.binding {
        let binding = format_ident!("{}", b);
        bindings.insert(b.clone(), BindingKind::Entity(entity.clone()));

        quote! {
            let #binding = {
                let repo = #repo::new(pool.clone());
                repo.create(#create_type {
                    #(#set_fields,)*
                }).await.map_err(SystemError::Database)?
            };
        }
    } else {
        quote! {
            {
                let repo = #repo::new(pool.clone());
                repo.create(#create_type {
                    #(#set_fields,)*
                }).await.map_err(SystemError::Database)?;
            }
        }
    }
}

fn generate_update(step: &UpdateStep, bindings: &mut HashMap<String, BindingKind>) -> TokenStream {
    let entity = &step.entity;
    let update_type = format_ident!("Update{}", entity);
    let repo = format_ident!("Sqlx{}Repository", entity);
    let target_tokens = path_to_cloned_tokens(&step.target);

    let parts: Vec<&str> = step.target.split('.').collect();
    let binding_name = format_ident!("{}", parts[0]);

    bindings.insert(parts[0].to_string(), BindingKind::Entity(entity.clone()));

    let set_fields: Vec<TokenStream> = step
        .set
        .iter()
        .map(|m| {
            let field = format_ident!("{}", m.field);
            match (&m.from, &m.value) {
                (Some(path), _) => {
                    let val = path_to_cloned_tokens(path);
                    quote! { #field: Some(#val) }
                }
                (_, Some(val)) if val.is_null() => {
                    quote! { #field: None }
                }
                (_, Some(val)) => {
                    let val = yaml_value_to_assignment_tokens(val);
                    quote! { #field: Some(#val) }
                }
                _ => unreachable!(),
            }
        })
        .collect();

    quote! {
        let #binding_name = {
            let repo = #repo::new(pool.clone());
            repo.update(#target_tokens, #update_type {
                #(#set_fields,)*
                ..Default::default()
            }).await.map_err(SystemError::Database)?
                .ok_or_else(|| SystemError::NotFound(
                    concat!(stringify!(#binding_name), " not found after update").into()
                ))?
        };
    }
}

fn generate_delete(step: &DeleteStep) -> TokenStream {
    let entity = &step.entity;
    let repo = format_ident!("Sqlx{}Repository", entity);
    let target_tokens = path_to_cloned_tokens(&step.target);
    let err_msg = format!("{} not found for deletion", entity);

    quote! {
        {
            let repo = #repo::new(pool.clone());
            if !repo.delete(#target_tokens).await.map_err(SystemError::Database)? {
                return Err(SystemError::NotFound(#err_msg.into()));
            }
        }
    }
}

fn generate_guard(step: &GuardStep) -> TokenStream {
    let condition = generate_condition(&step.check);
    let error_msg = &step.error;

    quote! {
        if !(#condition) {
            return Err(SystemError::GuardFailed(#error_msg.into()));
        }
    }
}

fn generate_branch(
    step: &BranchStep,
    spec: &SystemsSpec,
    bindings: &mut HashMap<String, BindingKind>,
) -> TokenStream {
    let condition = generate_condition(&step.check);
    let mut branch_bindings = bindings.clone();
    let then_tokens = generate_steps(&step.then, spec, &mut branch_bindings);

    if let Some(ref else_steps) = step.otherwise {
        let mut else_bindings = bindings.clone();
        let else_tokens = generate_steps(else_steps, spec, &mut else_bindings);
        quote! {
            if #condition {
                #(#then_tokens)*
            } else {
                #(#else_tokens)*
            }
        }
    } else {
        quote! {
            if #condition {
                #(#then_tokens)*
            }
        }
    }
}

fn generate_call_integration(
    step: &CallIntegrationStep,
    bindings: &mut HashMap<String, BindingKind>,
) -> TokenStream {
    let binding = format_ident!("{}", step.binding);
    let method = format_ident!("{}_{}", step.integration, step.operation);
    let composite = integration_op_pascal(&step.integration, &step.operation);
    let input_type = format_ident!("{}Input", composite);

    bindings.insert(
        step.binding.clone(),
        BindingKind::IntegrationOutput(step.integration.clone(), step.operation.clone()),
    );

    let params: Vec<TokenStream> = step
        .input
        .iter()
        .map(|inp| {
            let param = format_ident!("{}", inp.param);
            match (&inp.from, &inp.value) {
                (Some(path), _) => {
                    let val = path_to_cloned_tokens(path);
                    quote! { #param: #val }
                }
                (_, Some(val)) => {
                    let val = yaml_value_to_assignment_tokens(val);
                    quote! { #param: #val }
                }
                _ => unreachable!(),
            }
        })
        .collect();

    quote! {
        let #binding = integrations.#method(
            #input_type {
                #(#params,)*
            }
        ).await.map_err(SystemError::Integration)?;
    }
}

fn generate_emit_event(step: &EmitEventStep) -> TokenStream {
    let pascal = to_pascal_case(&step.event);
    let struct_name = format_ident!("{}Event", pascal);
    let method = format_ident!("emit_{}", step.event);

    let fields: Vec<TokenStream> = step
        .payload
        .iter()
        .map(|p| {
            let field = format_ident!("{}", p.field);
            let from = path_to_cloned_tokens(&p.from);
            quote! { #field: serde_json::json!(#from) }
        })
        .collect();

    quote! {
        events.#method(#struct_name {
            #(#fields,)*
        }).await;
    }
}

// ── Condition codegen ───────────────────────────────────────────────────

fn generate_condition(cond: &Condition) -> TokenStream {
    let field_tokens = path_to_tokens(&cond.field);

    if let Some(ref val) = cond.equals {
        let val_tokens = yaml_value_to_comparison_tokens(val);
        quote! { #field_tokens == #val_tokens }
    } else if let Some(ref val) = cond.not_equals {
        let val_tokens = yaml_value_to_comparison_tokens(val);
        quote! { #field_tokens != #val_tokens }
    } else if let Some(ref path) = cond.equals_field {
        let other = path_to_tokens(path);
        quote! { #field_tokens == #other }
    } else if let Some(ref path) = cond.not_equals_field {
        let other = path_to_tokens(path);
        quote! { #field_tokens != #other }
    } else {
        unreachable!("validation should have caught missing condition comparator")
    }
}
