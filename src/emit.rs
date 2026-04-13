use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use agentspec::plan::{FileKind, FileWrite, WriteMode, WritePlan};
use agentspec::provider::Provider;
use anyhow::{Context, Result, bail};

use crate::sync::manifest::{Manifest, ManifestEntry};

/// Aggregated sync stats for a single (provider, kind) destination.
pub(crate) struct BatchStats {
    pub provider: Provider,
    pub kind: FileKind,
    pub created: usize,
    pub updated: usize,
    pub removed: usize,
    pub backed_up: usize,
    pub unchanged: usize,
}

impl BatchStats {
    /// Returns true if every action count is zero except unchanged.
    pub(crate) fn is_unchanged_only(&self) -> bool {
        self.created == 0 && self.updated == 0 && self.removed == 0 && self.backed_up == 0
    }
}

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

/// Execute a write plan: write all file batches, then run post-write hooks.
pub fn emit(plan: &WritePlan, dry_run: bool, verbose: bool) -> Result<()> {
    let mut all_stats: Vec<BatchStats> = Vec::new();
    for w in &plan.writes {
        if let Some(stats) = write_batch(w, dry_run)? {
            all_stats.push(stats);
        }
    }
    if !all_stats.is_empty() {
        let mut stderr = std::io::stderr();
        // Stderr write failures are not actionable — don't fail the sync for them.
        let _ = render_sync_report(&mut stderr, &all_stats, dry_run, verbose);
    }
    for hook in &plan.post_write_hooks {
        hook.run(dry_run)?;
    }
    Ok(())
}

/// A column definition for the sync report table.
struct ReportColumn {
    header: &'static str,
    dry_header: &'static str,
    extract: fn(&BatchStats) -> usize,
    /// True for the "Unchanged" column, which is only shown in verbose mode.
    verbose_only: bool,
}

/// All possible action columns in sync report order.
const REPORT_COLUMNS: &[ReportColumn] = &[
    ReportColumn {
        header: "Created",
        dry_header: "Would Create",
        extract: |s| s.created,
        verbose_only: false,
    },
    ReportColumn {
        header: "Updated",
        dry_header: "Would Update",
        extract: |s| s.updated,
        verbose_only: false,
    },
    ReportColumn {
        header: "Removed",
        dry_header: "Would Remove",
        extract: |s| s.removed,
        verbose_only: false,
    },
    ReportColumn {
        header: "Backed Up",
        dry_header: "Would Back Up",
        extract: |s| s.backed_up,
        verbose_only: false,
    },
    ReportColumn {
        header: "Unchanged",
        dry_header: "Unchanged",
        extract: |s| s.unchanged,
        verbose_only: true,
    },
];

/// Renders a compact sync report table to the given writer.
///
/// In normal mode, only changed destinations appear and columns with all-zero
/// values are omitted. With `verbose`, all destinations (including unchanged)
/// and all columns are shown. Dry-run mode prefixes column headers with "Would".
fn render_sync_report(
    out: &mut dyn std::io::Write,
    stats: &[BatchStats],
    dry_run: bool,
    verbose: bool,
) -> std::io::Result<()> {
    let changed: Vec<&BatchStats> = stats.iter().filter(|s| !s.is_unchanged_only()).collect();
    let unchanged_count = stats.len() - changed.len();
    let changed_count = changed.len();

    let visible: Vec<&BatchStats> = if verbose {
        stats.iter().collect()
    } else {
        changed
    };

    // Nothing changed — short-circuit with a simple summary.
    if visible.is_empty() {
        let dest = if unchanged_count == 1 {
            "destination"
        } else {
            "destinations"
        };
        writeln!(out, "\n{unchanged_count} {dest} unchanged")?;
        return Ok(());
    }

    // Include a column if any visible row has a non-zero value.
    // Verbose-only columns (like "Unchanged") are hidden unless verbose is on.
    let active_columns: Vec<&ReportColumn> = REPORT_COLUMNS
        .iter()
        .filter(|col| {
            if col.verbose_only && !verbose {
                return false;
            }
            verbose || visible.iter().any(|s| (col.extract)(s) > 0)
        })
        .collect();

    render_table(out, &visible, &active_columns, dry_run)?;
    render_footer(out, changed_count, unchanged_count, dry_run)
}

