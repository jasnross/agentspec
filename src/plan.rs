use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::compile::{CompileResult, GeneratedFile};
use crate::provider::Provider;

// ── FileKind ────────────────────────────────────────────────────────────────

/// Output kinds agentspec distributes, mirroring the directory layout
/// under each provider's configuration tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileKind {
    Agents,
    Commands,
    Rules,
    Skills,
    Hooks,
    /// Provider plugin manifest (e.g., `.claude-plugin/plugin.json`,
    /// `.cursor-plugin/plugin.json`). The directory name is provider-specific
    /// and resolved at dispatch time via `Adapter::plugin_manifest_dir()`,
    /// not via [`FileKind::dir_name`].
    PluginManifest,
}

impl FileKind {
    /// Human-readable static label for diagnostics and report columns.
    ///
    /// Returns a `&'static str` for every variant (using `"plugin-manifest"`
    /// for [`FileKind::PluginManifest`]), so callers that just want a label
    /// don't need to handle `None`. **Not for filesystem dispatch** — use
    /// [`crate::adapters::Adapter::dir_for_kind`] for that, since the
    /// `PluginManifest` directory is provider-specific.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Agents => "agents",
            Self::Commands => "commands",
            Self::Rules => "rules",
            Self::Skills => "skills",
            Self::Hooks => "hooks",
            Self::PluginManifest => "plugin-manifest",
        }
    }

    /// Returns all variants. Used for invariant checks (e.g. `files_for_kind`).
    pub fn all() -> &'static [Self] {
        &[
            Self::Agents,
            Self::Commands,
            Self::Rules,
            Self::Skills,
            Self::Hooks,
            Self::PluginManifest,
        ]
    }
}

impl std::fmt::Display for FileKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

// Per-provider sync destination roots are computed by `Adapter::compile`
// (forward path) and `Adapter::removal_patches` (reverse path); `sync_plan`
// reads them from `CompileResult.dest_roots`, `remove_plan` reads them from
// `RemovalOutput.dest_root`. Non-adapter modules never call adapter path
// methods directly.

/// Builds a plan that writes compiled files to an output directory (e.g. `generated/`).
///
/// Each `CleanSlateWrite` declares that agentspec owns its destination entirely — every
/// provider subdirectory is deleted and rewritten from scratch on every compile.
pub fn compile_plan(
    result: &CompileResult,
    output_dir: &Path,
    providers: &[Provider],
) -> CompilePlan {
    let writes = providers
        .iter()
        .copied()
        .map(|provider| {
            let files: Vec<_> = result.files_for(provider).cloned().collect();
            CleanSlateWrite {
                provider,
                destination: output_dir.join(provider.to_string()),
                files,
            }
        })
        .collect();
    CompilePlan { writes }
}

/// Expands a leading `~/` to the home directory. Returns the path unchanged otherwise.
pub fn expand_tilde(path: &str, home: &Path) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// Tries `fs::remove_dir(dir)`; returns `Ok(true)` on success, `Ok(false)` when
/// the dir is non-empty or already missing. Any other I/O error propagates with
/// context.
pub fn try_rmdir_if_empty(dir: &Path) -> anyhow::Result<bool> {
    match std::fs::remove_dir(dir) {
        Ok(()) => Ok(true),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::DirectoryNotEmpty | std::io::ErrorKind::NotFound
            ) =>
        {
            Ok(false)
        }
        Err(e) => Err(e).with_context(|| format!("failed to rmdir {}", dir.display())),
    }
}

/// Deletes `host_path` and best-effort `rmdir`'s its parent directory.
/// Returns whether the parent was successfully `rmdir`'d.
///
/// Pure I/O: this helper does not write to stderr. User-facing messaging
/// for the delete-on-empty branch lives in [`RemovePatchReport::print_summary`]
/// so there's a single source of truth for the wording across dry-run and
/// live runs.
///
/// Under `dry_run` no filesystem changes occur; instead the function predicts
/// whether the parent rmdir would succeed (only the host file remains in it).
/// The empty-iterator default of `Iterator::all` correctly returns `true` for
/// a directory whose only entry is the host file we're about to "remove" —
/// `iter` becomes empty after we reject everything that isn't the host path,
/// matching the live-mode order of `remove_file` then `rmdir`.
///
/// This is the shared "delete-on-empty" tail used by every adapter's remove
/// post-write hook (Claude/Cursor via `hooks_merge::tidy_jsonc_file`;
/// `OpenCode` via `adapters::opencode::remove_opencode_instructions`).
pub fn delete_host_file_and_rmdir_parent(host_path: &Path, dry_run: bool) -> anyhow::Result<bool> {
    let parent = host_path.parent();
    if dry_run {
        let predicted_parent_rmdir = parent.is_some_and(|p| {
            std::fs::read_dir(p)
                .is_ok_and(|mut iter| iter.all(|entry| entry.is_ok_and(|e| e.path() == host_path)))
        });
        return Ok(predicted_parent_rmdir);
    }
    std::fs::remove_file(host_path)
        .with_context(|| format!("failed to remove empty host file {}", host_path.display()))?;
    match parent {
        Some(p) => try_rmdir_if_empty(p),
        None => Ok(false),
    }
}

