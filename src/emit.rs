use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use agentspec::plan::{FileWrite, WriteMode, WritePlan};
use anyhow::{Context, Result, bail};

use crate::sync::manifest::{Manifest, ManifestEntry};

/// The outcome of a single file sync operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SyncAction {
    /// File was newly created.
    Created,
    /// An existing file was overwritten with updated content.
    Updated,
    /// No change — content already matches.
    Unchanged,
    /// A user-owned file was backed up with a timestamp suffix before writing.
    BackedUp,
}

/// Execute a write plan: write all file batches, then apply config patches.
pub fn emit(plan: &WritePlan, dry_run: bool) -> Result<()> {
    for w in &plan.writes {
        write_batch(w, dry_run)?;
    }
    for hook in &plan.post_write_hooks {
        hook.run(dry_run)?;
    }
    Ok(())
}

fn write_batch(w: &FileWrite, dry_run: bool) -> Result<()> {
    match w.mode {
        WriteMode::CleanSlate => {
            if !dry_run {
                if w.destination.exists() {
                    fs::remove_dir_all(&w.destination)
                        .with_context(|| format!("failed to delete {}", w.destination.display()))?;
                }
                for file in &w.files {
                    let dest_path = w.destination.join(&file.path);
                    if let Some(parent) = dest_path.parent() {
                        fs::create_dir_all(parent).with_context(|| {
                            format!("failed to create directory {}", parent.display())
                        })?;
                    }
                    fs::write(&dest_path, &file.content)
                        .with_context(|| format!("failed to write {}", dest_path.display()))?;

                    #[cfg(unix)]
                    if let Some(mode) = file.mode {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(&dest_path, fs::Permissions::from_mode(mode))
                            .with_context(|| {
                                format!("failed to set permissions on {}", dest_path.display())
                            })?;
                    }
                }
            }
        }
        WriteMode::ManifestTracked => {
            eprintln!(
                "  {} → {}",
                if dry_run { "[dry-run] sync" } else { "sync" },
                w.destination.display()
            );

            if !dry_run {
                fs::create_dir_all(&w.destination).with_context(|| {
                    format!("failed to create dest dir {}", w.destination.display())
                })?;
            }

            let mut manifest = Manifest::load(&w.destination)?;
            let mut current_keys: HashSet<String> = HashSet::new();
            let mut n_created = 0usize;
            let mut n_updated = 0usize;
            let mut n_backed_up = 0usize;
            let mut n_unchanged = 0usize;
            let mut n_removed = 0usize;

            for file in &w.files {
                // Strip the first path component (kind dir: "agents/", "skills/", etc.)
                // since the destination IS the kind dir.
                let rel: PathBuf = file.path.components().skip(1).collect();
                let rel_str = rel.to_string_lossy().to_string();
                current_keys.insert(rel_str.clone());
                let dest = w.destination.join(&rel);

                let action = write_content_to_dest(
                    &file.content,
                    &dest,
                    &rel_str,
                    &mut manifest,
                    file.mode,
                    w.allow_overwrite,
                    dry_run,
                )?;
                match action {
                    SyncAction::Created => n_created += 1,
                    SyncAction::Updated => n_updated += 1,
                    SyncAction::BackedUp => n_backed_up += 1,
                    SyncAction::Unchanged => n_unchanged += 1,
                }
            }

            // Stale cleanup: remove dest files whose key is no longer in the current batch.
            let stale_keys: Vec<String> = manifest
                .files
                .keys()
                .filter(|k| !current_keys.contains(*k))
                .cloned()
                .collect();
            for key in stale_keys {
                let dest_file = w.destination.join(&key);
                if !dry_run {
                    if dest_file.exists() {
                        fs::remove_file(&dest_file).with_context(|| {
                            format!("failed to remove stale file {}", dest_file.display())
                        })?;
                    }
                    manifest.files.remove(&key);
                }
                n_removed += 1;
            }

            if !dry_run {
                manifest.save(&w.destination)?;
            }

            eprintln!(
                "    created={n_created} updated={n_updated} removed={n_removed} backed_up={n_backed_up} unchanged={n_unchanged}"
            );
        }
    }
    Ok(())
}

