use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use walkdir::WalkDir;

use super::manifest::{Manifest, ManifestEntry};

/// The outcome of a single file sync operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncAction {
    /// File or symlink was newly created.
    Created,
    /// An existing symlink pointing elsewhere was replaced.
    Updated,
    /// No change — symlink already points to the correct target.
    Unchanged,
    /// A stale symlink was removed.
    Removed,
    /// A regular file was backed up with a timestamp suffix before creating a symlink.
    BackedUp,
}

/// A single sync outcome for reporting.
#[derive(Debug, Clone)]
pub struct SyncEntry {
    // Used in unit tests to assert which file was affected; not yet consumed by production code.
    #[allow(dead_code)]
    pub path: PathBuf,
    pub action: SyncAction,
}

/// Ensures a symlink at `link` points to `target`.
///
/// Behaviour table:
/// - `link` is already a correct symlink → `Unchanged`
/// - `link` is a symlink pointing elsewhere → remove, create new → `Updated`
/// - `link` is a regular file or directory → backup with `.bak.<timestamp>`, create symlink → `BackedUp`
/// - `link` does not exist → create symlink → `Created`
///
/// When `dry_run` is true, no filesystem mutations are performed.
pub fn ensure_symlink(target: &Path, link: &Path, dry_run: bool) -> Result<SyncAction> {
    if link.is_symlink() {
        let current = fs::read_link(link)
            .with_context(|| format!("failed to read symlink {}", link.display()))?;
        if current == target {
            return Ok(SyncAction::Unchanged);
        }
        // Symlink exists but points elsewhere — replace it
        if !dry_run {
            fs::remove_file(link)
                .with_context(|| format!("failed to remove stale symlink {}", link.display()))?;
            std::os::unix::fs::symlink(target, link).with_context(|| {
                format!(
                    "failed to create symlink {} → {}",
                    link.display(),
                    target.display()
                )
            })?;
        }
        return Ok(SyncAction::Updated);
    }

    if link.exists() {
        // Regular file or directory — back it up before symlinking
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut bak = link.as_os_str().to_owned();
        bak.push(format!(".bak.{timestamp}"));
        let bak = PathBuf::from(bak);
        if !dry_run {
            fs::rename(link, &bak).with_context(|| {
                format!("failed to back up {} to {}", link.display(), bak.display())
            })?;
            std::os::unix::fs::symlink(target, link).with_context(|| {
                format!(
                    "failed to create symlink {} → {}",
                    link.display(),
                    target.display()
                )
            })?;
        }
        return Ok(SyncAction::BackedUp);
    }

    // Link does not exist — create it
    if !dry_run {
        std::os::unix::fs::symlink(target, link).with_context(|| {
            format!(
                "failed to create symlink {} → {}",
                link.display(),
                target.display()
            )
        })?;
    }
    Ok(SyncAction::Created)
}

/// Syncs all entries in `source_dir` into `dest_dir` as symlinks.
///
/// - Creates `dest_dir` if it does not exist.
/// - For each file/dir in `source_dir`, calls `ensure_symlink`.
/// - Stale cleanup: removes symlinks in `dest_dir` whose target starts with `source_dir`
///   but no longer exists in `source_dir`.
pub fn sync_symlinked_dir(
    source_dir: &Path,
    dest_dir: &Path,
    dry_run: bool,
) -> Result<Vec<SyncEntry>> {
    if !dry_run {
        fs::create_dir_all(dest_dir)
            .with_context(|| format!("failed to create dest dir {}", dest_dir.display()))?;
    }

    let mut entries = Vec::new();

    // Create/update symlinks for current source entries
    if source_dir.is_dir() {
        for entry in fs::read_dir(source_dir)
            .with_context(|| format!("failed to read source dir {}", source_dir.display()))?
        {
            let entry = entry.with_context(|| {
                format!("failed to iterate source dir {}", source_dir.display())
            })?;
            let name = entry.file_name();
            let target = entry.path();
            let link = dest_dir.join(&name);

            let action = ensure_symlink(&target, &link, dry_run)?;
            entries.push(SyncEntry { path: link, action });
        }
    }

    // Stale cleanup pass: remove symlinks that point into source_dir but target is gone.
    // Compare paths without canonicalization so the check works even when source_dir
    // contains symlink components (e.g., /tmp → /private/tmp on macOS), since
    // ensure_symlink always stores the exact absolute path we pass.
    if dest_dir.is_dir() {
        for entry in fs::read_dir(dest_dir).with_context(|| {
            format!("failed to read dest dir for cleanup {}", dest_dir.display())
        })? {
            let entry = entry
                .with_context(|| format!("failed to iterate dest dir {}", dest_dir.display()))?;
            let link_path = entry.path();

            if !link_path.is_symlink() {
                continue;
            }

            let Ok(target) = fs::read_link(&link_path) else {
                continue;
            };

            // Resolve target to absolute for comparison
            let target_abs = if target.is_absolute() {
                target.clone()
            } else {
                dest_dir.join(&target)
            };

            // Only remove if target is under source_dir and no longer exists.
            if target_abs.starts_with(source_dir) && !target_abs.exists() {
                if !dry_run {
                    fs::remove_file(&link_path).with_context(|| {
                        format!("failed to remove stale symlink {}", link_path.display())
                    })?;
                }
                entries.push(SyncEntry {
                    path: link_path,
                    action: SyncAction::Removed,
                });
            }
        }
    }

    Ok(entries)
}

