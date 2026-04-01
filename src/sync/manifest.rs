use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const MANIFEST_FILE: &str = ".agentspec-manifest.json";

/// Tracks files copied by agentspec so we can detect stale entries and avoid clobbering
/// user-owned files on the first sync.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub version: u32,
    /// Relative path (from dest dir) → entry
    // ManifestEntry is intentionally empty: presence in the map is the ownership signal.
    #[allow(clippy::zero_sized_map_values)]
    pub files: HashMap<String, ManifestEntry>,
}

/// Per-file manifest entry.
///
/// Currently empty — presence in the `files` map is the ownership signal.
/// `deny_unknown_fields` ensures stale or malformed manifest entries surface as
/// parse errors rather than silently succeeding.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestEntry {}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            version: 1,
            files: HashMap::new(),
        }
    }
}

impl Manifest {
    /// Loads `.agentspec-manifest.json` from `dir`. Returns an empty manifest if absent.
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(MANIFEST_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read manifest {}", path.display()))?;
        let manifest: Self = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse manifest {}", path.display()))?;
        Ok(manifest)
    }

    /// Saves the manifest to `.agentspec-manifest.json` in `dir`.
    pub fn save(&self, dir: &Path) -> Result<()> {
        let path = dir.join(MANIFEST_FILE);
        let content =
            serde_json::to_string_pretty(self).context("failed to serialize manifest")? + "\n";
        std::fs::write(&path, content)
            .with_context(|| format!("failed to write manifest {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn tmp() -> TempDir {
        tempfile::tempdir().expect("expected value")
    }

    #[test]
    fn test_load_absent_returns_default() {
        let t = tmp();
        let manifest = Manifest::load(t.path()).expect("expected value");
        assert_eq!(manifest.version, 1);
        assert!(manifest.files.is_empty());
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let t = tmp();
        let mut manifest = Manifest::default();
        manifest
            .files
            .insert("foo.md".to_string(), ManifestEntry {});
        manifest.save(t.path()).expect("expected value");

        let loaded = Manifest::load(t.path()).expect("expected value");
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.files.len(), 1);
        assert!(loaded.files.contains_key("foo.md"));
    }

    #[test]
    fn test_load_rejects_unknown_entry_fields() {
        // Manifests with unknown fields (e.g. old "source" from a prior schema) should
        // fail to parse — unknown fields surface as errors rather than silently succeeding.
        let t = tmp();
        let stale_json = r#"{"version":1,"files":{"foo.md":{"source":"/generated/foo.md"}}}"#;
        std::fs::write(t.path().join(".agentspec-manifest.json"), stale_json)
            .expect("expected value");

        let result = Manifest::load(t.path());
        assert!(result.is_err(), "expected parse error for unknown field");
    }
}