/// Writes `content` to `dest` with manifest tracking.
///
/// Behavior:
/// - `rel_path` in manifest AND content same → `Unchanged` (no write)
/// - `rel_path` in manifest AND content differs → overwrite, update manifest
/// - `rel_path` not in manifest AND dest exists AND `allow_overwrite: false` → error
/// - `rel_path` not in manifest AND dest exists AND `allow_overwrite: true` → back up, write, record
/// - dest does not exist → write, record
fn write_content_to_dest(
    content: &[u8],
    dest: &Path,
    rel_path: &str,
    manifest: &mut Manifest,
    mode: Option<u32>,
    allow_overwrite: bool,
    dry_run: bool,
) -> Result<SyncAction> {
    if manifest.files.contains_key(rel_path) {
        // We own this file — check if content changed.
        if dest.exists() {
            let dest_content = fs::read(dest)
                .with_context(|| format!("failed to read dest file {}", dest.display()))?;
            if dest_content == content {
                return Ok(SyncAction::Unchanged);
            }
            if !dry_run {
                write_file(dest, content, mode)?;
                manifest
                    .files
                    .insert(rel_path.to_string(), ManifestEntry {});
            }
            return Ok(SyncAction::Updated);
        }
    } else if dest.exists() {
        if !allow_overwrite {
            bail!(
                "collision: {} exists and is not managed by agentspec; configure a `prefix` in [sync.<provider>] to avoid conflicts, or pass --force to overwrite",
                dest.display()
            );
        }

        // Back up the user-owned file before overwriting.
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut bak_name = dest.as_os_str().to_owned();
        bak_name.push(format!(".bak.{timestamp}"));
        let bak = PathBuf::from(bak_name);
        if !dry_run {
            fs::rename(dest, &bak).with_context(|| {
                format!("failed to back up {} to {}", dest.display(), bak.display())
            })?;
            write_file(dest, content, mode)?;
            manifest
                .files
                .insert(rel_path.to_string(), ManifestEntry {});
        }
        return Ok(SyncAction::BackedUp);
    }

    // dest does not exist — create fresh.
    if !dry_run {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create dir {}", parent.display()))?;
        }
        write_file(dest, content, mode)?;
        manifest
            .files
            .insert(rel_path.to_string(), ManifestEntry {});
    }
    Ok(SyncAction::Created)
}