/// Copies a single source file to dest, tracking ownership in the manifest.
///
/// Behaviour:
/// - `rel_path` not in manifest AND dest exists → backup, copy, record
/// - `rel_path` in manifest AND content differs → warn, overwrite, update manifest
/// - `rel_path` in manifest AND content same → `Unchanged`
/// - dest does not exist → copy, record
///
/// When `dry_run` is true, no filesystem mutations are performed.
pub fn copy_file(
    source: &Path,
    dest: &Path,
    rel_path: &str,
    manifest: &mut Manifest,
    dry_run: bool,
) -> Result<SyncAction> {
    let source_content = fs::read(source)
        .with_context(|| format!("failed to read source file {}", source.display()))?;

    if manifest.files.contains_key(rel_path) {
        // We own this file — check if content changed
        if dest.exists() {
            let dest_content = fs::read(dest)
                .with_context(|| format!("failed to read dest file {}", dest.display()))?;
            if dest_content == source_content {
                return Ok(SyncAction::Unchanged);
            }
            // Content differs — overwrite
            eprintln!(
                "warning: overwriting changed file {} (agentspec-managed)",
                dest.display()
            );
            if !dry_run {
                fs::write(dest, &source_content)
                    .with_context(|| format!("failed to write {}", dest.display()))?;
                manifest.files.insert(
                    rel_path.to_string(),
                    ManifestEntry {
                        source: source.to_string_lossy().into_owned(),
                    },
                );
            }
            return Ok(SyncAction::Updated);
        }
    } else if dest.exists() {
        // Not in manifest but file exists — it's user-owned; back it up
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut bak_str = dest.as_os_str().to_owned();
        bak_str.push(format!(".bak.{timestamp}"));
        let bak = PathBuf::from(bak_str);
        if !dry_run {
            fs::rename(dest, &bak).with_context(|| {
                format!("failed to back up {} to {}", dest.display(), bak.display())
            })?;
            fs::write(dest, &source_content)
                .with_context(|| format!("failed to write {}", dest.display()))?;
            manifest.files.insert(
                rel_path.to_string(),
                ManifestEntry {
                    source: source.to_string_lossy().into_owned(),
                },
            );
        }
        return Ok(SyncAction::BackedUp);
    }

    // dest does not exist — copy fresh
    if !dry_run {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create dir {}", parent.display()))?;
        }
        fs::write(dest, &source_content)
            .with_context(|| format!("failed to write {}", dest.display()))?;
        manifest.files.insert(
            rel_path.to_string(),
            ManifestEntry {
                source: source.to_string_lossy().into_owned(),
            },
        );
    }
    Ok(SyncAction::Created)
}

