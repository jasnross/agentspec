use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use walkdir::WalkDir;

use crate::config::Provider;
use crate::compile::GeneratedFile;

/// Write all generated files to disk.
///
/// For each provider that appears in `files`, deletes `<output_dir>/<provider>/`
/// before writing anything (clean slate). Then writes each file, creating
/// parent directories as needed and setting permissions if specified.
pub fn write_generated_files(
    files: &[GeneratedFile],
    output_dir: &Path,
    providers: &[Provider],
) -> Result<()> {
    // Delete provider directories for all targeted providers (clean slate)
    for provider in providers {
        let target_dir = output_dir.join(provider.to_string());
        if target_dir.exists() {
            fs::remove_dir_all(&target_dir)
                .with_context(|| format!("failed to delete {}", target_dir.display()))?;
        }
    }

    // Write each file
    for file in files {
        let full_path = output_dir.parent().unwrap_or(output_dir).join(&file.path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }
        fs::write(&full_path, &file.content)
            .with_context(|| format!("failed to write {}", full_path.display()))?;

        #[cfg(unix)]
        if let Some(mode) = file.mode {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&full_path, fs::Permissions::from_mode(mode))
                .with_context(|| format!("failed to set permissions on {}", full_path.display()))?;
        }
    }

    Ok(())
}

/// Differences found when checking generated state against expected output.
#[derive(Debug, Default)]
pub struct CheckResult {
    pub missing: Vec<String>,
    pub outdated: Vec<String>,
    pub unexpected: Vec<String>,
}

impl CheckResult {
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty() && self.outdated.is_empty() && self.unexpected.is_empty()
    }

    pub fn problem_count(&self) -> usize {
        self.missing.len() + self.outdated.len() + self.unexpected.len()
    }
}

