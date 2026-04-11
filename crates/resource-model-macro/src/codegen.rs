use crate::spec::{EntitySpec, RelationSpec, Spec};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

fn map_type(ty: &str) -> TokenStream {
    match ty {
        "uuid" => quote! { uuid::Uuid },
        "string" | "text" => quote! { String },
        "int" => quote! { i32 },
        "bigint" => quote! { i64 },
        "float" => quote! { f64 },
        "bool" => quote! { bool },
        _ => unreachable!("unsupported type '{}' should have been caught by validation", ty),
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

pub fn generate(spec: &Spec) -> TokenStream {
    let crud_trait = generate_crud_trait();
    let migrate_fn = generate_migrate(spec);
    let api = spec.config.api;

    let entities: Vec<TokenStream> = spec
        .entities
        .iter()
        .map(|entity| {
            let relations: Vec<&RelationSpec> = spec
                .relations
                .iter()
                .filter(|r| r.source == entity.name)
                .collect();
            generate_entity(entity, &relations, spec, api)
        })
        .collect();

    let api_module = if api {
        generate_api(spec)
    } else {
        quote! {}
    };

    quote! {
        #crud_trait
        #migrate_fn
        #(#entities)*
        #api_module
    }
}

fn map_sql_type(ty: &str) -> &'static str {
    match ty {
        "uuid" => "UUID",
        "string" | "text" => "TEXT",
        "int" => "INTEGER",
        "bigint" => "BIGINT",
        "float" => "DOUBLE PRECISION",
        "bool" => "BOOLEAN",
        _ => unreachable!(),
    }
}

fn generate_migrate(spec: &Spec) -> TokenStream {
    let drop_stmts: Vec<String> = spec
        .entities
        .iter()
        .rev()
        .map(|e| format!("DROP TABLE IF EXISTS {} CASCADE", e.table))
        .collect();

    let create_stmts: Vec<String> = spec
        .entities
        .iter()
        .map(|entity| {
            let mut cols = Vec::new();

            cols.push(format!(
                "{} {} PRIMARY KEY",
                entity.id.name,
                map_sql_type(&entity.id.ty)
            ));

            for f in &entity.fields {
                let mut col = format!("{} {}", f.name, map_sql_type(&f.ty));
                if f.required {
                    col.push_str(" NOT NULL");
                }
                if f.unique {
                    col.push_str(" UNIQUE");
                }
                if let Some(ref refs) = f.references {
                    let target = spec
                        .entities
                        .iter()
                        .find(|e| e.name == refs.entity)
                        .unwrap();
                    col.push_str(&format!(" REFERENCES {}({})", target.table, refs.field));
                }
                cols.push(col);
            }

            format!(
                "CREATE TABLE {} (\n  {}\n)",
                entity.table,
                cols.join(",\n  ")
            )
        })
        .collect();

    let all_sql: Vec<&String> = drop_stmts.iter().chain(create_stmts.iter()).collect();

    let exec_calls: Vec<TokenStream> = all_sql
        .iter()
        .map(|sql| {
            quote! {
                sqlx::query(#sql).execute(pool).await?;
            }
        })
        .collect();

    quote! {
        pub async fn migrate(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
            #(#exec_calls)*
            Ok(())
        }
    }
}

fn generate_crud_trait() -> TokenStream {
    quote! {
        #[async_trait::async_trait]
        pub trait CrudRepository: Send + Sync {
            type Entity: Send + Sync;
            type Create: Send + Sync;
            type Update: Send + Sync;

            async fn create(&self, input: Self::Create) -> Result<Self::Entity, sqlx::Error>;
            async fn find_by_id(&self, id: uuid::Uuid) -> Result<Option<Self::Entity>, sqlx::Error>;
            async fn list(&self) -> Result<Vec<Self::Entity>, sqlx::Error>;
            async fn update(&self, id: uuid::Uuid, input: Self::Update) -> Result<Option<Self::Entity>, sqlx::Error>;
            async fn delete(&self, id: uuid::Uuid) -> Result<bool, sqlx::Error>;
        }
    }
}

fn generate_entity(
    entity: &EntitySpec,
    relations: &[&RelationSpec],
    spec: &Spec,
    api: bool,
) -> TokenStream {
    let name = format_ident!("{}", entity.name);
    let create_name = format_ident!("Create{}", entity.name);
    let update_name = format_ident!("Update{}", entity.name);
    let repo_trait_name = format_ident!("{}Repository", entity.name);
    let repo_struct_name = format_ident!("Sqlx{}Repository", entity.name);
    let table = &entity.table;

    let id_ident = format_ident!("{}", entity.id.name);
    let id_type = map_type(&entity.id.ty);

    // ── struct fields ──────────────────────────────────────────────────
    let entity_fields: Vec<TokenStream> = std::iter::once(quote! { pub #id_ident: #id_type })
        .chain(entity.fields.iter().map(|f| {
            let fname = format_ident!("{}", f.name);
            let ftype = map_type(&f.ty);
            if f.required {
                quote! { pub #fname: #ftype }
            } else {
                quote! { pub #fname: Option<#ftype> }
            }
        }))
        .collect();

    let create_fields: Vec<TokenStream> = entity
        .fields
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

    let update_fields: Vec<TokenStream> = entity
        .fields
        .iter()
        .map(|f| {
            let fname = format_ident!("{}", f.name);
            let ftype = map_type(&f.ty);
            quote! { pub #fname: Option<#ftype> }
        })
        .collect();

    // ── derives (conditionally include ToSchema) ───────────────────────
    let entity_derive = if api {
        quote! { #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow, utoipa::ToSchema)] }
    } else {
        quote! { #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)] }
    };

    let create_derive = if api {
        quote! { #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)] }
    } else {
        quote! { #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)] }
    };

    let update_derive = if api {
        quote! { #[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, utoipa::ToSchema)] }
    } else {
        quote! { #[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)] }
    };

    // ── SQL strings ────────────────────────────────────────────────────
    let all_col_names: Vec<&str> = std::iter::once(entity.id.name.as_str())
        .chain(entity.fields.iter().map(|f| f.name.as_str()))
        .collect();
    let col_list = all_col_names.join(", ");
    let placeholders: String = (1..=all_col_names.len())
        .map(|i| format!("${i}"))
        .collect::<Vec<_>>()
        .join(", ");

    let insert_sql = format!(
        "INSERT INTO {table} ({col_list}) VALUES ({placeholders}) RETURNING {col_list}"
    );

    let select_one_sql = format!(
        "SELECT {col_list} FROM {table} WHERE {} = $1",
        entity.id.name
    );

    let select_all_sql = format!(
        "SELECT {col_list} FROM {table} ORDER BY {}",
        entity.id.name
    );

    let set_clauses: Vec<String> = entity
        .fields
        .iter()
        .enumerate()
        .map(|(i, f)| format!("{name} = COALESCE(${p}, {name})", name = f.name, p = i + 2))
        .collect();
    let update_sql = format!(
        "UPDATE {table} SET {} WHERE {} = $1 RETURNING {col_list}",
        set_clauses.join(", "),
        entity.id.name
    );

    let delete_sql = format!("DELETE FROM {table} WHERE {} = $1", entity.id.name);

    // ── bind chains ────────────────────────────────────────────────────
    let insert_binds: Vec<TokenStream> = entity
        .fields
        .iter()
        .map(|f| {
            let fname = format_ident!("{}", f.name);
            quote! { .bind(&input.#fname) }
        })
        .collect();

    let update_binds: Vec<TokenStream> = entity
        .fields
        .iter()
        .map(|f| {
            let fname = format_ident!("{}", f.name);
            quote! { .bind(&input.#fname) }
        })
        .collect();

    // ── relation methods ───────────────────────────────────────────────
    let (rel_trait_methods, rel_impl_methods) = generate_relation_methods(relations, spec);

    quote! {
        #entity_derive
        pub struct #name {
            #(#entity_fields,)*
        }

        #create_derive
        pub struct #create_name {
            #(#create_fields,)*
        }

        #update_derive
        pub struct #update_name {
            #(#update_fields,)*
        }

        #[async_trait::async_trait]
        pub trait #repo_trait_name:
            CrudRepository<Entity = #name, Create = #create_name, Update = #update_name>
        {
            #(#rel_trait_methods)*
        }

        #[derive(Clone)]
        pub struct #repo_struct_name {
            pool: sqlx::PgPool,
        }

        impl #repo_struct_name {
            pub fn new(pool: sqlx::PgPool) -> Self {
                Self { pool }
            }
        }

        #[async_trait::async_trait]
        impl CrudRepository for #repo_struct_name {
            type Entity = #name;
            type Create = #create_name;
            type Update = #update_name;

            async fn create(&self, input: Self::Create) -> Result<Self::Entity, sqlx::Error> {
                sqlx::query_as::<_, #name>(#insert_sql)
                    .bind(uuid::Uuid::new_v4())
                    #(#insert_binds)*
                    .fetch_one(&self.pool)
                    .await
            }

            async fn find_by_id(&self, id: uuid::Uuid) -> Result<Option<Self::Entity>, sqlx::Error> {
                sqlx::query_as::<_, #name>(#select_one_sql)
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await
            }

            async fn list(&self) -> Result<Vec<Self::Entity>, sqlx::Error> {
                sqlx::query_as::<_, #name>(#select_all_sql)
                    .fetch_all(&self.pool)
                    .await
            }

            async fn update(&self, id: uuid::Uuid, input: Self::Update) -> Result<Option<Self::Entity>, sqlx::Error> {
                sqlx::query_as::<_, #name>(#update_sql)
                    .bind(id)
                    #(#update_binds)*
                    .fetch_optional(&self.pool)
                    .await
            }

            async fn delete(&self, id: uuid::Uuid) -> Result<bool, sqlx::Error> {
                let result = sqlx::query(#delete_sql)
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
                Ok(result.rows_affected() > 0)
            }
        }

        #[async_trait::async_trait]
        impl #repo_trait_name for #repo_struct_name {
            #(#rel_impl_methods)*
        }
    }
}

fn generate_relation_methods(
    relations: &[&RelationSpec],
    spec: &Spec,
) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let mut trait_methods = Vec::new();
    let mut impl_methods = Vec::new();

    for rel in relations {
        let method = format_ident!("{}", rel.name);
        let target_entity = spec.entities.iter().find(|e| e.name == rel.target).unwrap();
        let target_type = format_ident!("{}", rel.target);
        let fk_param = format_ident!("{}", rel.foreign_key);

        let target_col_list: String = std::iter::once(target_entity.id.name.as_str())
            .chain(target_entity.fields.iter().map(|f| f.name.as_str()))
            .collect::<Vec<_>>()
            .join(", ");

        match rel.kind.as_str() {
            "has_many" => {
                let sql = format!(
                    "SELECT {target_col_list} FROM {} WHERE {} = $1 ORDER BY {}",
                    target_entity.table, rel.foreign_key, target_entity.id.name
                );

                trait_methods.push(quote! {
                    async fn #method(&self, #fk_param: uuid::Uuid)
                        -> Result<Vec<#target_type>, sqlx::Error>;
                });

                impl_methods.push(quote! {
                    async fn #method(&self, #fk_param: uuid::Uuid)
                        -> Result<Vec<#target_type>, sqlx::Error>
                    {
                        sqlx::query_as::<_, #target_type>(#sql)
                            .bind(#fk_param)
                            .fetch_all(&self.pool)
                            .await
                    }
                });
            }
            "belongs_to" => {
                let sql = format!(
                    "SELECT {target_col_list} FROM {} WHERE {} = $1",
                    target_entity.table, target_entity.id.name
                );

                trait_methods.push(quote! {
                    async fn #method(&self, #fk_param: uuid::Uuid)
                        -> Result<Option<#target_type>, sqlx::Error>;
                });

                impl_methods.push(quote! {
                    async fn #method(&self, #fk_param: uuid::Uuid)
                        -> Result<Option<#target_type>, sqlx::Error>
                    {
                        sqlx::query_as::<_, #target_type>(#sql)
                            .bind(#fk_param)
                            .fetch_optional(&self.pool)
                            .await
                    }
                });
            }
            _ => {}
        }
    }

    (trait_methods, impl_methods)
}

// ── API generation (when config.api = true) ────────────────────────────

fn generate_api(spec: &Spec) -> TokenStream {
    let error_type = generate_api_error();

    let mut all_handlers = Vec::new();
    let mut route_registrations = Vec::new();

    for entity in &spec.entities {
        let has_many_rels: Vec<&RelationSpec> = spec
            .relations
            .iter()
            .filter(|r| r.source == entity.name && r.kind == "has_many")
            .collect();

        let (handlers, routes) = generate_entity_api(entity, &has_many_rels, spec);
        all_handlers.push(handlers);
        route_registrations.extend(routes);
    }

    quote! {
        pub mod resource_api {
            use super::*;

            #error_type

            #(#all_handlers)*

            pub fn router() -> utoipa_axum::router::OpenApiRouter<sqlx::PgPool> {
                utoipa_axum::router::OpenApiRouter::new()
                    #(#route_registrations)*
            }
        }
    }
}

fn generate_entity_api(
    entity: &EntitySpec,
    has_many_relations: &[&RelationSpec],
    _spec: &Spec,
) -> (TokenStream, Vec<TokenStream>) {
    let name = format_ident!("{}", entity.name);
    let create_name = format_ident!("Create{}", entity.name);
    let update_name = format_ident!("Update{}", entity.name);
    let repo_struct = format_ident!("Sqlx{}Repository", entity.name);
    let table = &entity.table;
    let entity_lower = to_snake_case(&entity.name);

    let list_fn = format_ident!("list_{}", table);
    let create_fn = format_ident!("create_{}", entity_lower);
    let get_fn = format_ident!("get_{}", entity_lower);
    let update_fn = format_ident!("update_{}", entity_lower);
    let delete_fn = format_ident!("delete_{}", entity_lower);

    let api_path = format!("/api/{}", table);
    let api_path_id = format!("/api/{}/{{id}}", table);
    let tag = table.to_string();

    let list_desc = format!("List all {}", table);
    let create_desc = format!("Create {}", entity_lower);
    let get_desc = format!("Get {} by ID", entity_lower);
    let update_desc = format!("Update {}", entity_lower);
    let delete_desc = format!("Delete {}", entity_lower);

    let crud_handlers = quote! {
        #[utoipa::path(
            get,
            path = #api_path,
            responses((status = 200, description = #list_desc, body = Vec<#name>)),
            tag = #tag
        )]
        pub async fn #list_fn(
            axum::extract::State(pool): axum::extract::State<sqlx::PgPool>,
        ) -> Result<axum::Json<Vec<#name>>, ApiError> {
            let repo = #repo_struct::new(pool);
            repo.list().await.map(axum::Json).map_err(ApiError::Internal)
        }

        #[utoipa::path(
            post,
            path = #api_path,
            request_body = #create_name,
            responses((status = 201, description = #create_desc, body = #name)),
            tag = #tag
        )]
        pub async fn #create_fn(
            axum::extract::State(pool): axum::extract::State<sqlx::PgPool>,
            axum::Json(input): axum::Json<#create_name>,
        ) -> Result<(axum::http::StatusCode, axum::Json<#name>), ApiError> {
            let repo = #repo_struct::new(pool);
            repo.create(input)
                .await
                .map(|e| (axum::http::StatusCode::CREATED, axum::Json(e)))
                .map_err(ApiError::Internal)
        }

        #[utoipa::path(
            get,
            path = #api_path_id,
            params(("id" = uuid::Uuid, Path, description = "Record ID")),
            responses(
                (status = 200, description = #get_desc, body = #name),
                (status = 404, description = "Not found")
            ),
            tag = #tag
        )]
        pub async fn #get_fn(
            axum::extract::State(pool): axum::extract::State<sqlx::PgPool>,
            axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
        ) -> Result<axum::Json<#name>, ApiError> {
            let repo = #repo_struct::new(pool);
            repo.find_by_id(id)
                .await
                .map_err(ApiError::Internal)?
                .map(axum::Json)
                .ok_or(ApiError::NotFound)
        }

        #[utoipa::path(
            put,
            path = #api_path_id,
            params(("id" = uuid::Uuid, Path, description = "Record ID")),
            request_body = #update_name,
            responses(
                (status = 200, description = #update_desc, body = #name),
                (status = 404, description = "Not found")
            ),
            tag = #tag
        )]
        pub async fn #update_fn(
            axum::extract::State(pool): axum::extract::State<sqlx::PgPool>,
            axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
            axum::Json(input): axum::Json<#update_name>,
        ) -> Result<axum::Json<#name>, ApiError> {
            let repo = #repo_struct::new(pool);
            repo.update(id, input)
                .await
                .map_err(ApiError::Internal)?
                .map(axum::Json)
                .ok_or(ApiError::NotFound)
        }

        #[utoipa::path(
            delete,
            path = #api_path_id,
            params(("id" = uuid::Uuid, Path, description = "Record ID")),
            responses(
                (status = 204, description = #delete_desc),
                (status = 404, description = "Not found")
            ),
            tag = #tag
        )]
        pub async fn #delete_fn(
            axum::extract::State(pool): axum::extract::State<sqlx::PgPool>,
            axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
        ) -> Result<axum::http::StatusCode, ApiError> {
            let repo = #repo_struct::new(pool);
            if repo.delete(id).await.map_err(ApiError::Internal)? {
                Ok(axum::http::StatusCode::NO_CONTENT)
            } else {
                Err(ApiError::NotFound)
            }
        }
    };

    let mut routes = vec![
        quote! { .routes(utoipa_axum::routes!(#list_fn, #create_fn)) },
        quote! { .routes(utoipa_axum::routes!(#get_fn, #update_fn, #delete_fn)) },
    ];

    let mut rel_handlers = Vec::new();
    for rel in has_many_relations {
        let rel_fn_name = format_ident!("get_{}_{}", entity_lower, rel.name);
        let target_type = format_ident!("{}", rel.target);
        let rel_method = format_ident!("{}", rel.name);
        let rel_path = format!("/api/{}/{{id}}/{}", table, rel.name);
        let rel_desc = format!("Get {} for {}", rel.name, entity_lower);

        rel_handlers.push(quote! {
            #[utoipa::path(
                get,
                path = #rel_path,
                params(("id" = uuid::Uuid, Path, description = "Parent record ID")),
                responses((status = 200, description = #rel_desc, body = Vec<#target_type>)),
                tag = #tag
            )]
            pub async fn #rel_fn_name(
                axum::extract::State(pool): axum::extract::State<sqlx::PgPool>,
                axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
            ) -> Result<axum::Json<Vec<#target_type>>, ApiError> {
                let repo = #repo_struct::new(pool);
                repo.#rel_method(id).await.map(axum::Json).map_err(ApiError::Internal)
            }
        });

        routes.push(quote! { .routes(utoipa_axum::routes!(#rel_fn_name)) });
    }

    let all = quote! {
        #crud_handlers
        #(#rel_handlers)*
    };

    (all, routes)
}

fn generate_api_error() -> TokenStream {
    quote! {
        pub enum ApiError {
            NotFound,
            Internal(sqlx::Error),
        }

        impl axum::response::IntoResponse for ApiError {
            fn into_response(self) -> axum::response::Response {
                match self {
                    Self::NotFound => (
                        axum::http::StatusCode::NOT_FOUND,
                        axum::Json(serde_json::json!({"error": "not found"})),
                    )
                        .into_response(),
                    Self::Internal(e) => {
                        eprintln!("database error: {e}");
                        (
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                            axum::Json(serde_json::json!({"error": "internal server error"})),
                        )
                            .into_response()
                    }
                }
            }
        }
    }
}