/// Syncs all files in `source_dir` to `dest_dir` by copying, tracking ownership in `manifest`.
///
/// - Creates `dest_dir` if it does not exist.
/// - Recursively walks `source_dir`; calls `copy_file` for each file.
/// - Stale cleanup: manifest entries whose source paths no longer exist → delete dest file,
///   remove from manifest.
pub fn sync_copied_dir(
    source_dir: &Path,
    dest_dir: &Path,
    manifest: &mut Manifest,
    dry_run: bool,
) -> Result<Vec<SyncEntry>> {
    if !dry_run {
        fs::create_dir_all(dest_dir)
            .with_context(|| format!("failed to create dest dir {}", dest_dir.display()))?;
    }

    let mut entries = Vec::new();

    // Copy current source files
    if source_dir.is_dir() {
        for entry in WalkDir::new(source_dir)
            .min_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let source = entry.path();
            let rel = source
                .strip_prefix(source_dir)
                .with_context(|| format!("path not under source dir: {}", source.display()))?;
            let rel_str = rel.to_string_lossy();
            let dest = dest_dir.join(rel);

            let action = copy_file(source, &dest, &rel_str, manifest, dry_run)?;
            entries.push(SyncEntry { path: dest, action });
        }
    }

    // Stale cleanup: remove dest files whose manifest source no longer exists
    let stale_keys: Vec<String> = manifest
        .files
        .iter()
        .filter(|(_, entry)| !Path::new(&entry.source).exists())
        .map(|(k, _)| k.clone())
        .collect();

    for key in stale_keys {
        let dest_file = dest_dir.join(&key);
        if !dry_run {
            if dest_file.exists() {
                fs::remove_file(&dest_file).with_context(|| {
                    format!("failed to remove stale dest file {}", dest_file.display())
                })?;
            }
            manifest.files.remove(&key);
        }
        entries.push(SyncEntry {
            path: dest_file,
            action: SyncAction::Removed,
        });
    }

    Ok(entries)
}