/// Compare expected generated files against what's currently on disk.
///
/// Checks for three kinds of problems:
/// - Missing: expected file doesn't exist on disk
/// - Outdated: file exists but content differs
/// - Unexpected: file exists on disk but isn't in the expected set
#[allow(clippy::unnecessary_wraps)] // FIXME: decide on the return type
pub fn check_generated_state(
    expected: &[GeneratedFile],
    base_dir: &Path,
    providers: &[Provider],
) -> Result<CheckResult> {
    let mut result = CheckResult::default();

    // Build map of expected paths → content
    let expected_map: HashMap<String, &[u8]> = expected
        .iter()
        .map(|f| {
            (
                f.path.to_str().unwrap_or_default().to_string(),
                f.content.as_slice(),
            )
        })
        .collect();

    // Check each expected file
    for (rel_path, expected_content) in &expected_map {
        let full_path = base_dir.join(rel_path);
        match fs::read(&full_path) {
            Ok(actual) => {
                if actual != *expected_content {
                    result.outdated.push(rel_path.clone());
                }
            }
            Err(_) => {
                result.missing.push(rel_path.clone());
            }
        }
    }

    // Check for unexpected files on disk
    for provider in providers {
        let target_root = base_dir.join("generated").join(provider.to_string());
        if !target_root.exists() {
            continue;
        }

        let on_disk: HashSet<String> = WalkDir::new(&target_root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| {
                e.path()
                    .strip_prefix(base_dir)
                    .ok()
                    .map(|p| p.to_str().unwrap_or_default().to_string())
            })
            .collect();

        for disk_path in on_disk {
            if !expected_map.contains_key(&disk_path) {
                result.unexpected.push(disk_path);
            }
        }
    }

    // Sort for deterministic output
    result.missing.sort();
    result.outdated.sort();
    result.unexpected.sort();

    Ok(result)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::config::Provider;

    fn make_file(provider: Provider, rel_path: &str, content: &str) -> GeneratedFile {
        GeneratedFile::text(provider, rel_path, content.to_string())
    }

    #[test]
    fn test_write_and_check_clean() {
        let tmp = TempDir::new().expect("expected value");
        let base = tmp.path();
        let output_dir = base.join("generated");

        let files = vec![make_file(
            Provider::Claude,
            "generated/claude/skills/test/SKILL.md",
            "---\nname: test\n---\n\nBody.\n",
        )];

        write_generated_files(&files, &output_dir, &[Provider::Claude]).expect("expected value");

        // Verify file exists
        assert!(base.join("generated/claude/skills/test/SKILL.md").exists());

        let check =
            check_generated_state(&files, base, &[Provider::Claude]).expect("expected value");
        assert!(check.is_clean(), "expected clean check: {check:?}");
    }

    #[test]
    fn test_check_detects_missing_file() {
        let tmp = TempDir::new().expect("expected value");
        let base = tmp.path();

        let files = vec![make_file(
            Provider::Claude,
            "generated/claude/skills/test/SKILL.md",
            "content",
        )];

        // Don't write anything — file is missing
        let check =
            check_generated_state(&files, base, &[Provider::Claude]).expect("expected value");
        assert_eq!(check.missing.len(), 1);
        assert!(check.missing[0].contains("test/SKILL.md"));
    }

    #[test]
    fn test_check_detects_outdated_file() {
        let tmp = TempDir::new().expect("expected value");
        let base = tmp.path();

        let dir = base.join("generated/claude/skills/test");
        fs::create_dir_all(&dir).expect("expected value");
        fs::write(dir.join("SKILL.md"), "old content").expect("expected value");

        let files = vec![make_file(
            Provider::Claude,
            "generated/claude/skills/test/SKILL.md",
            "new content",
        )];

        let check =
            check_generated_state(&files, base, &[Provider::Claude]).expect("expected value");
        assert_eq!(check.outdated.len(), 1);
    }

    #[test]
    fn test_check_detects_unexpected_file() {
        let tmp = TempDir::new().expect("expected value");
        let base = tmp.path();

        // Write an extra file that's not in the expected set
        let dir = base.join("generated/claude/agents");
        fs::create_dir_all(&dir).expect("expected value");
        fs::write(dir.join("stale.md"), "leftover").expect("expected value");

        let files: Vec<GeneratedFile> = vec![];

        let check =
            check_generated_state(&files, base, &[Provider::Claude]).expect("expected value");
        assert_eq!(check.unexpected.len(), 1);
        assert!(check.unexpected[0].contains("stale.md"));
    }

    #[test]
    fn test_write_cleans_provider_dir_first() {
        let tmp = TempDir::new().expect("expected value");
        let base = tmp.path();
        let output_dir = base.join("generated");

        // Create a stale file
        let stale_dir = output_dir.join("claude/agents");
        fs::create_dir_all(&stale_dir).expect("expected value");
        fs::write(stale_dir.join("old.md"), "stale").expect("expected value");

        // Write new files (different path)
        let files = vec![make_file(
            Provider::Claude,
            "generated/claude/skills/new/SKILL.md",
            "fresh",
        )];

        write_generated_files(&files, &output_dir, &[Provider::Claude]).expect("expected value");

        // Old file should be gone
        assert!(!stale_dir.join("old.md").exists());
        // New file should exist
        assert!(base.join("generated/claude/skills/new/SKILL.md").exists());
    }

    #[test]
    fn test_write_executable_permission() {
        let tmp = TempDir::new().expect("expected value");
        let base = tmp.path();
        let output_dir = base.join("generated");

        let files = vec![GeneratedFile::binary(
            Provider::Claude,
            "generated/claude/skills/gh-safe/gh-safe.sh",
            b"#!/bin/bash\necho hi".to_vec(),
            Some(0o755),
        )];

        write_generated_files(&files, &output_dir, &[Provider::Claude]).expect("expected value");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = fs::metadata(base.join("generated/claude/skills/gh-safe/gh-safe.sh"))
                .expect("expected value");
            assert!(
                meta.permissions().mode() & 0o111 != 0,
                "should be executable"
            );
        }
    }
}
