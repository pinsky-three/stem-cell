//! Project manifest — a small YAML file checked into the project root
//! that describes how `stem` should treat the checkout.
//!
//! Phase 1 uses this minimally (just `name` + `template`). Phase 2 will
//! grow a `remote_url` field so `stem push`/`stem pull` know where the
//! canonical GitHub mirror lives.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const MANIFEST_FILENAME: &str = "stem.yaml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectManifest {
    /// Human-readable project name. Typically matches the directory name.
    pub name: String,

    /// The template URL (or built-in name) the project was scaffolded from.
    /// `None` when the project was cloned from an existing repo rather
    /// than initialized from a template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,

    /// Stem-cell tooling version that scaffolded the project. Pinned so
    /// future `stem upgrade` can know what to migrate from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stem_version: Option<String>,
}

impl ProjectManifest {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            template: None,
            stem_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }
    }

    pub fn to_yaml(&self) -> Result<String> {
        serde_yaml::to_string(self).map_err(|e| Error::ManifestParse(e.to_string()))
    }

    pub fn from_yaml(s: &str) -> Result<Self> {
        serde_yaml::from_str(s).map_err(|e| Error::ManifestParse(e.to_string()))
    }

    /// Reads the manifest from `<project_root>/stem.yaml`. Returns
    /// `None` when no manifest file exists (projects scaffolded before
    /// manifests were introduced).
    pub async fn load(project_root: &Path) -> Result<Option<Self>> {
        let path = project_root.join(MANIFEST_FILENAME);
        match tokio::fs::read_to_string(&path).await {
            Ok(s) => Self::from_yaml(&s).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::Io(e)),
        }
    }

    pub async fn save(&self, project_root: &Path) -> Result<()> {
        let path = project_root.join(MANIFEST_FILENAME);
        tokio::fs::write(&path, self.to_yaml()?).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn yaml_roundtrip_preserves_fields() {
        let m = ProjectManifest {
            name: "my-app".into(),
            template: Some("https://github.com/pinsky-three/stem-cell-shrank".into()),
            stem_version: Some("0.1.0".into()),
        };
        let yaml = m.to_yaml().unwrap();
        let parsed = ProjectManifest::from_yaml(&yaml).unwrap();
        assert_eq!(m, parsed);
    }

    #[test]
    fn template_is_optional() {
        let yaml = "name: bare\n";
        let parsed = ProjectManifest::from_yaml(yaml).unwrap();
        assert_eq!(parsed.name, "bare");
        assert!(parsed.template.is_none());
    }

    #[tokio::test]
    async fn load_returns_none_when_file_missing() {
        let dir = tempdir().unwrap();
        assert!(ProjectManifest::load(dir.path()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn save_then_load_roundtrips() {
        let dir = tempdir().unwrap();
        let m = ProjectManifest::new("test-app");
        m.save(dir.path()).await.unwrap();
        let loaded = ProjectManifest::load(dir.path()).await.unwrap().unwrap();
        assert_eq!(loaded.name, "test-app");
    }
}
