use crate::spec::Spec;
use std::collections::HashSet;

pub fn validate(spec: &Spec) -> Vec<String> {
    let mut errors = Vec::new();

    if spec.version != 1 {
        errors.push(format!("unsupported version {}, expected 1", spec.version));
    }

    if spec.config.backend != "postgres" {
        errors.push(format!(
            "unsupported backend '{}', only 'postgres' is supported",
            spec.config.backend
        ));
    }

    if !matches!(spec.config.visibility.as_str(), "pub" | "pub(crate)" | "") {
        errors.push(format!(
            "unsupported visibility '{}', expected 'pub', 'pub(crate)', or ''",
            spec.config.visibility
        ));
    }

    let mut entity_names: HashSet<&str> = HashSet::new();
    let mut table_names: HashSet<&str> = HashSet::new();

    for entity in &spec.entities {
        if !entity_names.insert(&entity.name) {
            errors.push(format!("duplicate entity name '{}'", entity.name));
        }
        if !table_names.insert(&entity.table) {
            errors.push(format!("duplicate table name '{}'", entity.table));
        }
    }

    let valid_types = ["uuid", "string", "text", "int", "bigint", "float", "bool"];

    for entity in &spec.entities {
        if !valid_types.contains(&entity.id.ty.as_str()) {
            errors.push(format!(
                "{}.{}: unsupported type '{}'",
                entity.name, entity.id.name, entity.id.ty
            ));
        }

        let mut field_names: HashSet<&str> = HashSet::new();
        field_names.insert(&entity.id.name);

        for field in &entity.fields {
            if !field_names.insert(&field.name) {
                errors.push(format!(
                    "{}: duplicate field name '{}'",
                    entity.name, field.name
                ));
            }
            if !valid_types.contains(&field.ty.as_str()) {
                errors.push(format!(
                    "{}.{}: unsupported type '{}'",
                    entity.name, field.name, field.ty
                ));
            }
        }
    }

    for entity in &spec.entities {
        for field in &entity.fields {
            if let Some(ref refs) = field.references {
                if !entity_names.contains(refs.entity.as_str()) {
                    errors.push(format!(
                        "{}.{}: references unknown entity '{}'",
                        entity.name, field.name, refs.entity
                    ));
                } else {
                    let target = spec
                        .entities
                        .iter()
                        .find(|e| e.name == refs.entity)
                        .unwrap();
                    let field_exists = target.id.name == refs.field
                        || target.fields.iter().any(|f| f.name == refs.field);
                    if !field_exists {
                        errors.push(format!(
                            "{}.{}: references unknown field '{}.{}'",
                            entity.name, field.name, refs.entity, refs.field
                        ));
                    }
                }
            }
        }
    }

    let valid_kinds = ["has_many", "belongs_to"];
    for rel in &spec.relations {
        if !valid_kinds.contains(&rel.kind.as_str()) {
            errors.push(format!(
                "relation '{}': unsupported kind '{}', expected 'has_many' or 'belongs_to'",
                rel.name, rel.kind
            ));
        }

        let source_exists = entity_names.contains(rel.source.as_str());
        let target_exists = entity_names.contains(rel.target.as_str());

        if !source_exists {
            errors.push(format!(
                "relation '{}': unknown source entity '{}'",
                rel.name, rel.source
            ));
        }
        if !target_exists {
            errors.push(format!(
                "relation '{}': unknown target entity '{}'",
                rel.name, rel.target
            ));
        }

        if source_exists && target_exists {
            match rel.kind.as_str() {
                "has_many" => {
                    let target = spec
                        .entities
                        .iter()
                        .find(|e| e.name == rel.target)
                        .unwrap();
                    let fk_exists = target.fields.iter().any(|f| f.name == rel.foreign_key);
                    if !fk_exists {
                        errors.push(format!(
                            "relation '{}': foreign key '{}' not found on target entity '{}'",
                            rel.name, rel.foreign_key, rel.target
                        ));
                    }
                }
                "belongs_to" => {
                    let source = spec
                        .entities
                        .iter()
                        .find(|e| e.name == rel.source)
                        .unwrap();
                    let fk_exists = source.fields.iter().any(|f| f.name == rel.foreign_key);
                    if !fk_exists {
                        errors.push(format!(
                            "relation '{}': foreign key '{}' not found on source entity '{}'",
                            rel.name, rel.foreign_key, rel.source
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Spec;

    fn parse(yaml: &str) -> Spec {
        serde_yaml::from_str(yaml).unwrap()
    }

    const VALID: &str = r#"
version: 1
config:
  visibility: "pub"
  backend: "postgres"
entities:
  - name: "User"
    table: "users"
    id: { name: "id", type: "uuid" }
    fields:
      - { name: "name", type: "string", required: true }
      - { name: "email", type: "string", required: true, unique: true }
  - name: "Post"
    table: "posts"
    id: { name: "id", type: "uuid" }
    fields:
      - { name: "title", type: "string", required: true }
      - name: "user_id"
        type: "uuid"
        required: true
        references: { entity: "User", field: "id" }
relations:
  - { name: "posts", kind: "has_many", source: "User", target: "Post", foreign_key: "user_id" }
  - { name: "author", kind: "belongs_to", source: "Post", target: "User", foreign_key: "user_id" }
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
    fn wrong_backend() {
        let yaml = VALID.replace("backend: \"postgres\"", "backend: \"mysql\"");
        let errs = validate(&parse(&yaml));
        assert!(errs.iter().any(|e| e.contains("backend")));
    }

    #[test]
    fn duplicate_entity_name() {
        let yaml = r#"
version: 1
config: { visibility: "pub", backend: "postgres" }
entities:
  - name: "User"
    table: "users"
    id: { name: "id", type: "uuid" }
    fields: []
  - name: "User"
    table: "accounts"
    id: { name: "id", type: "uuid" }
    fields: []
relations: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("duplicate entity")));
    }

    #[test]
    fn duplicate_table_name() {
        let yaml = r#"
version: 1
config: { visibility: "pub", backend: "postgres" }
entities:
  - name: "User"
    table: "people"
    id: { name: "id", type: "uuid" }
    fields: []
  - name: "Admin"
    table: "people"
    id: { name: "id", type: "uuid" }
    fields: []
relations: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("duplicate table")));
    }

    #[test]
    fn duplicate_field_name() {
        let yaml = r#"
version: 1
config: { visibility: "pub", backend: "postgres" }
entities:
  - name: "User"
    table: "users"
    id: { name: "id", type: "uuid" }
    fields:
      - { name: "name", type: "string", required: true }
      - { name: "name", type: "string", required: true }
relations: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("duplicate field")));
    }

    #[test]
    fn field_name_collides_with_id() {
        let yaml = r#"
version: 1
config: { visibility: "pub", backend: "postgres" }
entities:
  - name: "User"
    table: "users"
    id: { name: "id", type: "uuid" }
    fields:
      - { name: "id", type: "uuid", required: true }
relations: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("duplicate field")));
    }

    #[test]
    fn unsupported_field_type() {
        let yaml = r#"
version: 1
config: { visibility: "pub", backend: "postgres" }
entities:
  - name: "User"
    table: "users"
    id: { name: "id", type: "uuid" }
    fields:
      - { name: "age", type: "integer", required: true }
relations: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("unsupported type")));
    }

    #[test]
    fn reference_to_unknown_entity() {
        let yaml = r#"
version: 1
config: { visibility: "pub", backend: "postgres" }
entities:
  - name: "Post"
    table: "posts"
    id: { name: "id", type: "uuid" }
    fields:
      - name: "user_id"
        type: "uuid"
        required: true
        references: { entity: "Usr", field: "id" }
relations: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("unknown entity 'Usr'")));
    }

    #[test]
    fn reference_to_unknown_field() {
        let yaml = r#"
version: 1
config: { visibility: "pub", backend: "postgres" }
entities:
  - name: "User"
    table: "users"
    id: { name: "id", type: "uuid" }
    fields: []
  - name: "Post"
    table: "posts"
    id: { name: "id", type: "uuid" }
    fields:
      - name: "user_id"
        type: "uuid"
        required: true
        references: { entity: "User", field: "uid" }
relations: []
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("unknown field 'User.uid'")));
    }

    #[test]
    fn relation_unknown_source() {
        let yaml = r#"
version: 1
config: { visibility: "pub", backend: "postgres" }
entities:
  - name: "Post"
    table: "posts"
    id: { name: "id", type: "uuid" }
    fields: []
relations:
  - { name: "posts", kind: "has_many", source: "Ghost", target: "Post", foreign_key: "x" }
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("unknown source")));
    }

    #[test]
    fn relation_unsupported_kind() {
        let yaml = r#"
version: 1
config: { visibility: "pub", backend: "postgres" }
entities:
  - name: "User"
    table: "users"
    id: { name: "id", type: "uuid" }
    fields: []
  - name: "Post"
    table: "posts"
    id: { name: "id", type: "uuid" }
    fields: []
relations:
  - { name: "tags", kind: "many_to_many", source: "User", target: "Post", foreign_key: "x" }
"#;
        let errs = validate(&parse(yaml));
        assert!(errs.iter().any(|e| e.contains("unsupported kind")));
    }

    #[test]
    fn relation_missing_fk_on_target() {
        let yaml = r#"
version: 1
config: { visibility: "pub", backend: "postgres" }
entities:
  - name: "User"
    table: "users"
    id: { name: "id", type: "uuid" }
    fields: []
  - name: "Post"
    table: "posts"
    id: { name: "id", type: "uuid" }
    fields:
      - { name: "title", type: "string", required: true }
relations:
  - { name: "posts", kind: "has_many", source: "User", target: "Post", foreign_key: "user_id" }
"#;
        let errs = validate(&parse(yaml));
        assert!(errs
            .iter()
            .any(|e| e.contains("foreign key 'user_id' not found")));
    }
}
