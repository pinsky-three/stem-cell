use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub version: u32,
    pub config: Config,
    pub entities: Vec<EntitySpec>,
    pub relations: Vec<RelationSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub visibility: String,
    pub backend: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntitySpec {
    pub name: String,
    pub table: String,
    pub id: IdSpec,
    pub fields: Vec<FieldSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub required: bool,
    #[serde(default)]
    #[allow(dead_code)]
    pub unique: bool,
    #[serde(default)]
    pub references: Option<ReferenceSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceSpec {
    pub entity: String,
    pub field: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationSpec {
    pub name: String,
    pub kind: String,
    pub source: String,
    pub target: String,
    pub foreign_key: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_spec() {
        let yaml = r#"
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
relations:
  - { name: "posts", kind: "has_many", source: "User", target: "Post", foreign_key: "user_id" }
"#;
        let spec: Spec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.version, 1);
        assert_eq!(spec.config.backend, "postgres");
        assert_eq!(spec.entities.len(), 1);
        assert_eq!(spec.entities[0].fields.len(), 2);
        assert!(spec.entities[0].fields[1].unique);
        assert_eq!(spec.relations.len(), 1);
    }

    #[test]
    fn parse_field_with_reference() {
        let yaml = r#"
version: 1
config:
  visibility: "pub"
  backend: "postgres"
entities:
  - name: "Post"
    table: "posts"
    id: { name: "id", type: "uuid" }
    fields:
      - name: "user_id"
        type: "uuid"
        required: true
        references: { entity: "User", field: "id" }
relations: []
"#;
        let spec: Spec = serde_yaml::from_str(yaml).unwrap();
        let refs = spec.entities[0].fields[0].references.as_ref().unwrap();
        assert_eq!(refs.entity, "User");
        assert_eq!(refs.field, "id");
    }

    #[test]
    fn reject_unknown_config_key() {
        let yaml = r#"
version: 1
config:
  visibility: "pub"
  backend: "postgres"
  extra: "nope"
entities: []
relations: []
"#;
        let result: Result<Spec, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn reject_unknown_entity_key() {
        let yaml = r#"
version: 1
config:
  visibility: "pub"
  backend: "postgres"
entities:
  - name: "User"
    table: "users"
    id: { name: "id", type: "uuid" }
    fields: []
    description: "should fail"
relations: []
"#;
        let result: Result<Spec, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn reject_missing_required_field_attr() {
        let yaml = r#"
version: 1
config:
  visibility: "pub"
  backend: "postgres"
entities:
  - name: "User"
    table: "users"
    id: { name: "id", type: "uuid" }
    fields:
      - { name: "name", type: "string" }
relations: []
"#;
        let result: Result<Spec, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "`required` must be explicit");
    }
}