/// Removes `name:` lines from all `SKILL.md` files under `dest_dir`.
///
/// Used for work-profile plugin copies where the plugin namespace prefix replaces the
/// explicit `name:` frontmatter field.
pub fn apply_strip_name(dest_dir: &Path, dry_run: bool) -> Result<()> {
    for entry in WalkDir::new(dest_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && e.file_name() == "SKILL.md")
    {
        let path = entry.path();
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;

        // Strip `name:` only from within the YAML frontmatter block (between `---` delimiters)
        // to avoid removing matching lines from the Markdown body.
        // Assumes `name:` is always a single-line scalar — multi-line (block/folded) values
        // would require a YAML parser. This is safe because the compiler always emits
        // single-line `name:` values in generated frontmatter.
        let stripped: String = {
            let mut in_frontmatter = false;
            let mut frontmatter_done = false;
            let mut first = true;
            content
                .lines()
                .filter(|line| {
                    if first && *line == "---" {
                        first = false;
                        in_frontmatter = true;
                        return true;
                    }
                    first = false;
                    if in_frontmatter && !frontmatter_done {
                        if *line == "---" {
                            frontmatter_done = true;
                            in_frontmatter = false;
                            return true;
                        }
                        return !line.starts_with("name:");
                    }
                    true
                })
                .map(|line| format!("{line}\n"))
                .collect()
        };

        if stripped != content {
            if dry_run {
                eprintln!("would strip name: from {}", path.display());
            } else {
                fs::write(path, stripped)
                    .with_context(|| format!("failed to write {}", path.display()))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::super::manifest::Manifest;
    use super::*;

    fn setup() -> (TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().expect("expected value");
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).expect("expected value");
        fs::create_dir_all(&dst).expect("expected value");
        (tmp, src, dst)
    }

    #[test]
    fn test_new_entry_creates_symlink() {
        let (_tmp, src, dst) = setup();
        let target = src.join("foo.md");
        fs::write(&target, "hello").expect("expected value");
        let link = dst.join("foo.md");

        let action = ensure_symlink(&target, &link, false).expect("expected value");
        assert_eq!(action, SyncAction::Created);
        assert!(link.is_symlink());
        assert_eq!(fs::read_link(&link).expect("expected value"), target);
    }

    #[test]
    fn test_existing_correct_symlink_is_unchanged() {
        let (_tmp, src, dst) = setup();
        let target = src.join("foo.md");
        fs::write(&target, "hello").expect("expected value");
        let link = dst.join("foo.md");

        std::os::unix::fs::symlink(&target, &link).expect("expected value");
        let action = ensure_symlink(&target, &link, false).expect("expected value");
        assert_eq!(action, SyncAction::Unchanged);
    }

    #[test]
    fn test_symlink_pointing_elsewhere_is_updated() {
        let (_tmp, src, dst) = setup();
        let target_a = src.join("a.md");
        let target_b = src.join("b.md");
        fs::write(&target_a, "a").expect("expected value");
        fs::write(&target_b, "b").expect("expected value");
        let link = dst.join("link.md");

        std::os::unix::fs::symlink(&target_a, &link).expect("expected value");
        let action = ensure_symlink(&target_b, &link, false).expect("expected value");
        assert_eq!(action, SyncAction::Updated);
        assert_eq!(fs::read_link(&link).expect("expected value"), target_b);
    }

    #[test]
    fn test_regular_file_in_dest_is_backed_up() {
        let (_tmp, src, dst) = setup();
        let target = src.join("foo.md");
        fs::write(&target, "new").expect("expected value");
        let link = dst.join("foo.md");
        fs::write(&link, "old content").expect("expected value");

        let action = ensure_symlink(&target, &link, false).expect("expected value");
        assert_eq!(action, SyncAction::BackedUp);
        assert!(link.is_symlink());
        // A .bak.<timestamp> file should exist
        let bak_exists = fs::read_dir(&dst)
            .expect("expected value")
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("foo.md.bak."));
        assert!(bak_exists, "expected backup file");
    }

    #[test]
    fn test_stale_symlink_removed_by_sync_dir() {
        let (_tmp, src, dst) = setup();

        // Create a file, sync it, then delete it from source
        let target = src.join("stale.md");
        fs::write(&target, "gone").expect("expected value");

        sync_symlinked_dir(&src, &dst, false).expect("expected value");
        assert!(dst.join("stale.md").is_symlink());

        // Remove from source
        fs::remove_file(&target).expect("expected value");

        let entries = sync_symlinked_dir(&src, &dst, false).expect("expected value");
        let removed: Vec<_> = entries
            .iter()
            .filter(|e| e.action == SyncAction::Removed)
            .collect();
        assert_eq!(removed.len(), 1);
        assert!(!dst.join("stale.md").exists());
    }

    #[test]
    fn test_non_stale_symlink_pointing_outside_source_preserved() {
        let (_tmp, src, dst) = setup();

        // Place a symlink whose target is outside src — agentspec should leave it alone.
        let outside_target = PathBuf::from("/tmp/some-external-file-that-may-not-exist");
        std::os::unix::fs::symlink(&outside_target, dst.join("user-link.md"))
            .expect("expected value");

        let entries = sync_symlinked_dir(&src, &dst, false).expect("expected value");
        // The user-link.md must not appear as Removed
        let removed_names: Vec<_> = entries
            .iter()
            .filter(|e| e.action == SyncAction::Removed)
            .map(|e| {
                e.path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            })
            .collect();
        assert!(
            !removed_names.contains(&"user-link.md".to_string()),
            "user-link.md should not be removed"
        );
    }

    #[test]
    fn test_dry_run_no_filesystem_mutations() {
        let (_tmp, src, dst) = setup();
        let target = src.join("foo.md");
        fs::write(&target, "hello").expect("expected value");
        let link = dst.join("foo.md");

        let action = ensure_symlink(&target, &link, true).expect("expected value");
        assert_eq!(action, SyncAction::Created);
        // Nothing should have been created
        assert!(!link.exists());
    }

    // -----------------------------------------------------------------------
    // Copy strategy tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_copy_no_manifest_no_dest_creates_file() {
        let (_tmp, src, dst) = setup();
        let source = src.join("foo.md");
        fs::write(&source, "content").expect("expected value");
        let dest = dst.join("foo.md");
        let mut manifest = Manifest::default();

        let action =
            copy_file(&source, &dest, "foo.md", &mut manifest, false).expect("expected value");
        assert_eq!(action, SyncAction::Created);
        assert!(dest.exists());
        assert_eq!(
            fs::read_to_string(&dest).expect("expected value"),
            "content"
        );
        assert!(manifest.files.contains_key("foo.md"));
    }

    #[test]
    fn test_copy_no_manifest_dest_exists_backs_up() {
        let (_tmp, src, dst) = setup();
        let source = src.join("foo.md");
        fs::write(&source, "new").expect("expected value");
        let dest = dst.join("foo.md");
        fs::write(&dest, "old").expect("expected value");
        let mut manifest = Manifest::default();

        let action =
            copy_file(&source, &dest, "foo.md", &mut manifest, false).expect("expected value");
        assert_eq!(action, SyncAction::BackedUp);
        assert_eq!(fs::read_to_string(&dest).expect("expected value"), "new");
        let bak_exists = fs::read_dir(&dst)
            .expect("expected value")
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("foo.md.bak."));
        assert!(bak_exists, "expected backup file");
    }

    #[test]
    fn test_copy_manifest_entry_same_content_unchanged() {
        let (_tmp, src, dst) = setup();
        let source = src.join("foo.md");
        fs::write(&source, "same").expect("expected value");
        let dest = dst.join("foo.md");
        fs::write(&dest, "same").expect("expected value");
        let mut manifest = Manifest::default();
        manifest.files.insert(
            "foo.md".to_string(),
            super::super::manifest::ManifestEntry {
                source: source.to_string_lossy().into_owned(),
            },
        );

        let action =
            copy_file(&source, &dest, "foo.md", &mut manifest, false).expect("expected value");
        assert_eq!(action, SyncAction::Unchanged);
    }

    #[test]
    fn test_copy_manifest_entry_content_differs_overwrites() {
        let (_tmp, src, dst) = setup();
        let source = src.join("foo.md");
        fs::write(&source, "new").expect("expected value");
        let dest = dst.join("foo.md");
        fs::write(&dest, "old").expect("expected value");
        let mut manifest = Manifest::default();
        manifest.files.insert(
            "foo.md".to_string(),
            super::super::manifest::ManifestEntry {
                source: source.to_string_lossy().into_owned(),
            },
        );

        let action =
            copy_file(&source, &dest, "foo.md", &mut manifest, false).expect("expected value");
        assert_eq!(action, SyncAction::Updated);
        assert_eq!(fs::read_to_string(&dest).expect("expected value"), "new");
    }

    #[test]
    fn test_copy_stale_manifest_entry_removes_dest() {
        let (_tmp, src, dst) = setup();
        let source = src.join("gone.md");
        fs::write(&source, "temp").expect("expected value");
        let dest = dst.join("gone.md");

        let mut manifest = Manifest::default();
        sync_copied_dir(&src, &dst, &mut manifest, false).expect("expected value");
        assert!(dest.exists());

        // Remove source
        fs::remove_file(&source).expect("expected value");

        let entries = sync_copied_dir(&src, &dst, &mut manifest, false).expect("expected value");
        let removed: Vec<_> = entries
            .iter()
            .filter(|e| e.action == SyncAction::Removed)
            .collect();
        assert_eq!(removed.len(), 1);
        assert!(!dest.exists());
        assert!(!manifest.files.contains_key("gone.md"));
    }

    #[test]
    fn test_apply_strip_name_removes_name_lines() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skill_dir = tmp.path().join("skill-foo");
        fs::create_dir_all(&skill_dir).expect("expected value");
        let skill_file = skill_dir.join("SKILL.md");
        fs::write(
            &skill_file,
            "---\nname: my-skill\ndescription: test\n---\n\nbody\n",
        )
        .expect("expected value");

        apply_strip_name(tmp.path(), false).expect("expected value");

        let content = fs::read_to_string(&skill_file).expect("expected value");
        assert!(!content.contains("name:"), "name: line should be removed");
        assert!(
            content.contains("description: test"),
            "other lines preserved"
        );
    }

    #[test]
    fn test_apply_strip_name_preserves_body_name_lines() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skill_dir = tmp.path().join("skill-foo");
        fs::create_dir_all(&skill_dir).expect("expected value");
        let skill_file = skill_dir.join("SKILL.md");
        // `name:` appears both in frontmatter and in the Markdown body — only frontmatter should be stripped.
        fs::write(
            &skill_file,
            "---\nname: my-skill\ndescription: test\n---\n\nname: example\n",
        )
        .expect("expected value");

        apply_strip_name(tmp.path(), false).expect("expected value");

        let content = fs::read_to_string(&skill_file).expect("expected value");
        assert!(
            content.contains("name: example"),
            "body name: line should be preserved"
        );
        assert!(
            !content.contains("name: my-skill"),
            "frontmatter name: line should be removed"
        );
    }

    #[test]
    fn test_copy_dry_run_no_mutations() {
        let (_tmp, src, dst) = setup();
        let source = src.join("foo.md");
        fs::write(&source, "content").expect("expected value");
        let dest = dst.join("foo.md");
        let mut manifest = Manifest::default();

        let action =
            copy_file(&source, &dest, "foo.md", &mut manifest, true).expect("expected value");
        assert_eq!(action, SyncAction::Created);
        assert!(!dest.exists());
        assert!(manifest.files.is_empty());
    }
}