/// Writes `content` to `dest` and optionally sets Unix file permissions.
fn write_file(dest: &Path, content: &[u8], mode: Option<u32>) -> Result<()> {
    fs::write(dest, content).with_context(|| format!("failed to write {}", dest.display()))?;

    #[cfg(unix)]
    if let Some(m) = mode {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dest, fs::Permissions::from_mode(m))
            .with_context(|| format!("failed to set permissions on {}", dest.display()))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use agentspec::compile::GeneratedFile;
    use agentspec::plan::{FileWrite, WriteMode, WritePlan};
    use agentspec::provider::Provider;
    use tempfile::TempDir;

    use super::*;

    fn make_file(provider: Provider, rel_path: &str, content: &str) -> GeneratedFile {
        GeneratedFile::text(provider, rel_path, content.to_string())
    }

    fn clean_slate_plan(
        provider: Provider,
        output_dir: &Path,
        files: Vec<GeneratedFile>,
    ) -> WritePlan {
        WritePlan {
            writes: vec![FileWrite {
                provider,
                destination: output_dir.join(provider.to_string()),
                files,
                mode: WriteMode::CleanSlate,
                allow_overwrite: true,
            }],
            post_write_hooks: vec![],
        }
    }

    #[test]
    fn test_write_and_verify() {
        let tmp = TempDir::new().expect("expected value");
        let output_dir = tmp.path().join("generated");

        let files = vec![make_file(
            Provider::Claude,
            "skills/test/SKILL.md",
            "---\nname: test\n---\n\nBody.\n",
        )];

        let plan = clean_slate_plan(Provider::Claude, &output_dir, files);
        emit(&plan, false).expect("expected value");

        assert!(output_dir.join("claude/skills/test/SKILL.md").exists());
    }

    #[test]
    fn test_write_cleans_provider_dir_first() {
        let tmp = TempDir::new().expect("expected value");
        let output_dir = tmp.path().join("generated");

        // Create a stale file
        let stale_dir = output_dir.join("claude/agents");
        fs::create_dir_all(&stale_dir).expect("expected value");
        fs::write(stale_dir.join("old.md"), "stale").expect("expected value");

        // Write new files (different path)
        let files = vec![make_file(Provider::Claude, "skills/new/SKILL.md", "fresh")];
        let plan = clean_slate_plan(Provider::Claude, &output_dir, files);
        emit(&plan, false).expect("expected value");

        // Old file should be gone
        assert!(!stale_dir.join("old.md").exists());
        // New file should exist
        assert!(output_dir.join("claude/skills/new/SKILL.md").exists());
    }

    #[test]
    fn test_write_executable_permission() {
        let tmp = TempDir::new().expect("expected value");
        let output_dir = tmp.path().join("generated");

        let plan = WritePlan {
            writes: vec![FileWrite {
                provider: Provider::Claude,
                destination: output_dir.join("claude"),
                files: vec![GeneratedFile::binary(
                    Provider::Claude,
                    "skills/gh-safe/gh-safe.sh",
                    b"#!/bin/bash\necho hi".to_vec(),
                    Some(0o755),
                )],
                mode: WriteMode::CleanSlate,
                allow_overwrite: true,
            }],
            post_write_hooks: vec![],
        };

        emit(&plan, false).expect("expected value");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = fs::metadata(output_dir.join("claude/skills/gh-safe/gh-safe.sh"))
                .expect("expected value");
            assert!(
                meta.permissions().mode() & 0o111 != 0,
                "should be executable"
            );
        }
    }

    #[test]
    fn test_manifest_tracked_creates_file_and_writes_manifest() {
        let tmp = TempDir::new().expect("expected value");
        let dest = tmp.path().join("skills");

        let plan = WritePlan {
            writes: vec![FileWrite {
                provider: Provider::Claude,
                destination: dest.clone(),
                files: vec![make_file(
                    Provider::Claude,
                    "skills/basic/SKILL.md",
                    "---\nname: basic\n---\n\nbody\n",
                )],
                mode: WriteMode::ManifestTracked,
                allow_overwrite: true,
            }],
            post_write_hooks: vec![],
        };

        emit(&plan, false).expect("expected value");

        assert!(dest.join("basic/SKILL.md").exists());
        assert!(dest.join(".agentspec-manifest.json").exists());

        let manifest = Manifest::load(&dest).expect("expected value");
        assert!(manifest.files.contains_key("basic/SKILL.md"));
    }

    #[test]
    fn test_manifest_tracked_stale_cleanup() {
        let tmp = TempDir::new().expect("expected value");
        let dest = tmp.path().join("skills");

        // First sync: write basic/SKILL.md
        let plan = WritePlan {
            writes: vec![FileWrite {
                provider: Provider::Claude,
                destination: dest.clone(),
                files: vec![make_file(Provider::Claude, "skills/basic/SKILL.md", "v1")],
                mode: WriteMode::ManifestTracked,
                allow_overwrite: true,
            }],
            post_write_hooks: vec![],
        };
        emit(&plan, false).expect("expected value");
        assert!(dest.join("basic/SKILL.md").exists());

        // Second sync: empty files list → basic/SKILL.md becomes stale
        let plan2 = WritePlan {
            writes: vec![FileWrite {
                provider: Provider::Claude,
                destination: dest.clone(),
                files: vec![],
                mode: WriteMode::ManifestTracked,
                allow_overwrite: true,
            }],
            post_write_hooks: vec![],
        };
        emit(&plan2, false).expect("expected value");

        assert!(
            !dest.join("basic/SKILL.md").exists(),
            "stale file should be removed"
        );
        let manifest = Manifest::load(&dest).expect("expected value");
        assert!(
            !manifest.files.contains_key("basic/SKILL.md"),
            "stale key should be removed from manifest"
        );
    }

    fn manifest_tracked_plan(
        dest: &Path,
        files: Vec<GeneratedFile>,
        allow_overwrite: bool,
    ) -> WritePlan {
        WritePlan {
            writes: vec![FileWrite {
                provider: Provider::Claude,
                destination: dest.to_path_buf(),
                files,
                mode: WriteMode::ManifestTracked,
                allow_overwrite,
            }],
            post_write_hooks: vec![],
        }
    }

    #[test]
    fn test_manifest_tracked_unchanged_managed_file() {
        let tmp = TempDir::new().expect("expected value");
        let dest = tmp.path().join("skills");

        // First sync: write the file.
        let plan = manifest_tracked_plan(
            &dest,
            vec![make_file(Provider::Claude, "skills/basic/SKILL.md", "body")],
            false,
        );
        emit(&plan, false).expect("expected value");

        // Second sync: same content — should be Unchanged (no manifest rewrite needed).
        let plan2 = manifest_tracked_plan(
            &dest,
            vec![make_file(Provider::Claude, "skills/basic/SKILL.md", "body")],
            false,
        );
        emit(&plan2, false).expect("expected value");

        // File still present and unchanged.
        let content = fs::read_to_string(dest.join("basic/SKILL.md")).expect("expected value");
        assert_eq!(content, "body");
    }

    #[test]
    fn test_manifest_tracked_updated_managed_file() {
        let tmp = TempDir::new().expect("expected value");
        let dest = tmp.path().join("skills");

        let plan = manifest_tracked_plan(
            &dest,
            vec![make_file(Provider::Claude, "skills/basic/SKILL.md", "v1")],
            false,
        );
        emit(&plan, false).expect("expected value");

        let plan2 = manifest_tracked_plan(
            &dest,
            vec![make_file(Provider::Claude, "skills/basic/SKILL.md", "v2")],
            false,
        );
        emit(&plan2, false).expect("expected value");

        let content = fs::read_to_string(dest.join("basic/SKILL.md")).expect("expected value");
        assert_eq!(content, "v2");
    }

    #[test]
    fn test_manifest_tracked_collision_errors_without_overwrite() {
        let tmp = TempDir::new().expect("expected value");
        let dest = tmp.path().join("agents");
        fs::create_dir_all(&dest).expect("expected value");
        // Pre-existing user-owned file (not in manifest).
        fs::write(dest.join("foo.md"), "user owned").expect("expected value");

        let plan = manifest_tracked_plan(
            &dest,
            vec![make_file(Provider::Claude, "agents/foo.md", "agentspec")],
            false, // allow_overwrite = false
        );
        let err = emit(&plan, false).expect_err("expected collision error");
        assert!(
            err.to_string().contains("collision:"),
            "expected collision error, got: {err}"
        );
        // Original file must be untouched.
        let content = fs::read_to_string(dest.join("foo.md")).expect("expected value");
        assert_eq!(content, "user owned");
    }

    #[test]
    fn test_manifest_tracked_backs_up_unmanaged_file_with_force() {
        let tmp = TempDir::new().expect("expected value");
        let dest = tmp.path().join("agents");
        fs::create_dir_all(&dest).expect("expected value");
        fs::write(dest.join("foo.md"), "user owned").expect("expected value");

        let plan = manifest_tracked_plan(
            &dest,
            vec![make_file(Provider::Claude, "agents/foo.md", "agentspec")],
            true, // allow_overwrite = true
        );
        emit(&plan, false).expect("expected value");

        // New content written.
        let content = fs::read_to_string(dest.join("foo.md")).expect("expected value");
        assert_eq!(content, "agentspec");
        // A backup file was created.
        let bak_exists = fs::read_dir(&dest)
            .expect("expected value")
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with("foo.md.bak."));
        assert!(bak_exists, "expected a .bak. backup file");
    }

    #[test]
    fn test_manifest_tracked_dry_run_no_mutations() {
        let tmp = TempDir::new().expect("expected value");
        let dest = tmp.path().join("skills");

        let plan = WritePlan {
            writes: vec![FileWrite {
                provider: Provider::Claude,
                destination: dest.clone(),
                files: vec![make_file(Provider::Claude, "skills/basic/SKILL.md", "body")],
                mode: WriteMode::ManifestTracked,
                allow_overwrite: true,
            }],
            post_write_hooks: vec![],
        };

        emit(&plan, true).expect("expected value");

        assert!(!dest.exists(), "dry-run must not create directory");
    }
}