// ── Plan types ──────────────────────────────────────────────────────────────

/// Forward-direction post-write patch (sync pipeline).
///
/// Applies agentspec-owned entries into a host config file (e.g., merging
/// hook entries into `settings.json`). The sync pipeline constructs these
/// via `Adapter::compile` and runs them after manifest-tracked file writes.
///
/// `Send + Sync` future-proofs the trait for parallel emit.
pub trait ForwardPatch: std::fmt::Debug + Send + Sync {
    fn run(&self, dry_run: bool) -> anyhow::Result<()>;
}

/// Reverse-direction post-write patch (remove/prune pipeline).
///
/// Strips agentspec-owned entries identified by on-disk `_agentspec_id`
/// sentinels from a host config file. The remove pipeline constructs these
/// via `Adapter::removal_patches` and runs them after manifest-tracked
/// file deletions.
///
/// `Send + Sync` future-proofs the trait for parallel emit.
pub trait ReversePatch: std::fmt::Debug + Send + Sync {
    fn run_remove(&self, dry_run: bool) -> anyhow::Result<()>;
}

/// Outcome of a per-provider remove patch.
///
/// Produced by Claude's settings tidy, Cursor's hooks tidy, and `OpenCode`'s
/// instructions filter — all three consume the same shape. The counts are
/// informational; callers use them to decide whether to print a summary line.
/// `host_file_deleted` and `parent_rmdir` capture the new "delete-on-empty"
/// outcomes added in 0.x — when the patch removes any owned content **and**
/// the residual file is effectively empty per the provider's predicate, the
/// host file is deleted and the parent directory is best-effort `rmdir`'d.
#[derive(Debug, Default)]
pub struct RemovePatchReport {
    pub host_path: PathBuf,
    pub user_entries_remaining: usize,
    pub host_file_deleted: bool,
    pub parent_rmdir: bool,
}

impl RemovePatchReport {
    /// Prints a one-line summary to stderr based on what happened.
    ///
    /// - `host_file_deleted` → `"removed empty <host>; rmdir'd <parent>"` (or
    ///   `"removed empty <host>; kept parent <parent> (non-empty)"` if the
    ///   parent had other content). Under `dry_run` the verbs become
    ///   `"would remove empty"` and `"would rmdir"` / `"would keep parent"`.
    /// - `user_entries_remaining > 0` → `"M user-authored entr{y|ies} remain in <host>"`.
    /// - Both zero/false → suppressed (avoids noisy "0 entries remain" lines on the common fresh-config path).
    ///
    /// Under `dry_run` every line is prefixed with `[dry-run] ` so it's
    /// distinguishable in piped stderr.
    ///
    /// The first two outputs are mutually exclusive in practice — a delete
    /// only fires when no user entries remain — but a `debug_assert!` pins
    /// the invariant so a future refactor can't silently drift.
    ///
    /// This is the **single source of truth** for delete-on-empty messaging.
    /// `delete_host_file_and_rmdir_parent` is a pure I/O helper and emits no
    /// output of its own; both dry-run and live verbs are produced here so
    /// users see one consistent line per remove event.
    pub fn print_summary(&self, dry_run: bool) {
        debug_assert!(
            !(self.host_file_deleted && self.user_entries_remaining > 0),
            "host_file_deleted should never coexist with surviving user entries; \
             saw host_file_deleted=true and user_entries_remaining={}",
            self.user_entries_remaining,
        );
        let prefix = if dry_run { "[dry-run] " } else { "" };

        if self.host_file_deleted {
            let remove_verb = if dry_run { "would remove" } else { "removed" };
            let parent_clause = match (self.host_path.parent(), self.parent_rmdir) {
                (Some(parent), true) => {
                    let rmdir_verb = if dry_run { "would rmdir" } else { "rmdir'd" };
                    format!("; {rmdir_verb} {}", parent.display())
                }
                (Some(parent), false) => {
                    let keep_verb = if dry_run { "would keep" } else { "kept" };
                    format!("; {keep_verb} parent {} (non-empty)", parent.display())
                }
                (None, _) => String::new(),
            };
            eprintln!(
                "{prefix}{remove_verb} empty {path}{parent_clause}",
                path = self.host_path.display(),
            );
            return;
        }

        if self.user_entries_remaining == 0 {
            return;
        }
        let entry_word = if self.user_entries_remaining == 1 {
            "entry"
        } else {
            "entries"
        };
        eprintln!(
            "{prefix}{count} user-authored {entry_word} remain in {path}",
            count = self.user_entries_remaining,
            path = self.host_path.display(),
        );
    }
}

