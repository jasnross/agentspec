use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use walkdir::WalkDir;

use super::manifest::{Manifest, ManifestEntry};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NamePrefixMode {
    Agents,
    Skills,
}

/// The outcome of a single file sync operation.
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug)]
pub struct SyncEntry {
    // Used in unit tests to assert which file was affected; not yet consumed by production code.
    #[allow(dead_code)]
    pub path: PathBuf,
    pub action: SyncAction,
}

/// Ensures a symlink at `link` points to `target`.
///
/// Behavior table:
/// - `link` is already a correct symlink → `Unchanged`
/// - `link` is a symlink pointing elsewhere:
///   - if `allow_overwrite` is false and target is outside the managed source dir → error
///   - otherwise remove, create new → `Updated`
/// - `link` is a regular file or directory:
///   - if `allow_overwrite` is false → error
///   - otherwise backup with `.bak.<timestamp>`, create symlink → `BackedUp`
/// - `link` does not exist → create symlink → `Created`
///
/// When `dry_run` is true, no filesystem mutations are performed.
pub fn ensure_symlink(
    target: &Path,
    link: &Path,
    allow_overwrite: bool,
    dry_run: bool,
) -> Result<SyncAction> {
    if link.is_symlink() {
        let current = fs::read_link(link)
            .with_context(|| format!("failed to read symlink {}", link.display()))?;
        if current == target {
            return Ok(SyncAction::Unchanged);
        }

        if !allow_overwrite {
            let current_abs = if current.is_absolute() {
                current.clone()
            } else {
                link.parent()
                    .map_or_else(|| PathBuf::from(&current), |parent| parent.join(&current))
            };
            let current_abs = lexical_normalize(&current_abs);
            let is_managed_symlink = target.parent().is_some_and(|source_dir| {
                let source_dir = lexical_normalize(source_dir);
                current_abs.starts_with(&source_dir)
            });

            if !is_managed_symlink {
                bail!(
                    "collision: {} exists and is not managed by agentspec; configure a `prefix` in [sync.<provider>] to avoid conflicts, or pass --force to overwrite",
                    link.display()
                );
            }
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
        if !allow_overwrite {
            bail!(
                "collision: {} exists and is not managed by agentspec; configure a `prefix` in [sync.<provider>] to avoid conflicts, or pass --force to overwrite",
                link.display()
            );
        }

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
    file_prefix: Option<&str>,
    allow_overwrite: bool,
    dry_run: bool,
) -> Result<Vec<SyncEntry>> {
    if !dry_run {
        fs::create_dir_all(dest_dir)
            .with_context(|| format!("failed to create dest dir {}", dest_dir.display()))?;
    }

    let mut entries = Vec::new();
    let mut expected_link_names: HashSet<OsString> = HashSet::new();

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
            let dest_name = prefixed_name(&name, file_prefix);
            expected_link_names.insert(dest_name.clone());
            let link = dest_dir.join(&dest_name);

            let action = ensure_symlink(&target, &link, allow_overwrite, dry_run)?;
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
            let target_norm = lexical_normalize(&target_abs);
            let source_norm = lexical_normalize(source_dir);

            // Only remove if target is under source_dir and no longer exists.
            let is_expected_name = link_path
                .file_name()
                .is_some_and(|name| expected_link_names.contains(name));

            if target_norm.starts_with(&source_norm)
                && (!target_abs.exists() || (allow_overwrite && !is_expected_name))
            {
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
/// Behavior:
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
    name_prefix: Option<(&str, NamePrefixMode)>,
    allow_overwrite: bool,
    dry_run: bool,
) -> Result<SyncAction> {
    let mut source_content = fs::read(source)
        .with_context(|| format!("failed to read source file {}", source.display()))?;

    if let Some((prefix, mode)) = name_prefix {
        let rel_path = Path::new(rel_path);
        if should_prefix_frontmatter_name(rel_path, mode)
            && let Ok(source_text) = std::str::from_utf8(&source_content)
        {
            let prefixed_text = prefix_frontmatter_name(source_text, prefix);
            if prefixed_text != source_text {
                source_content = prefixed_text.into_bytes();
            }
        }
    }

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
        if !allow_overwrite {
            bail!(
                "collision: {} exists and is not managed by agentspec; configure a `prefix` in [sync.<provider>] to avoid conflicts, or pass --force to overwrite",
                dest.display()
            );
        }

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
    file_prefix: Option<&str>,
    name_prefix: Option<(&str, NamePrefixMode)>,
    allow_overwrite: bool,
    dry_run: bool,
) -> Result<Vec<SyncEntry>> {
    if !dry_run {
        fs::create_dir_all(dest_dir)
            .with_context(|| format!("failed to create dest dir {}", dest_dir.display()))?;
    }

    let mut entries = Vec::new();
    let mut current_manifest_keys: HashSet<String> = HashSet::new();

    // Copy current source files
    if source_dir.is_dir() {
        for entry in WalkDir::new(source_dir)
            .min_depth(1)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
        {
            let source = entry.path();
            let rel = source
                .strip_prefix(source_dir)
                .with_context(|| format!("path not under source dir: {}", source.display()))?;
            let prefixed_rel = prefix_rel_path(rel, file_prefix);
            let rel_str = prefixed_rel.to_string_lossy();
            current_manifest_keys.insert(rel_str.to_string());
            let dest = dest_dir.join(&prefixed_rel);

            let action = copy_file(
                source,
                &dest,
                &rel_str,
                manifest,
                name_prefix,
                allow_overwrite,
                dry_run,
            )?;
            entries.push(SyncEntry { path: dest, action });
        }
    }

    // Stale cleanup: remove dest files whose manifest source no longer exists
    let stale_keys: Vec<String> = manifest
        .files
        .iter()
        .filter(|(key, entry)| {
            !current_manifest_keys.contains(*key) || !Path::new(&entry.source).exists()
        })
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

fn prefixed_name(name: &OsStr, prefix: Option<&str>) -> OsString {
    match prefix {
        None => name.to_owned(),
        Some(prefix) => {
            let mut result = OsString::from(prefix);
            result.push(name);
            result
        }
    }
}

fn prefix_rel_path(rel: &Path, prefix: Option<&str>) -> PathBuf {
    let Some(prefix) = prefix else {
        return rel.to_path_buf();
    };

    let mut components = rel.components();
    let Some(first) = components.next() else {
        return rel.to_path_buf();
    };

    let mut prefixed_first = OsString::from(prefix);
    prefixed_first.push(first.as_os_str());
    PathBuf::from(prefixed_first).join(components.as_path())
}

fn should_prefix_frontmatter_name(rel_path: &Path, mode: NamePrefixMode) -> bool {
    match mode {
        NamePrefixMode::Skills => rel_path.file_name().is_some_and(|name| name == "SKILL.md"),
        NamePrefixMode::Agents => {
            rel_path.extension().is_some_and(|ext| ext == "md")
                && rel_path
                    .parent()
                    .is_none_or(|parent| parent.as_os_str().is_empty())
        }
    }
}

fn prefix_frontmatter_name(content: &str, prefix: &str) -> String {
    let mut in_frontmatter = false;
    let mut frontmatter_done = false;
    let mut first = true;
    let prefix_marker = format!("{prefix}:");

    content.lines().fold(String::new(), |mut out, line| {
        if first && line == "---" {
            first = false;
            in_frontmatter = true;
        } else {
            first = false;

            if in_frontmatter && !frontmatter_done {
                if line == "---" {
                    frontmatter_done = true;
                    in_frontmatter = false;
                } else if let Some(value) = line.strip_prefix("name: ")
                    && !value.starts_with(&prefix_marker)
                {
                    out.push_str("name: ");
                    out.push_str(prefix);
                    out.push(':');
                    out.push_str(value);
                    out.push('\n');
                    return out;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
        out
    })
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let can_pop = normalized
                    .components()
                    .next_back()
                    .is_some_and(|c| c != Component::RootDir);
                if can_pop {
                    normalized.pop();
                }
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
        }
    }

    normalized
}

/// Rewrites `name:` in `SKILL.md` and agent `.md` frontmatter to add a colon-separated prefix.
///
/// For skills: walks `dest_dir` for files named `SKILL.md`.
/// For agents: walks `dest_dir` for top-level `*.md` files (agents are flat files).
/// In both cases, replaces `name: <value>` with `name: <prefix>:<value>` in the
/// YAML frontmatter block only.
///
/// Only operates on files whose `name:` line does not already start with `<prefix>:`.
/// This makes the operation idempotent on re-sync.
#[allow(dead_code)] // Kept as a standalone post-processor utility for dedicated unit coverage.
pub fn apply_prefix_name(dest_dir: &Path, prefix: &str, dry_run: bool) -> Result<()> {
    for entry in WalkDir::new(dest_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            if !e.file_type().is_file() {
                return false;
            }

            let path = e.path();
            let is_skill_file = e.file_name() == "SKILL.md";
            let is_agent_file = path.extension().is_some_and(|ext| ext == "md")
                && path.parent().is_some_and(|parent| parent == dest_dir);

            is_skill_file || is_agent_file
        })
    {
        let path = entry.path();
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;

        let prefixed = prefix_frontmatter_name(&content, prefix);

        if prefixed != content {
            if dry_run {
                eprintln!("would prefix name: in {}", path.display());
            } else {
                fs::write(path, prefixed)
                    .with_context(|| format!("failed to write {}", path.display()))?;
            }
        }
    }

    Ok(())
}

/// Removes `name:` lines from all `SKILL.md` files under `dest_dir`.
// FIXME: this isn't needed once we are using structs
pub fn apply_strip_name(dest_dir: &Path, dry_run: bool) -> Result<()> {
    for entry in WalkDir::new(dest_dir)
        .into_iter()
        .filter_map(Result::ok)
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
                .fold(String::new(), |mut out, line| {
                    out.push_str(line);
                    out.push('\n');
                    out
                })
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

        let action = ensure_symlink(&target, &link, true, false).expect("expected value");
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
        let action = ensure_symlink(&target, &link, true, false).expect("expected value");
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
        let action = ensure_symlink(&target_b, &link, true, false).expect("expected value");
        assert_eq!(action, SyncAction::Updated);
        assert_eq!(fs::read_link(&link).expect("expected value"), target_b);
    }

    #[test]
    fn test_symlink_pointing_elsewhere_errors_by_default() {
        let (_tmp, src, dst) = setup();
        let target_b = src.join("b.md");
        fs::write(&target_b, "b").expect("expected value");
        let unmanaged_target = dst.join("unmanaged.md");
        fs::write(&unmanaged_target, "x").expect("expected value");
        let link = dst.join("link.md");

        std::os::unix::fs::symlink(&unmanaged_target, &link).expect("expected value");
        let err = ensure_symlink(&target_b, &link, false, false).expect_err("expected error");
        assert!(err.to_string().contains("collision:"));
        assert_eq!(
            fs::read_link(&link).expect("expected value"),
            unmanaged_target
        );
    }

    #[test]
    fn test_symlink_pointing_elsewhere_managed_updates_without_force() {
        let (_tmp, src, dst) = setup();
        let target_a = src.join("a.md");
        let target_b = src.join("b.md");
        fs::write(&target_a, "a").expect("expected value");
        fs::write(&target_b, "b").expect("expected value");
        let link = dst.join("link.md");

        std::os::unix::fs::symlink(&target_a, &link).expect("expected value");
        let action = ensure_symlink(&target_b, &link, false, false).expect("expected value");
        assert_eq!(action, SyncAction::Updated);
        assert_eq!(fs::read_link(&link).expect("expected value"), target_b);
    }

    #[test]
    fn test_symlink_with_parent_segments_is_not_treated_as_managed() {
        let (_tmp, src, dst) = setup();
        let target = src.join("target.md");
        fs::write(&target, "managed").expect("expected value");

        let unmanaged = dst.join("unmanaged.md");
        fs::write(&unmanaged, "user").expect("expected value");
        let tricky = src.join("../dst/unmanaged.md");
        let link = dst.join("link.md");
        std::os::unix::fs::symlink(&tricky, &link).expect("expected value");

        let err = ensure_symlink(&target, &link, false, false).expect_err("expected error");
        assert!(err.to_string().contains("collision:"));
        assert_eq!(fs::read_link(&link).expect("expected value"), tricky);
    }

    #[test]
    fn test_regular_file_in_dest_is_backed_up() {
        let (_tmp, src, dst) = setup();
        let target = src.join("foo.md");
        fs::write(&target, "new").expect("expected value");
        let link = dst.join("foo.md");
        fs::write(&link, "old content").expect("expected value");

        let action = ensure_symlink(&target, &link, true, false).expect("expected value");
        assert_eq!(action, SyncAction::BackedUp);
        assert!(link.is_symlink());
        // A .bak.<timestamp> file should exist
        let bak_exists = fs::read_dir(&dst)
            .expect("expected value")
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with("foo.md.bak."));
        assert!(bak_exists, "expected backup file");
    }

    #[test]
    fn test_ensure_symlink_collision_errors_by_default() {
        let (_tmp, src, dst) = setup();
        let target = src.join("foo.md");
        fs::write(&target, "new").expect("expected value");
        let link = dst.join("foo.md");
        fs::write(&link, "old content").expect("expected value");

        let err = ensure_symlink(&target, &link, false, false).expect_err("expected error");
        let message = err.to_string();
        assert!(message.contains("collision:"));
        assert_eq!(
            fs::read_to_string(&link).expect("expected value"),
            "old content"
        );
        let bak_exists = fs::read_dir(&dst)
            .expect("expected value")
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with("foo.md.bak."));
        assert!(!bak_exists, "backup should not be created");
    }

    #[test]
    fn test_ensure_symlink_collision_allowed_with_force() {
        let (_tmp, src, dst) = setup();
        let target = src.join("foo.md");
        fs::write(&target, "new").expect("expected value");
        let link = dst.join("foo.md");
        fs::write(&link, "old content").expect("expected value");

        let action = ensure_symlink(&target, &link, true, false).expect("expected value");
        assert_eq!(action, SyncAction::BackedUp);
        assert!(link.is_symlink());
    }

    #[test]
    fn test_stale_symlink_removed_by_sync_dir() {
        let (_tmp, src, dst) = setup();

        // Create a file, sync it, then delete it from source
        let target = src.join("stale.md");
        fs::write(&target, "gone").expect("expected value");

        sync_symlinked_dir(&src, &dst, None, true, false).expect("expected value");
        assert!(dst.join("stale.md").is_symlink());

        // Remove from source
        fs::remove_file(&target).expect("expected value");

        let entries = sync_symlinked_dir(&src, &dst, None, true, false).expect("expected value");
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

        let entries = sync_symlinked_dir(&src, &dst, None, true, false).expect("expected value");
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
    fn test_non_expected_symlink_name_preserved_without_force() {
        let (_tmp, src, dst) = setup();
        let target = src.join("commit");
        fs::create_dir_all(&target).expect("expected value");

        std::os::unix::fs::symlink(&target, dst.join("old-commit")).expect("expected value");

        let entries = sync_symlinked_dir(&src, &dst, None, false, false).expect("expected value");
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

        assert!(!removed_names.contains(&"old-commit".to_string()));
        assert!(dst.join("old-commit").is_symlink());
    }

    #[test]
    fn test_dry_run_no_filesystem_mutations() {
        let (_tmp, src, dst) = setup();
        let target = src.join("foo.md");
        fs::write(&target, "hello").expect("expected value");
        let link = dst.join("foo.md");

        let action = ensure_symlink(&target, &link, true, true).expect("expected value");
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

        let action = copy_file(&source, &dest, "foo.md", &mut manifest, None, true, false)
            .expect("expected value");
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

        let action = copy_file(&source, &dest, "foo.md", &mut manifest, None, true, false)
            .expect("expected value");
        assert_eq!(action, SyncAction::BackedUp);
        assert_eq!(fs::read_to_string(&dest).expect("expected value"), "new");
        let bak_exists = fs::read_dir(&dst)
            .expect("expected value")
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with("foo.md.bak."));
        assert!(bak_exists, "expected backup file");
    }

    #[test]
    fn test_copy_file_collision_errors_by_default() {
        let (_tmp, src, dst) = setup();
        let source = src.join("foo.md");
        fs::write(&source, "new").expect("expected value");
        let dest = dst.join("foo.md");
        fs::write(&dest, "old").expect("expected value");
        let mut manifest = Manifest::default();

        let err = copy_file(&source, &dest, "foo.md", &mut manifest, None, false, false)
            .expect_err("expected error");
        let message = err.to_string();
        assert!(message.contains("collision:"));
        assert_eq!(fs::read_to_string(&dest).expect("expected value"), "old");
        assert!(manifest.files.is_empty());
    }

    #[test]
    fn test_copy_file_collision_allowed_with_force() {
        let (_tmp, src, dst) = setup();
        let source = src.join("foo.md");
        fs::write(&source, "new").expect("expected value");
        let dest = dst.join("foo.md");
        fs::write(&dest, "old").expect("expected value");
        let mut manifest = Manifest::default();

        let action = copy_file(&source, &dest, "foo.md", &mut manifest, None, true, false)
            .expect("expected value");
        assert_eq!(action, SyncAction::BackedUp);
        assert_eq!(fs::read_to_string(&dest).expect("expected value"), "new");
        assert!(manifest.files.contains_key("foo.md"));
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

        let action = copy_file(&source, &dest, "foo.md", &mut manifest, None, true, false)
            .expect("expected value");
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

        let action = copy_file(&source, &dest, "foo.md", &mut manifest, None, true, false)
            .expect("expected value");
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
        sync_copied_dir(&src, &dst, &mut manifest, None, None, true, false)
            .expect("expected value");
        assert!(dest.exists());

        // Remove source
        fs::remove_file(&source).expect("expected value");

        let entries = sync_copied_dir(&src, &dst, &mut manifest, None, None, true, false)
            .expect("expected value");
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

        let action = copy_file(&source, &dest, "foo.md", &mut manifest, None, true, true)
            .expect("expected value");
        assert_eq!(action, SyncAction::Created);
        assert!(!dest.exists());
        assert!(manifest.files.is_empty());
    }

    #[test]
    fn test_copy_file_name_prefix_applies_to_skill_frontmatter() {
        let (_tmp, src, dst) = setup();
        let source = src.join("tw-commit/SKILL.md");
        fs::create_dir_all(source.parent().expect("expected parent")).expect("expected value");
        fs::write(&source, "---\nname: commit\n---\n\nbody\n").expect("expected value");
        let dest = dst.join("tw-commit/SKILL.md");
        let mut manifest = Manifest::default();

        let action = copy_file(
            &source,
            &dest,
            "tw-commit/SKILL.md",
            &mut manifest,
            Some(("tw", NamePrefixMode::Skills)),
            true,
            false,
        )
        .expect("expected value");

        assert_eq!(action, SyncAction::Created);
        let content = fs::read_to_string(&dest).expect("expected value");
        assert!(content.contains("name: tw:commit"));
    }

    #[test]
    fn test_copy_file_name_prefix_preserves_body_name_lines() {
        let (_tmp, src, dst) = setup();
        let source = src.join("tw-commit/SKILL.md");
        fs::create_dir_all(source.parent().expect("expected parent")).expect("expected value");
        fs::write(&source, "---\nname: commit\n---\n\nname: keep-body\n").expect("expected value");
        let dest = dst.join("tw-commit/SKILL.md");
        let mut manifest = Manifest::default();

        copy_file(
            &source,
            &dest,
            "tw-commit/SKILL.md",
            &mut manifest,
            Some(("tw", NamePrefixMode::Skills)),
            true,
            false,
        )
        .expect("expected value");

        let content = fs::read_to_string(&dest).expect("expected value");
        assert!(content.contains("name: tw:commit"));
        assert!(content.contains("name: keep-body"));
    }

    #[test]
    fn test_sync_symlinked_dir_applies_file_prefix() {
        let (_tmp, src, dst) = setup();
        fs::create_dir_all(src.join("commit")).expect("expected value");

        let entries =
            sync_symlinked_dir(&src, &dst, Some("tw-"), true, false).expect("expected value");

        assert!(entries.iter().any(|e| e.path.ends_with("tw-commit")));
        let link = dst.join("tw-commit");
        assert!(link.is_symlink());
        assert_eq!(
            fs::read_link(&link).expect("expected value"),
            src.join("commit")
        );
    }

    #[test]
    fn test_sync_symlinked_dir_no_prefix_unchanged() {
        let (_tmp, src, dst) = setup();
        fs::create_dir_all(src.join("commit")).expect("expected value");

        let entries = sync_symlinked_dir(&src, &dst, None, true, false).expect("expected value");

        assert!(entries.iter().any(|e| e.path.ends_with("commit")));
        let link = dst.join("commit");
        assert!(link.is_symlink());
        assert_eq!(
            fs::read_link(&link).expect("expected value"),
            src.join("commit")
        );
    }

    #[test]
    fn test_sync_copied_dir_applies_file_prefix() {
        let (_tmp, src, dst) = setup();
        fs::create_dir_all(src.join("commit")).expect("expected value");
        fs::write(src.join("commit/SKILL.md"), "body").expect("expected value");
        let mut manifest = Manifest::default();

        let entries = sync_copied_dir(&src, &dst, &mut manifest, Some("tw-"), None, true, false)
            .expect("expected value");

        assert!(
            entries
                .iter()
                .any(|e| e.path.ends_with(Path::new("tw-commit/SKILL.md")))
        );
        assert!(dst.join("tw-commit/SKILL.md").exists());
        assert!(manifest.files.contains_key("tw-commit/SKILL.md"));
    }

    #[test]
    fn test_sync_copied_dir_prefix_stale_cleanup() {
        let (_tmp, src, dst) = setup();
        fs::create_dir_all(src.join("commit")).expect("expected value");
        fs::write(src.join("commit/SKILL.md"), "body").expect("expected value");
        let mut manifest = Manifest::default();

        sync_copied_dir(&src, &dst, &mut manifest, Some("tw-"), None, true, false)
            .expect("expected value");
        assert!(dst.join("tw-commit/SKILL.md").exists());

        fs::remove_file(src.join("commit/SKILL.md")).expect("expected value");
        fs::remove_dir(src.join("commit")).expect("expected value");

        sync_copied_dir(&src, &dst, &mut manifest, Some("tw-"), None, true, false)
            .expect("expected value");

        assert!(!dst.join("tw-commit/SKILL.md").exists());
        assert!(!manifest.files.contains_key("tw-commit/SKILL.md"));
    }

    #[test]
    fn test_apply_prefix_name_adds_prefix_to_skill() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skill_dir = tmp.path().join("skill-foo");
        fs::create_dir_all(&skill_dir).expect("expected value");
        let skill_file = skill_dir.join("SKILL.md");
        fs::write(&skill_file, "---\nname: commit\n---\n\nbody\n").expect("expected value");

        apply_prefix_name(tmp.path(), "tw", false).expect("expected value");

        let content = fs::read_to_string(&skill_file).expect("expected value");
        assert!(content.contains("name: tw:commit"));
    }

    #[test]
    fn test_apply_prefix_name_does_not_modify_body() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skill_dir = tmp.path().join("skill-foo");
        fs::create_dir_all(&skill_dir).expect("expected value");
        let skill_file = skill_dir.join("SKILL.md");
        fs::write(&skill_file, "---\nname: commit\n---\n\nname: body-value\n")
            .expect("expected value");

        apply_prefix_name(tmp.path(), "tw", false).expect("expected value");

        let content = fs::read_to_string(&skill_file).expect("expected value");
        assert!(content.contains("name: tw:commit"));
        assert!(content.contains("name: body-value"));
    }

    #[test]
    fn test_apply_prefix_name_idempotent() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skill_dir = tmp.path().join("skill-foo");
        fs::create_dir_all(&skill_dir).expect("expected value");
        let skill_file = skill_dir.join("SKILL.md");
        fs::write(&skill_file, "---\nname: tw:commit\n---\n\nbody\n").expect("expected value");

        apply_prefix_name(tmp.path(), "tw", false).expect("expected value");
        let first = fs::read_to_string(&skill_file).expect("expected value");
        apply_prefix_name(tmp.path(), "tw", false).expect("expected value");
        let second = fs::read_to_string(&skill_file).expect("expected value");

        assert_eq!(first, second);
    }

    #[test]
    fn test_apply_prefix_name_dry_run() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skill_dir = tmp.path().join("skill-foo");
        fs::create_dir_all(&skill_dir).expect("expected value");
        let skill_file = skill_dir.join("SKILL.md");
        let original = "---\nname: commit\n---\n\nbody\n";
        fs::write(&skill_file, original).expect("expected value");

        apply_prefix_name(tmp.path(), "tw", true).expect("expected value");

        let content = fs::read_to_string(&skill_file).expect("expected value");
        assert_eq!(content, original);
    }

    #[test]
    fn test_apply_prefix_name_flat_agent() {
        let tmp = tempfile::tempdir().expect("expected value");
        let agent_file = tmp.path().join("review-pr.md");
        fs::write(&agent_file, "---\nname: review-pr\n---\n\nbody\n").expect("expected value");

        apply_prefix_name(tmp.path(), "tw", false).expect("expected value");

        let content = fs::read_to_string(&agent_file).expect("expected value");
        assert!(content.contains("name: tw:review-pr"));
    }
}
