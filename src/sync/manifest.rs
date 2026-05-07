use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const MANIFEST_FILE: &str = ".agentspec-manifest.json";

/// Manifest schema version written by this binary.
///
/// Bumped when the on-disk shape changes. `Manifest::load_strict` (used by
/// `remove`) refuses any manifest whose `version` exceeds this constant; older
/// versions are accepted best-effort.
pub const MANIFEST_VERSION: u32 = 1;

/// Tracks files copied by agentspec so we can detect stale entries and avoid clobbering
/// user-owned files on the first sync.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub version: u32,
    /// Relative path (from dest dir) → entry
    // ManifestEntry is intentionally empty: presence in the map is the ownership signal.
    #[allow(clippy::zero_sized_map_values)]
    pub files: BTreeMap<String, ManifestEntry>,
}

/// Per-file manifest entry.
///
/// Currently empty — presence in the `files` map is the ownership signal.
/// Older manifests may contain additional fields (e.g., `source`); these are
/// silently ignored on deserialization.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ManifestEntry {}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            version: MANIFEST_VERSION,
            files: BTreeMap::new(),
        }
    }
}

impl Manifest {
    /// Returns the path where the manifest file lives for `dir`. Useful for
    /// existence checks before running a write batch (so empty batches can
    /// skip dir creation when there's no prior manifest to clean from).
    pub fn path(dir: &Path) -> std::path::PathBuf {
        dir.join(MANIFEST_FILE)
    }

    /// Loads `.agentspec-manifest.json` from `dir`. Returns an empty manifest if absent.
    pub fn load(dir: &Path) -> Result<Self> {
        let path = Self::path(dir);
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read manifest {}", path.display()))?;
        let manifest: Self = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse manifest {}", path.display()))?;
        Ok(manifest)
    }

    /// Loads with strict version compatibility: refuses any manifest whose
    /// `version` exceeds [`MANIFEST_VERSION`].
    ///
    /// Used by `remove` so a forward-incompatible manifest is never deleted by
    /// an older binary that can't reason about its contents. Returns the
    /// default empty manifest if the file is absent (matching `load`'s
    /// missing-file behavior).
    pub fn load_strict(dir: &Path) -> Result<Self> {
        let path = Self::path(dir);
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read manifest {}", path.display()))?;
        let manifest: Self = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse manifest {}", path.display()))?;
        if manifest.version > MANIFEST_VERSION {
            anyhow::bail!(
                "manifest at {} has version {}, but this agentspec binary writes version {}; \
                 upgrade agentspec or remove the manifest manually",
                path.display(),
                manifest.version,
                MANIFEST_VERSION,
            );
        }
        Ok(manifest)
    }

    /// Saves the manifest to `.agentspec-manifest.json` in `dir`.
    pub fn save(&self, dir: &Path) -> Result<()> {
        let path = Self::path(dir);
        let content =
            serde_json::to_string_pretty(self).context("failed to serialize manifest")? + "\n";
        std::fs::write(&path, content)
            .with_context(|| format!("failed to write manifest {}", path.display()))?;
        Ok(())
    }

    /// Deletes the manifest file in `dir`, if present. Treats `NotFound` as
    /// success so callers can use this as an idempotent cleanup step.
    pub fn delete(dir: &Path) -> Result<()> {
        let path = Self::path(dir);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => {
                Err(e).with_context(|| format!("failed to remove manifest at {}", path.display()))
            }
        }
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
    fn test_load_ignores_unknown_entry_fields() {
        // Manifests with unknown fields (e.g. old "source" from a prior schema) should
        // parse successfully — unknown fields are silently ignored for backward compat.
        let t = tmp();
        let stale_json = r#"{"version":1,"files":{"foo.md":{"source":"/generated/foo.md"}}}"#;
        std::fs::write(t.path().join(".agentspec-manifest.json"), stale_json)
            .expect("expected value");

        let manifest = Manifest::load(t.path()).expect("old manifest should parse");
        assert!(manifest.files.contains_key("foo.md"));
    }

    #[test]
    fn test_load_strict_refuses_higher_version() {
        let t = tmp();
        std::fs::write(
            t.path().join(".agentspec-manifest.json"),
            r#"{"version":2,"files":{}}"#,
        )
        .expect("expected value");

        let err = Manifest::load_strict(t.path()).expect_err("expected version refusal");
        let full = format!("{err:#}");
        assert!(full.contains("version 2"), "error: {full}");
        assert!(full.contains("version 1"), "error: {full}");
        assert!(full.contains("upgrade agentspec"), "error: {full}");
    }

    // No `test_load_strict_accepts_lower_version` — `MANIFEST_VERSION` is `1`,
    // so a "lower than 1" manifest cannot be constructed without writing
    // `version: 0`, which has never been a real value. Add this case once
    // `MANIFEST_VERSION > 1`.

    #[test]
    fn test_load_strict_returns_default_for_missing_file() {
        let t = tmp();
        let manifest = Manifest::load_strict(t.path()).expect("missing manifest is fine");
        assert_eq!(manifest.version, MANIFEST_VERSION);
        assert!(manifest.files.is_empty());
    }

    #[test]
    fn test_delete_is_no_op_when_missing() {
        let t = tmp();
        Manifest::delete(t.path()).expect("delete should succeed when file is absent");
    }

    #[test]
    fn test_delete_removes_existing_manifest() {
        let t = tmp();
        Manifest::default()
            .save(t.path())
            .expect("save should succeed");
        let path = Manifest::path(t.path());
        assert!(path.exists(), "manifest should exist after save");

        Manifest::delete(t.path()).expect("delete should succeed");
        assert!(!path.exists(), "manifest should be gone after delete");
    }
}