/// A batch of files for a destination agentspec owns exclusively (the `compile`
/// pipeline). The destination is deleted and rewritten from scratch on every emit.
#[derive(Debug)]
pub struct CleanSlateWrite {
    pub provider: Provider,
    pub destination: PathBuf,
    pub files: Vec<GeneratedFile>,
}

/// A batch of files for a destination shared with the user (the `sync` pipeline).
/// Only files tracked in the manifest are created, updated, or pruned; collisions
/// with user-authored content honour `overwrite`.
#[derive(Debug)]
pub struct ManifestTrackedWrite {
    pub provider: Provider,
    pub kind: FileKind,
    pub destination: PathBuf,
    pub files: Vec<GeneratedFile>,
    pub overwrite: bool,
}

/// A batch description for the `remove` pipeline. The manifest at
/// `destination/.agentspec-manifest.json` is the source of truth at execution time —
/// no file content is carried because every tracked file is deleted.
#[derive(Debug)]
pub struct RemoveWrite {
    pub provider: Provider,
    pub kind: FileKind,
    pub destination: PathBuf,
}

/// Plan for the `compile` pipeline: clean-slate writes only, no post-write hooks.
#[derive(Debug)]
pub struct CompilePlan {
    pub writes: Vec<CleanSlateWrite>,
}

/// Plan for the `sync` pipeline: manifest-tracked writes followed by post-write
/// patches (e.g. `OpenCode` instructions patching, Claude/Cursor settings merge).
#[derive(Debug)]
pub struct SyncPlan {
    pub writes: Vec<ManifestTrackedWrite>,
    pub post_write_patches: Vec<Box<dyn ForwardPatch>>,
}

/// Plan for the `remove` pipeline: manifest-driven removals followed by
/// post-write patches (e.g. Claude/Cursor settings tidy, `OpenCode`
/// instructions filter).
#[derive(Debug)]
pub struct RemovePlan {
    pub writes: Vec<RemoveWrite>,
    pub post_write_patches: Vec<Box<dyn ReversePatch>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_kind_hooks_display_name() {
        assert_eq!(FileKind::Hooks.display_name(), "hooks");
    }

    #[test]
    fn test_file_kind_plugin_manifest_display_name() {
        assert_eq!(FileKind::PluginManifest.display_name(), "plugin-manifest");
        assert_eq!(FileKind::PluginManifest.to_string(), "plugin-manifest");
    }

    #[test]
    fn test_file_kind_all_includes_hooks() {
        assert!(FileKind::all().contains(&FileKind::Hooks));
    }

    #[test]
    fn test_file_kind_all_includes_plugin_manifest() {
        assert!(FileKind::all().contains(&FileKind::PluginManifest));
    }

    #[test]
    fn test_expand_tilde_replaces_home() {
        let result = expand_tilde("~/foo/bar", Path::new("/home/user"));
        assert_eq!(result, PathBuf::from("/home/user/foo/bar"));
    }

    #[test]
    fn test_expand_tilde_absolute_unchanged() {
        let result = expand_tilde("/absolute/path", Path::new("/home/user"));
        assert_eq!(result, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_file_kind_display() {
        assert_eq!(FileKind::Skills.to_string(), "skills");
        assert_eq!(FileKind::Agents.to_string(), "agents");
        assert_eq!(FileKind::Commands.to_string(), "commands");
        assert_eq!(FileKind::Rules.to_string(), "rules");
    }

    #[test]
    fn test_plan_types_construct() {
        let compile = CompilePlan {
            writes: vec![CleanSlateWrite {
                provider: Provider::Claude,
                destination: PathBuf::from("/tmp/compile"),
                files: vec![],
            }],
        };
        assert_eq!(compile.writes.len(), 1);
        assert_eq!(compile.writes[0].provider, Provider::Claude);
        assert_eq!(compile.writes[0].destination, PathBuf::from("/tmp/compile"));

        let sync = SyncPlan {
            writes: vec![ManifestTrackedWrite {
                provider: Provider::Cursor,
                kind: FileKind::Skills,
                destination: PathBuf::from("/tmp/sync"),
                files: vec![],
                overwrite: true,
            }],
            post_write_patches: vec![],
        };
        assert_eq!(sync.writes.len(), 1);
        assert_eq!(sync.writes[0].kind, FileKind::Skills);
        assert!(sync.writes[0].overwrite);
        assert!(sync.post_write_patches.is_empty());

        let remove = RemovePlan {
            writes: vec![RemoveWrite {
                provider: Provider::OpenCode,
                kind: FileKind::Rules,
                destination: PathBuf::from("/tmp/remove"),
            }],
            post_write_patches: vec![],
        };
        assert_eq!(remove.writes.len(), 1);
        assert_eq!(remove.writes[0].kind, FileKind::Rules);
        assert!(remove.post_write_patches.is_empty());
    }
}
