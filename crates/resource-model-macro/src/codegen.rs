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

pub fn generate(spec: &Spec) -> TokenStream {
    let crud_trait = generate_crud_trait();
    let migrate_fn = generate_migrate(spec);

    let entities: Vec<TokenStream> = spec
        .entities
        .iter()
        .map(|entity| {
            let relations: Vec<&RelationSpec> = spec
                .relations
                .iter()
                .filter(|r| r.source == entity.name)
                .collect();
            generate_entity(entity, &relations, spec)
        })
        .collect();

    quote! {
        #crud_trait
        #migrate_fn
        #(#entities)*
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
    // Drop in reverse dependency order, create in forward order.
    // CASCADE handles FK deps during drop.
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

fn generate_entity(entity: &EntitySpec, relations: &[&RelationSpec], spec: &Spec) -> TokenStream {
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
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
        pub struct #name {
            #(#entity_fields,)*
        }

        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
        pub struct #create_name {
            #(#create_fields,)*
        }

        #[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
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