/// Renders the table header, separator, and data rows.
fn render_table(
    out: &mut dyn std::io::Write,
    rows: &[&BatchStats],
    columns: &[&ReportColumn],
    dry_run: bool,
) -> std::io::Result<()> {
    let provider_w = rows
        .iter()
        .map(|s| s.provider.display_name().len())
        .max()
        .unwrap_or(0)
        .max("Provider".len());

    let kind_w = rows
        .iter()
        .map(|s| s.kind.dir_name().len())
        .max()
        .unwrap_or(0)
        .max("Kind".len());

    let col_widths: Vec<usize> = columns
        .iter()
        .map(|col| {
            let header = if dry_run { col.dry_header } else { col.header };
            let max_val = rows.iter().map(|s| (col.extract)(s)).max().unwrap_or(0);
            let num_w = if max_val == 0 {
                1
            } else {
                max_val.ilog10() as usize + 1
            };
            header.len().max(num_w)
        })
        .collect();

    // Header row.
    writeln!(out)?;
    write!(out, "{:<provider_w$}  {:<kind_w$}", "Provider", "Kind")?;
    for (col, &w) in columns.iter().zip(&col_widths) {
        let header = if dry_run { col.dry_header } else { col.header };
        write!(out, "  {header:>w$}")?;
    }
    writeln!(out)?;

    // Separator row.
    write!(out, "{:\u{2500}<provider_w$}  {:\u{2500}<kind_w$}", "", "")?;
    for &w in &col_widths {
        write!(out, "  {:\u{2500}<w$}", "")?;
    }
    writeln!(out)?;

    // Data rows.
    for s in rows {
        write!(
            out,
            "{:<provider_w$}  {:<kind_w$}",
            s.provider.display_name(),
            s.kind.dir_name()
        )?;
        for (col, &w) in columns.iter().zip(&col_widths) {
            write!(out, "  {:>w$}", (col.extract)(s))?;
        }
        writeln!(out)?;
    }

    Ok(())
}

/// Renders the summary footer line.
fn render_footer(
    out: &mut dyn std::io::Write,
    changed_count: usize,
    unchanged_count: usize,
    dry_run: bool,
) -> std::io::Result<()> {
    let verb = if dry_run { "would change" } else { "changed" };
    let dest = if changed_count == 1 {
        "destination"
    } else {
        "destinations"
    };
    writeln!(out)?;
    if unchanged_count > 0 {
        writeln!(
            out,
            "{changed_count} {dest} {verb}, {unchanged_count} unchanged"
        )
    } else {
        writeln!(out, "{changed_count} {dest} {verb}")
    }
}

fn write_batch(w: &FileWrite, dry_run: bool) -> Result<Option<BatchStats>> {
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
            Ok(None)
        }
        WriteMode::ManifestTracked => {
            let kind = w.kind.context("ManifestTracked writes must have a kind")?;

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
                    w.overwrite,
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

            Ok(Some(BatchStats {
                provider: w.provider,
                kind,
                created: n_created,
                updated: n_updated,
                removed: n_removed,
                backed_up: n_backed_up,
                unchanged: n_unchanged,
            }))
        }
    }
}

