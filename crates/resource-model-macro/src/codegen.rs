use crate::spec::{EntitySpec, FieldSpec, RelationSpec, Spec};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::{HashMap, VecDeque};

// ── Type mapping ────────────────────────────────────────────────────────

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
        _ => unreachable!(
            "unsupported type '{}' should have been caught by validation",
            ty
        ),
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
        "timestamp" => "TIMESTAMPTZ",
        "decimal" => "NUMERIC",
        "json" => "JSONB",
        _ => unreachable!(),
    }
}

/// Safe zero-value DEFAULT clause for each SQL type.
/// Used in `ALTER TABLE ADD COLUMN ... NOT NULL DEFAULT x` so existing rows
/// are backfilled and the NOT NULL constraint doesn't fail.
fn zero_default(ty: &str) -> &'static str {
    match ty {
        "uuid" => " DEFAULT '00000000-0000-0000-0000-000000000000'::uuid",
        "string" | "text" => " DEFAULT ''",
        "int" => " DEFAULT 0",
        "bigint" => " DEFAULT 0",
        "float" => " DEFAULT 0",
        "bool" => " DEFAULT false",
        "timestamp" => " DEFAULT now()",
        "decimal" => " DEFAULT 0",
        "json" => " DEFAULT '{}'::jsonb",
        _ => "",
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

// ── Topological sort ────────────────────────────────────────────────────

fn topological_sort(entities: &[EntitySpec]) -> Vec<&EntitySpec> {
    let name_to_idx: HashMap<&str, usize> = entities
        .iter()
        .enumerate()
        .map(|(i, e)| (e.name.as_str(), i))
        .collect();

    let mut in_degree = vec![0usize; entities.len()];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); entities.len()];

    for (i, entity) in entities.iter().enumerate() {
        for field in &entity.fields {
            if let Some(ref refs) = field.references
                && let Some(&dep_idx) = name_to_idx.get(refs.entity.as_str())
                && dep_idx != i
            {
                adj[dep_idx].push(i);
                in_degree[i] += 1;
            }
        }
    }

    let mut queue: VecDeque<usize> = in_degree
        .iter()
        .enumerate()
        .filter(|(_, d)| **d == 0)
        .map(|(i, _)| i)
        .collect();

    let mut sorted = Vec::with_capacity(entities.len());
    while let Some(idx) = queue.pop_front() {
        sorted.push(&entities[idx]);
        for &next in &adj[idx] {
            in_degree[next] -= 1;
            if in_degree[next] == 0 {
                queue.push_back(next);
            }
        }
    }

    if sorted.len() < entities.len() {
        // Cycle detected — fall back to original order
        entities.iter().collect()
    } else {
        sorted
    }
}

// ── Top-level generator ─────────────────────────────────────────────────

