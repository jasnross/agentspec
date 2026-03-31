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
/// `source` was removed: the copy executor now works from in-memory `GeneratedFile`
/// content, so there is no source path to record. Existing manifests on disk that
/// contain `"source": "..."` are silently ignored on load (no `deny_unknown_fields`).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
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
    fn test_load_ignores_stale_source_field() {
        // Existing manifests on disk may have a "source" field from the old schema.
        // Since ManifestEntry does not use deny_unknown_fields, serde_json silently ignores it.
        let t = tmp();
        let stale_json = r#"{"version":1,"files":{"foo.md":{"source":"/generated/foo.md"}}}"#;
        std::fs::write(t.path().join(".agentspec-manifest.json"), stale_json)
            .expect("expected value");

        let loaded = Manifest::load(t.path()).expect("expected value");
        assert_eq!(loaded.files.len(), 1);
        assert!(loaded.files.contains_key("foo.md"));
    }
}