/// Writes `content` to `dest` with manifest tracking.
///
/// Behavior:
/// - `rel_path` in manifest AND content same → `Unchanged` (no write)
/// - `rel_path` in manifest AND content differs → overwrite, update manifest
/// - `rel_path` not in manifest AND dest exists AND `overwrite: false` → error
/// - `rel_path` not in manifest AND dest exists AND `overwrite: true` → back up, write, record
/// - dest does not exist → write, record
fn write_content_to_dest(
    content: &[u8],
    dest: &Path,
    rel_path: &str,
    manifest: &mut Manifest,
    mode: Option<u32>,
    overwrite: bool,
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
        if !overwrite {
            bail!(
                "collision: {} exists and is not managed by agentspec; pass --force to overwrite",
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
                kind: None,
                destination: output_dir.join(provider.to_string()),
                files,
                mode: WriteMode::CleanSlate,
                overwrite: true,
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
        emit(&plan, false, false).expect("expected value");

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
        emit(&plan, false, false).expect("expected value");

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
                kind: None,
                destination: output_dir.join("claude"),
                files: vec![GeneratedFile::binary(
                    Provider::Claude,
                    "skills/gh-safe/gh-safe.sh",
                    b"#!/bin/bash\necho hi".to_vec(),
                    Some(0o755),
                )],
                mode: WriteMode::CleanSlate,
                overwrite: true,
            }],
            post_write_hooks: vec![],
        };

        emit(&plan, false, false).expect("expected value");

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
                kind: Some(agentspec::plan::FileKind::Skills),
                destination: dest.clone(),
                files: vec![make_file(
                    Provider::Claude,
                    "skills/basic/SKILL.md",
                    "---\nname: basic\n---\n\nbody\n",
                )],
                mode: WriteMode::ManifestTracked,
                overwrite: true,
            }],
            post_write_hooks: vec![],
        };

        emit(&plan, false, false).expect("expected value");

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
                kind: Some(agentspec::plan::FileKind::Skills),
                destination: dest.clone(),
                files: vec![make_file(Provider::Claude, "skills/basic/SKILL.md", "v1")],
                mode: WriteMode::ManifestTracked,
                overwrite: true,
            }],
            post_write_hooks: vec![],
        };
        emit(&plan, false, false).expect("expected value");
        assert!(dest.join("basic/SKILL.md").exists());

        // Second sync: empty files list → basic/SKILL.md becomes stale
        let plan2 = WritePlan {
            writes: vec![FileWrite {
                provider: Provider::Claude,
                kind: Some(agentspec::plan::FileKind::Skills),
                destination: dest.clone(),
                files: vec![],
                mode: WriteMode::ManifestTracked,
                overwrite: true,
            }],
            post_write_hooks: vec![],
        };
        emit(&plan2, false, false).expect("expected value");

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

    fn manifest_tracked_plan(dest: &Path, files: Vec<GeneratedFile>, overwrite: bool) -> WritePlan {
        WritePlan {
            writes: vec![FileWrite {
                provider: Provider::Claude,
                kind: Some(agentspec::plan::FileKind::Skills),
                destination: dest.to_path_buf(),
                files,
                mode: WriteMode::ManifestTracked,
                overwrite,
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
        emit(&plan, false, false).expect("expected value");

        // Second sync: same content — should be Unchanged (no manifest rewrite needed).
        let plan2 = manifest_tracked_plan(
            &dest,
            vec![make_file(Provider::Claude, "skills/basic/SKILL.md", "body")],
            false,
        );
        emit(&plan2, false, false).expect("expected value");

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
        emit(&plan, false, false).expect("expected value");

        let plan2 = manifest_tracked_plan(
            &dest,
            vec![make_file(Provider::Claude, "skills/basic/SKILL.md", "v2")],
            false,
        );
        emit(&plan2, false, false).expect("expected value");

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
            false, // overwrite = false
        );
        let err = emit(&plan, false, false).expect_err("expected collision error");
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
            true, // overwrite = true
        );
        emit(&plan, false, false).expect("expected value");

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
    fn test_batch_stats_is_unchanged_only() {
        use agentspec::plan::FileKind;

        let unchanged = BatchStats {
            provider: Provider::Claude,
            kind: FileKind::Skills,
            created: 0,
            updated: 0,
            removed: 0,
            backed_up: 0,
            unchanged: 5,
        };
        assert!(unchanged.is_unchanged_only());

        let with_created = BatchStats {
            created: 1,
            ..unchanged
        };
        assert!(!with_created.is_unchanged_only());

        let with_updated = BatchStats {
            created: 0,
            updated: 1,
            ..unchanged
        };
        assert!(!with_updated.is_unchanged_only());

        let with_removed = BatchStats {
            created: 0,
            updated: 0,
            removed: 1,
            ..unchanged
        };
        assert!(!with_removed.is_unchanged_only());

        let with_backed_up = BatchStats {
            created: 0,
            updated: 0,
            removed: 0,
            backed_up: 1,
            ..unchanged
        };
        assert!(!with_backed_up.is_unchanged_only());

        // All zeros including unchanged is also "unchanged only"
        let all_zero = BatchStats {
            unchanged: 0,
            ..unchanged
        };
        assert!(all_zero.is_unchanged_only());
    }

    // ── render_sync_report tests ──────────────────────────────────────────

    fn make_stats(
        provider: Provider,
        kind: agentspec::plan::FileKind,
        created: usize,
        updated: usize,
        removed: usize,
        backed_up: usize,
        unchanged: usize,
    ) -> BatchStats {
        BatchStats {
            provider,
            kind,
            created,
            updated,
            removed,
            backed_up,
            unchanged,
        }
    }

    #[test]
    fn test_render_all_unchanged_non_verbose() {
        use agentspec::plan::FileKind;

        let stats = vec![
            make_stats(Provider::Claude, FileKind::Agents, 0, 0, 0, 0, 8),
            make_stats(Provider::Claude, FileKind::Skills, 0, 0, 0, 0, 30),
        ];
        let mut buf = Vec::new();
        render_sync_report(&mut buf, &stats, false, false).expect("expected value");
        let output = String::from_utf8(buf).expect("expected value");
        assert!(
            output.contains("2 destinations unchanged"),
            "output: {output}"
        );
        // Should NOT contain table headers when nothing changed.
        assert!(!output.contains("Provider"), "output: {output}");
    }

    #[test]
    fn test_render_single_action_type() {
        use agentspec::plan::FileKind;

        let stats = vec![
            make_stats(Provider::Claude, FileKind::Skills, 0, 3, 0, 0, 30),
            make_stats(Provider::Claude, FileKind::Agents, 0, 0, 0, 0, 8),
        ];
        let mut buf = Vec::new();
        render_sync_report(&mut buf, &stats, false, false).expect("expected value");
        let output = String::from_utf8(buf).expect("expected value");
        // Only "Updated" column should appear (not Created, Removed, etc.)
        assert!(output.contains("Updated"), "output: {output}");
        assert!(!output.contains("Created"), "output: {output}");
        assert!(!output.contains("Removed"), "output: {output}");
        assert!(!output.contains("Unchanged"), "output: {output}");
        // Changed row should appear, unchanged row should not.
        assert!(output.contains("skills"), "output: {output}");
        assert!(!output.contains("agents"), "output: {output}");
        assert!(
            output.contains("1 destination changed, 1 unchanged"),
            "output: {output}"
        );
    }

    #[test]
    fn test_render_separator_width_matches_header() {
        use agentspec::plan::FileKind;

        let stats = vec![make_stats(
            Provider::OpenCode,
            FileKind::Commands,
            0,
            1,
            0,
            0,
            5,
        )];
        let mut buf = Vec::new();
        render_sync_report(&mut buf, &stats, false, false).expect("expected value");
        let output = String::from_utf8(buf).expect("expected value");
        let lines: Vec<&str> = output.lines().collect();
        // Header is line 1 (after blank line 0), separator is line 2.
        let header_line = lines[1];
        let sep_line = lines[2];
        // Separator should be same display width as header (both use spaces for
        // column gaps and ─ for fills).
        assert_eq!(
            header_line.chars().count(),
            sep_line.chars().count(),
            "header: {header_line:?}\nsep:    {sep_line:?}"
        );
    }

    #[test]
    fn test_render_mixed_actions() {
        use agentspec::plan::FileKind;

        let stats = vec![make_stats(
            Provider::Claude,
            FileKind::Skills,
            2,
            3,
            1,
            0,
            10,
        )];
        let mut buf = Vec::new();
        render_sync_report(&mut buf, &stats, false, false).expect("expected value");
        let output = String::from_utf8(buf).expect("expected value");
        assert!(output.contains("Created"), "output: {output}");
        assert!(output.contains("Updated"), "output: {output}");
        assert!(output.contains("Removed"), "output: {output}");
        // Backed Up is all zeros — should be omitted.
        assert!(!output.contains("Backed Up"), "output: {output}");
        // Unchanged column omitted in non-verbose mode.
        assert!(!output.contains("Unchanged"), "output: {output}");
    }

    #[test]
    fn test_render_verbose_shows_unchanged() {
        use agentspec::plan::FileKind;

        let stats = vec![
            make_stats(Provider::Claude, FileKind::Skills, 0, 3, 0, 0, 30),
            make_stats(Provider::Claude, FileKind::Agents, 0, 0, 0, 0, 8),
        ];
        let mut buf = Vec::new();
        render_sync_report(&mut buf, &stats, false, true).expect("expected value");
        let output = String::from_utf8(buf).expect("expected value");
        // Both rows should appear in verbose mode.
        assert!(output.contains("skills"), "output: {output}");
        assert!(output.contains("agents"), "output: {output}");
        // Unchanged column should appear.
        assert!(output.contains("Unchanged"), "output: {output}");
    }

    #[test]
    fn test_render_dry_run_headers() {
        use agentspec::plan::FileKind;

        let stats = vec![make_stats(
            Provider::Claude,
            FileKind::Skills,
            0,
            3,
            0,
            0,
            10,
        )];
        let mut buf = Vec::new();
        render_sync_report(&mut buf, &stats, true, false).expect("expected value");
        let output = String::from_utf8(buf).expect("expected value");
        assert!(output.contains("Would Update"), "output: {output}");
        assert!(
            output.contains("would change"),
            "summary should use 'would change': {output}"
        );
    }

    #[test]
    fn test_render_column_width_adapts() {
        use agentspec::plan::FileKind;

        let stats = vec![
            make_stats(Provider::Claude, FileKind::Skills, 0, 1, 0, 0, 0),
            make_stats(Provider::OpenCode, FileKind::Commands, 0, 1, 0, 0, 0),
        ];
        let mut buf = Vec::new();
        render_sync_report(&mut buf, &stats, false, false).expect("expected value");
        let output = String::from_utf8(buf).expect("expected value");
        // "OpenCode" is wider than "Claude" — both should be padded to the same
        // width. The provider column should be at least 8 chars wide.
        assert!(output.contains("Claude  "), "output: {output}");
        assert!(output.contains("OpenCode"), "output: {output}");
    }

    #[test]
    fn test_write_batch_returns_none_for_clean_slate() {
        let tmp = TempDir::new().expect("expected value");
        let w = FileWrite {
            provider: Provider::Claude,
            kind: None,
            destination: tmp.path().join("out"),
            files: vec![],
            mode: WriteMode::CleanSlate,
            overwrite: true,
        };
        let result = write_batch(&w, false).expect("expected value");
        assert!(result.is_none());
    }

    #[test]
    fn test_write_batch_returns_some_for_manifest_tracked() {
        use agentspec::plan::FileKind;

        let tmp = TempDir::new().expect("expected value");
        let dest = tmp.path().join("skills");
        let w = FileWrite {
            provider: Provider::Claude,
            kind: Some(FileKind::Skills),
            destination: dest,
            files: vec![make_file(Provider::Claude, "skills/basic/SKILL.md", "body")],
            mode: WriteMode::ManifestTracked,
            overwrite: true,
        };
        let result = write_batch(&w, false).expect("expected value");
        let stats = result.expect("expected Some(BatchStats)");
        assert_eq!(stats.created, 1);
        assert_eq!(stats.unchanged, 0);
        assert!(matches!(stats.provider, Provider::Claude));
        assert!(matches!(stats.kind, FileKind::Skills));
    }

    #[test]
    fn test_manifest_tracked_dry_run_no_mutations() {
        let tmp = TempDir::new().expect("expected value");
        let dest = tmp.path().join("skills");

        let plan = WritePlan {
            writes: vec![FileWrite {
                provider: Provider::Claude,
                kind: Some(agentspec::plan::FileKind::Skills),
                destination: dest.clone(),
                files: vec![make_file(Provider::Claude, "skills/basic/SKILL.md", "body")],
                mode: WriteMode::ManifestTracked,
                overwrite: true,
            }],
            post_write_hooks: vec![],
        };

        emit(&plan, true, false).expect("expected value");

        assert!(!dest.exists(), "dry-run must not create directory");
    }
}
