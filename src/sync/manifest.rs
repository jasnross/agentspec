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
    pub files: HashMap<String, ManifestEntry>,
}

/// Per-file manifest entry.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ManifestEntry {
    /// Absolute path to the source file in `generated/`.
    pub source: String,
}

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
        manifest.files.insert(
            "foo.md".to_string(),
            ManifestEntry {
                source: "/generated/foo.md".to_string(),
            },
        );
        manifest.save(t.path()).expect("expected value");

        let loaded = Manifest::load(t.path()).expect("expected value");
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.files.len(), 1);
        assert_eq!(loaded.files["foo.md"].source, "/generated/foo.md");
    }
}