pub fn generate(spec: &Spec) -> TokenStream {
    let sorted = topological_sort(&spec.entities);
    let api = spec.config.api;
    let sd = spec.config.soft_delete;

    let crud_trait = generate_crud_trait();
    let migrate_fn = generate_migrate(spec, &sorted, sd);

    let entities: Vec<TokenStream> = sorted
        .iter()
        .map(|entity| {
            let relations: Vec<&RelationSpec> = spec
                .relations
                .iter()
                .filter(|r| r.source == entity.name)
                .collect();
            generate_entity(entity, &relations, spec, api, sd)
        })
        .collect();

    let api_module = if api {
        generate_api(spec, sd)
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

// ── Additive migration ──────────────────────────────────────────────────

fn generate_migrate(spec: &Spec, sorted: &[&EntitySpec], soft_delete: bool) -> TokenStream {
    let mut stmts: Vec<String> = Vec::new();

    for entity in sorted {
        // Create table with just the PK and timestamp columns
        let mut create_cols = vec![format!(
            "{} {} PRIMARY KEY",
            entity.id.name,
            map_sql_type(&entity.id.ty)
        )];
        create_cols.push("created_at TIMESTAMPTZ NOT NULL DEFAULT now()".into());
        create_cols.push("updated_at TIMESTAMPTZ NOT NULL DEFAULT now()".into());

        stmts.push(format!(
            "CREATE TABLE IF NOT EXISTS {} (\n  {}\n)",
            entity.table,
            create_cols.join(",\n  ")
        ));

        // Add each field column (idempotent).
        // Required fields get a DEFAULT so the ALTER succeeds when the table
        // already has rows (the default backfills existing NULLs).
        for f in &entity.fields {
            let mut col_def = format!("{} {}", f.name, map_sql_type(&f.ty));
            if f.required {
                col_def.push_str(" NOT NULL");
                col_def.push_str(zero_default(&f.ty));
            }
            if f.unique {
                col_def.push_str(" UNIQUE");
            }
            if let Some(ref refs) = f.references {
                let target = spec
                    .entities
                    .iter()
                    .find(|e| e.name == refs.entity)
                    .unwrap();
                col_def.push_str(&format!(" REFERENCES {}({})", target.table, refs.field));
            }
            stmts.push(format!(
                "ALTER TABLE {} ADD COLUMN IF NOT EXISTS {}",
                entity.table, col_def
            ));
        }

        // Ensure timestamp columns exist on pre-existing tables
        stmts.push(format!(
            "ALTER TABLE {} ADD COLUMN IF NOT EXISTS created_at TIMESTAMPTZ NOT NULL DEFAULT now()",
            entity.table
        ));
        stmts.push(format!(
            "ALTER TABLE {} ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ NOT NULL DEFAULT now()",
            entity.table
        ));

        if soft_delete {
            stmts.push(format!(
                "ALTER TABLE {} ADD COLUMN IF NOT EXISTS deleted_at TIMESTAMPTZ",
                entity.table
            ));
        }
    }

    let exec_calls: Vec<TokenStream> = stmts
        .iter()
        .map(|sql| {
            quote! {
                sqlx::query(#sql).execute(pool).await?;
            }
        })
        .collect();

    // Phase 2: Reconcile — drop columns that are no longer in the spec.
    // Without this, removing a field from the YAML leaves a NOT NULL column
    // in the DB that the generated INSERT no longer populates → 422 errors.
    let reconcile_blocks: Vec<TokenStream> = sorted
        .iter()
        .map(|entity| {
            let table = &entity.table;
            let mut expected: Vec<&str> = vec![entity.id.name.as_str()];
            expected.extend(entity.fields.iter().map(|f| f.name.as_str()));
            expected.push("created_at");
            expected.push("updated_at");
            if soft_delete {
                expected.push("deleted_at");
            }

            let info_sql = format!(
                "SELECT column_name::text FROM information_schema.columns \
                 WHERE table_name = '{}' AND table_schema = 'public'",
                entity.table
            );

            quote! {
                {
                    let expected: &[&str] = &[#(#expected),*];
                    let actual: Vec<(String,)> = sqlx::query_as(#info_sql)
                        .fetch_all(pool)
                        .await?;
                    for (col,) in &actual {
                        if !expected.contains(&col.as_str()) {
                            let drop_sql = format!(
                                "ALTER TABLE {} DROP COLUMN IF EXISTS {} CASCADE",
                                #table, col
                            );
                            sqlx::query(&drop_sql).execute(pool).await?;
                        }
                    }
                }
            }
        })
        .collect();

    quote! {
        pub async fn migrate(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
            // Phase 1: ensure all spec tables and columns exist (additive)
            #(#exec_calls)*
            // Phase 2: drop columns no longer in the spec (reconcile)
            #(#reconcile_blocks)*
            Ok(())
        }
    }
}

// ── CRUD trait ──────────────────────────────────────────────────────────

fn generate_crud_trait() -> TokenStream {
    quote! {
        #[async_trait::async_trait]
        pub trait CrudRepository: Send + Sync {
            type Entity: Send + Sync;
            type Create: Send + Sync;
            type Update: Send + Sync;

            async fn create(&self, input: Self::Create) -> Result<Self::Entity, sqlx::Error>;
            async fn find_by_id(&self, id: uuid::Uuid) -> Result<Option<Self::Entity>, sqlx::Error>;
            async fn list(&self, limit: i64, offset: i64) -> Result<(Vec<Self::Entity>, i64), sqlx::Error>;
            async fn update(&self, id: uuid::Uuid, input: Self::Update) -> Result<Option<Self::Entity>, sqlx::Error>;
            async fn delete(&self, id: uuid::Uuid) -> Result<bool, sqlx::Error>;
        }
    }
}

// ── Entity generation ───────────────────────────────────────────────────

fn generate_entity(
    entity: &EntitySpec,
    relations: &[&RelationSpec],
    spec: &Spec,
    api: bool,
    soft_delete: bool,
) -> TokenStream {
    let name = format_ident!("{}", entity.name);
    let create_name = format_ident!("Create{}", entity.name);
    let update_name = format_ident!("Update{}", entity.name);
    let repo_trait_name = format_ident!("{}Repository", entity.name);
    let repo_struct_name = format_ident!("Sqlx{}Repository", entity.name);
    let table = &entity.table;

    let id_ident = format_ident!("{}", entity.id.name);
    let id_type = map_type(&entity.id.ty);

    // ── struct fields (entity includes timestamps) ──────────────────────
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
        .chain(std::iter::once(
            quote! { pub created_at: chrono::DateTime<chrono::Utc> },
        ))
        .chain(std::iter::once(
            quote! { pub updated_at: chrono::DateTime<chrono::Utc> },
        ))
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

    // ── derives ─────────────────────────────────────────────────────────
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

    // ── column lists ────────────────────────────────────────────────────
    let user_col_names: Vec<&str> = entity.fields.iter().map(|f| f.name.as_str()).collect();

    let full_col_list = std::iter::once(entity.id.name.as_str())
        .chain(user_col_names.iter().copied())
        .chain(["created_at", "updated_at"])
        .collect::<Vec<_>>()
        .join(", ");

    let insert_col_names: Vec<&str> = std::iter::once(entity.id.name.as_str())
        .chain(user_col_names.iter().copied())
        .collect();
    let insert_col_list = insert_col_names.join(", ");
    let insert_placeholders: String = (1..=insert_col_names.len())
        .map(|i| format!("${i}"))
        .collect::<Vec<_>>()
        .join(", ");

    // ── SQL statements ──────────────────────────────────────────────────
    let insert_sql = format!(
        "INSERT INTO {table} ({insert_col_list}) VALUES ({insert_placeholders}) RETURNING {full_col_list}"
    );

    let sd_clause = if soft_delete {
        " AND deleted_at IS NULL"
    } else {
        ""
    };

    let select_one_sql = format!(
        "SELECT {full_col_list} FROM {table} WHERE {} = $1{sd_clause}",
        entity.id.name
    );

    let set_clauses: Vec<String> = entity
        .fields
        .iter()
        .enumerate()
        .map(|(i, f)| format!("{name} = COALESCE(${p}, {name})", name = f.name, p = i + 2))
        .collect();
    let update_sql = format!(
        "UPDATE {table} SET {}, updated_at = now() WHERE {} = $1{sd_clause} RETURNING {full_col_list}",
        set_clauses.join(", "),
        entity.id.name
    );

    let delete_sql = if soft_delete {
        format!(
            "UPDATE {table} SET deleted_at = now() WHERE {} = $1 AND deleted_at IS NULL",
            entity.id.name
        )
    } else {
        format!("DELETE FROM {table} WHERE {} = $1", entity.id.name)
    };

    // ── bind chains ─────────────────────────────────────────────────────
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

    // ── list_filtered (pagination + filtering + sorting) ────────────────
    let base_select = format!("SELECT {full_col_list} FROM {table} WHERE 1=1");
    let base_count = format!("SELECT COUNT(*)::bigint FROM {table} WHERE 1=1");
    let sd_push = if soft_delete {
        quote! { qb.push(" AND deleted_at IS NULL"); }
    } else {
        quote! {}
    };

    let filter_arms = generate_filter_arms(&entity.fields);

    let valid_sort_fields: Vec<&str> = user_col_names
        .iter()
        .copied()
        .chain(["created_at", "updated_at"])
        .collect();
    let valid_sorts_check: Vec<TokenStream> =
        valid_sort_fields.iter().map(|s| quote! { #s }).collect();

    let id_name = &entity.id.name;

    let list_filtered_method = quote! {
        pub async fn list_filtered(
            &self,
            filters: &std::collections::HashMap<String, String>,
            sort: Option<&str>,
            order: Option<&str>,
            limit: i64,
            offset: i64,
        ) -> Result<(Vec<#name>, i64), sqlx::Error> {
            let total: i64 = {
                let mut qb = sqlx::QueryBuilder::<sqlx::Postgres>::new(#base_count);
                #sd_push
                #(#filter_arms)*
                let row: (i64,) = qb.build_query_as().fetch_one(&self.pool).await?;
                row.0
            };

            let rows: Vec<#name> = {
                let mut qb = sqlx::QueryBuilder::<sqlx::Postgres>::new(#base_select);
                #sd_push
                #(#filter_arms)*

                let valid_sorts: &[&str] = &[#(#valid_sorts_check),*];
                if let Some(s) = sort {
                    if valid_sorts.contains(&s) {
                        qb.push(" ORDER BY ");
                        qb.push(s);
                        if order == Some("desc") {
                            qb.push(" DESC");
                        }
                    } else {
                        qb.push(concat!(" ORDER BY ", #id_name));
                    }
                } else {
                    qb.push(concat!(" ORDER BY ", #id_name));
                }

                qb.push(" LIMIT ");
                qb.push_bind(limit);
                qb.push(" OFFSET ");
                qb.push_bind(offset);

                qb.build_query_as().fetch_all(&self.pool).await?
            };

            Ok((rows, total))
        }
    };

    // ── relation methods ────────────────────────────────────────────────
    let (rel_trait_methods, rel_impl_methods) =
        generate_relation_methods(relations, spec, soft_delete);

    // ── list via CrudRepository delegates to list_filtered ──────────────
    let list_default_sql = format!(
        "SELECT {full_col_list} FROM {table} WHERE 1=1{sd_clause} ORDER BY {} LIMIT $1 OFFSET $2",
        entity.id.name
    );
    let count_default_sql = format!("SELECT COUNT(*)::bigint FROM {table} WHERE 1=1{sd_clause}");

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

            #list_filtered_method
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

            async fn list(&self, limit: i64, offset: i64) -> Result<(Vec<Self::Entity>, i64), sqlx::Error> {
                let total: (i64,) = sqlx::query_as(#count_default_sql)
                    .fetch_one(&self.pool)
                    .await?;
                let rows = sqlx::query_as::<_, #name>(#list_default_sql)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await?;
                Ok((rows, total.0))
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

// ── Filter arms generation ──────────────────────────────────────────────

fn generate_filter_arms(fields: &[FieldSpec]) -> Vec<TokenStream> {
    fields
        .iter()
        .flat_map(|f| {
            let field_name = &f.name;
            let eq_sql = format!(" AND {} = ", f.name);

            let mut arms = Vec::new();

            match f.ty.as_str() {
                "string" | "text" => {
                    let like_key = format!("{}__contains", f.name);
                    let like_sql = format!(" AND {} ILIKE ", f.name);
                    arms.push(quote! {
                        if let Some(v) = filters.get(#field_name) {
                            qb.push(#eq_sql);
                            qb.push_bind(v.clone());
                        }
                        if let Some(v) = filters.get(#like_key) {
                            qb.push(#like_sql);
                            qb.push_bind(format!("%{v}%"));
                        }
                    });
                }
                "int" => {
                    arms.push(quote! {
                        if let Some(v) = filters.get(#field_name) {
                            if let Ok(n) = v.parse::<i32>() {
                                qb.push(#eq_sql);
                                qb.push_bind(n);
                            }
                        }
                    });
                }
                "bigint" => {
                    arms.push(quote! {
                        if let Some(v) = filters.get(#field_name) {
                            if let Ok(n) = v.parse::<i64>() {
                                qb.push(#eq_sql);
                                qb.push_bind(n);
                            }
                        }
                    });
                }
                "float" => {
                    arms.push(quote! {
                        if let Some(v) = filters.get(#field_name) {
                            if let Ok(n) = v.parse::<f64>() {
                                qb.push(#eq_sql);
                                qb.push_bind(n);
                            }
                        }
                    });
                }
                "bool" => {
                    arms.push(quote! {
                        if let Some(v) = filters.get(#field_name) {
                            let b = matches!(v.as_str(), "true" | "1" | "yes");
                            qb.push(#eq_sql);
                            qb.push_bind(b);
                        }
                    });
                }
                "uuid" => {
                    arms.push(quote! {
                        if let Some(v) = filters.get(#field_name) {
                            if let Ok(id) = v.parse::<uuid::Uuid>() {
                                qb.push(#eq_sql);
                                qb.push_bind(id);
                            }
                        }
                    });
                }
                "decimal" => {
                    arms.push(quote! {
                        if let Some(v) = filters.get(#field_name) {
                            if let Ok(d) = v.parse::<rust_decimal::Decimal>() {
                                qb.push(#eq_sql);
                                qb.push_bind(d);
                            }
                        }
                    });
                }
                "json" => {
                    let like_key = format!("{}__contains", f.name);
                    let cast_sql = format!(" AND {}::text ILIKE ", f.name);
                    arms.push(quote! {
                        if let Some(v) = filters.get(#like_key) {
                            qb.push(#cast_sql);
                            qb.push_bind(format!("%{v}%"));
                        }
                    });
                }
                // timestamp — skip equality filter (ranges would be better)
                _ => {}
            }

            arms
        })
        .collect()
}

// ── Relation methods ────────────────────────────────────────────────────

fn generate_relation_methods(
    relations: &[&RelationSpec],
    spec: &Spec,
    soft_delete: bool,
) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let mut trait_methods = Vec::new();
    let mut impl_methods = Vec::new();

    let sd_clause = if soft_delete {
        " AND deleted_at IS NULL"
    } else {
        ""
    };

    for rel in relations {
        let method = format_ident!("{}", rel.name);
        let target_entity = spec.entities.iter().find(|e| e.name == rel.target).unwrap();
        let target_type = format_ident!("{}", rel.target);
        let fk_param = format_ident!("{}", rel.foreign_key);

        let target_col_list: String = std::iter::once(target_entity.id.name.as_str())
            .chain(target_entity.fields.iter().map(|f| f.name.as_str()))
            .chain(["created_at", "updated_at"])
            .collect::<Vec<_>>()
            .join(", ");

        match rel.kind.as_str() {
            "has_many" => {
                let sql = format!(
                    "SELECT {target_col_list} FROM {} WHERE {} = $1{sd_clause} ORDER BY {}",
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
                    "SELECT {target_col_list} FROM {} WHERE {} = $1{sd_clause}",
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

// ── API generation ──────────────────────────────────────────────────────

fn generate_api(spec: &Spec, soft_delete: bool) -> TokenStream {
    let error_type = generate_api_error();
    let health_endpoints = generate_health_endpoints();
    let auth_middleware = generate_auth_middleware();

    let mut all_handlers = Vec::new();
    let mut route_registrations = Vec::new();

    for entity in &spec.entities {
        let has_many_rels: Vec<&RelationSpec> = spec
            .relations
            .iter()
            .filter(|r| r.source == entity.name && r.kind == "has_many")
            .collect();

        let (handlers, routes) = generate_entity_api(entity, &has_many_rels, spec, soft_delete);
        all_handlers.push(handlers);
        route_registrations.extend(routes);
    }

    quote! {
        pub mod resource_api {
            use super::*;
            use axum::response::IntoResponse;

            #error_type
            #health_endpoints
            #auth_middleware

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
    soft_delete: bool,
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

    let list_desc = format!("List {} (paginated, filterable)", table);
    let create_desc = format!("Create {}", entity_lower);
    let get_desc = format!("Get {} by ID", entity_lower);
    let update_desc = format!("Update {}", entity_lower);
    let delete_desc = if soft_delete {
        format!("Soft-delete {}", entity_lower)
    } else {
        format!("Delete {}", entity_lower)
    };

    let crud_handlers = quote! {
        #[utoipa::path(
            get,
            path = #api_path,
            params(
                ("limit" = Option<i64>, Query, description = "Max results per page (default 50)"),
                ("offset" = Option<i64>, Query, description = "Number of results to skip"),
                ("sort" = Option<String>, Query, description = "Field to sort by"),
                ("order" = Option<String>, Query, description = "Sort direction: asc or desc"),
            ),
            responses((status = 200, description = #list_desc, body = Vec<#name>)),
            tag = #tag
        )]
        pub async fn #list_fn(
            axum::extract::State(pool): axum::extract::State<sqlx::PgPool>,
            axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
        ) -> Result<axum::response::Response, ApiError> {
            let repo = #repo_struct::new(pool);
            let limit: i64 = params.get("limit").and_then(|v| v.parse().ok()).unwrap_or(50);
            let offset: i64 = params.get("offset").and_then(|v| v.parse().ok()).unwrap_or(0);
            let sort = params.get("sort").cloned();
            let order = params.get("order").cloned();
            let filters: std::collections::HashMap<String, String> = params
                .into_iter()
                .filter(|(k, _)| !matches!(k.as_str(), "limit" | "offset" | "sort" | "order"))
                .collect();

            let (rows, total) = repo
                .list_filtered(&filters, sort.as_deref(), order.as_deref(), limit, offset)
                .await
                .map_err(ApiError::from_db)?;

            let mut res = axum::Json(rows).into_response();
            res.headers_mut().insert(
                "x-total-count",
                total.to_string().parse().unwrap(),
            );
            Ok(res)
        }

        #[utoipa::path(
            post,
            path = #api_path,
            request_body = #create_name,
            responses(
                (status = 201, description = #create_desc, body = #name),
                (status = 409, description = "Conflict (duplicate)"),
                (status = 422, description = "Validation error")
            ),
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
                .map_err(ApiError::from_db)
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
                .map_err(ApiError::from_db)?
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
                (status = 404, description = "Not found"),
                (status = 409, description = "Conflict (duplicate)"),
                (status = 422, description = "Validation error")
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
                .map_err(ApiError::from_db)?
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
            if repo.delete(id).await.map_err(ApiError::from_db)? {
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
                repo.#rel_method(id).await.map(axum::Json).map_err(ApiError::from_db)
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

// ── API error with conflict/FK mapping ──────────────────────────────────

fn generate_api_error() -> TokenStream {
    quote! {
        pub enum ApiError {
            NotFound,
            Conflict(String),
            Validation(String),
            Unauthorized,
            Internal(String),
        }

        impl ApiError {
            pub fn from_db(e: sqlx::Error) -> Self {
                if let sqlx::Error::Database(ref db_err) = e {
                    if let Some(code) = db_err.code() {
                        match code.as_ref() {
                            "23505" => return Self::Conflict(db_err.message().to_string()),
                            "23503" => return Self::Validation(format!("foreign key violation: {}", db_err.message())),
                            "23502" => return Self::Validation(format!("not null violation: {}", db_err.message())),
                            "23514" => return Self::Validation(format!("check violation: {}", db_err.message())),
                            _ => {}
                        }
                    }
                }
                Self::Internal(e.to_string())
            }
        }

        impl axum::response::IntoResponse for ApiError {
            fn into_response(self) -> axum::response::Response {
                let (status, msg) = match self {
                    Self::NotFound => (
                        axum::http::StatusCode::NOT_FOUND,
                        "not found".to_string(),
                    ),
                    Self::Conflict(m) => (
                        axum::http::StatusCode::CONFLICT,
                        m,
                    ),
                    Self::Validation(m) => (
                        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                        m,
                    ),
                    Self::Unauthorized => (
                        axum::http::StatusCode::UNAUTHORIZED,
                        "unauthorized".to_string(),
                    ),
                    Self::Internal(m) => {
                        tracing::error!(error = %m, "database error");
                        (
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                            "internal server error".to_string(),
                        )
                    }
                };
                (status, axum::Json(serde_json::json!({"error": msg}))).into_response()
            }
        }
    }
}

// ── Health endpoints ────────────────────────────────────────────────────

fn generate_health_endpoints() -> TokenStream {
    quote! {
        pub async fn healthz() -> &'static str {
            "ok"
        }

        pub async fn readyz(
            axum::extract::State(pool): axum::extract::State<sqlx::PgPool>,
        ) -> Result<&'static str, axum::http::StatusCode> {
            sqlx::query("SELECT 1")
                .execute(&pool)
                .await
                .map(|_| "ok")
                .map_err(|_| axum::http::StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

// ── Auth middleware ──────────────────────────────────────────────────────

fn generate_auth_middleware() -> TokenStream {
    quote! {
        pub async fn api_key_auth(
            req: axum::extract::Request,
            next: axum::middleware::Next,
        ) -> Result<axum::response::Response, axum::http::StatusCode> {
            if let Ok(expected) = std::env::var("API_KEY") {
                let valid = req
                    .headers()
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.strip_prefix("Bearer "))
                    .is_some_and(|key| key == expected);

                if !valid {
                    return Err(axum::http::StatusCode::UNAUTHORIZED);
                }
            }
            Ok(next.run(req).await)
        }
    }
}
