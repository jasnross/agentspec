use std::path::{Path, PathBuf};

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
}

impl FileKind {
    /// Directory name used under provider config dirs (e.g. `"agents"`, `"skills"`).
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Agents => "agents",
            Self::Commands => "commands",
            Self::Rules => "rules",
            Self::Skills => "skills",
            Self::Hooks => "hooks",
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
        ]
    }
}

impl std::fmt::Display for FileKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.dir_name())
    }
}

/// Returns the file kinds generated for a given provider.
///
/// `Hooks` is included only for `Claude` and `Cursor`; `OpenCode` does not
/// receive hook output in v1 (the hook-skip warning is surfaced separately
/// via compile-time diagnostics).
pub fn file_kinds(provider: Provider) -> Vec<FileKind> {
    match provider {
        Provider::Claude | Provider::Cursor => vec![
            FileKind::Agents,
            FileKind::Rules,
            FileKind::Skills,
            FileKind::Hooks,
        ],
        Provider::OpenCode => vec![
            FileKind::Agents,
            FileKind::Commands,
            FileKind::Rules,
            FileKind::Skills,
        ],
    }
}

// ── Destination resolution helpers ──────────────────────────────────────────

/// Returns the user-level destination directory for a provider/kind pair
/// (e.g. `~/.claude/agents`, `~/.config/opencode/skills`).
pub fn user_dest_dir(provider: Provider, kind: FileKind, home: &Path) -> PathBuf {
    match provider {
        Provider::Claude => home.join(".claude").join(kind.dir_name()),
        Provider::Cursor => home.join(".cursor").join(kind.dir_name()),
        Provider::OpenCode => home.join(".config").join("opencode").join(kind.dir_name()),
    }
}

/// Returns the project-local destination directory for a provider/kind pair.
pub fn project_dest_dir(provider: Provider, kind: FileKind, cwd: &Path) -> PathBuf {
    let tool_dir = match provider {
        Provider::Claude => ".claude",
        Provider::Cursor => ".cursor",
        Provider::OpenCode => ".opencode",
    };
    cwd.join(tool_dir).join(kind.dir_name())
}

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

// ── Plan types ──────────────────────────────────────────────────────────────

/// A post-write action that runs after all file writes complete.
///
/// Hooks capture their own context (paths, config) when constructed.
/// Emit calls `run(dry_run)` without knowing what the hook does.
pub trait PostWriteHook: std::fmt::Debug {
    fn run(&self, dry_run: bool) -> anyhow::Result<()>;
}

/// Outcome of a per-provider remove patch.
///
/// Currently produced by Claude's settings tidy and Cursor's hooks tidy
/// (Phase 3). `OpenCode`'s instructions filter joins in Phase 4 and consumes
/// the same shape. The count is informational; callers use it to decide
/// whether to print a summary line.
#[derive(Debug, Default)]
pub struct RemovePatchReport {
    pub host_path: PathBuf,
    pub user_entries_remaining: usize,
}

impl RemovePatchReport {
    /// Prints "`M` user-authored entr{y|ies} remain in `<host_path>`" to
    /// stderr when `user_entries_remaining > 0`. Suppressed otherwise to
    /// avoid noisy "0 user-authored entries remain" lines on the common
    /// fresh-config path. Under `dry_run` the line is prefixed with
    /// `[dry-run] ` so it's distinguishable in piped stderr.
    pub fn print_summary(&self, dry_run: bool) {
        if self.user_entries_remaining == 0 {
            return;
        }
        let entry_word = if self.user_entries_remaining == 1 {
            "entry"
        } else {
            "entries"
        };
        let prefix = if dry_run { "[dry-run] " } else { "" };
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
/// hooks (e.g. `OpenCode` instructions patching, Claude/Cursor settings merge).
#[derive(Debug)]
pub struct SyncPlan {
    pub writes: Vec<ManifestTrackedWrite>,
    pub post_write_hooks: Vec<Box<dyn PostWriteHook>>,
}

/// Plan for the `remove` pipeline: manifest-driven removals followed by post-write
/// hooks (e.g. Claude/Cursor settings tidy, `OpenCode` instructions filter).
#[derive(Debug)]
pub struct RemovePlan {
    pub writes: Vec<RemoveWrite>,
    pub post_write_hooks: Vec<Box<dyn PostWriteHook>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_kinds_claude() {
        let kinds = file_kinds(Provider::Claude);
        assert!(kinds.contains(&FileKind::Agents));
        assert!(kinds.contains(&FileKind::Rules));
        assert!(kinds.contains(&FileKind::Skills));
        assert!(kinds.contains(&FileKind::Hooks));
        assert!(!kinds.contains(&FileKind::Commands));
    }

    #[test]
    fn test_file_kinds_opencode_excludes_hooks() {
        let kinds = file_kinds(Provider::OpenCode);
        assert!(kinds.contains(&FileKind::Agents));
        assert!(kinds.contains(&FileKind::Commands));
        assert!(kinds.contains(&FileKind::Rules));
        assert!(kinds.contains(&FileKind::Skills));
        assert!(
            !kinds.contains(&FileKind::Hooks),
            "OpenCode does not receive hook output in v1"
        );
    }

    #[test]
    fn test_file_kinds_cursor_includes_hooks() {
        let kinds = file_kinds(Provider::Cursor);
        assert!(kinds.contains(&FileKind::Hooks));
    }

    #[test]
    fn test_file_kind_hooks_dir_name() {
        assert_eq!(FileKind::Hooks.dir_name(), "hooks");
    }

    #[test]
    fn test_file_kind_all_includes_hooks() {
        assert!(FileKind::all().contains(&FileKind::Hooks));
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
    fn test_user_dest_dir_claude_agents() {
        let result = user_dest_dir(Provider::Claude, FileKind::Agents, Path::new("/home/user"));
        assert_eq!(result, PathBuf::from("/home/user/.claude/agents"));
    }

    #[test]
    fn test_project_dest_dir_cursor_skills() {
        let result = project_dest_dir(
            Provider::Cursor,
            FileKind::Skills,
            Path::new("/work/project"),
        );
        assert_eq!(result, PathBuf::from("/work/project/.cursor/skills"));
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
            post_write_hooks: vec![],
        };
        assert_eq!(sync.writes.len(), 1);
        assert_eq!(sync.writes[0].kind, FileKind::Skills);
        assert!(sync.writes[0].overwrite);
        assert!(sync.post_write_hooks.is_empty());

        let remove = RemovePlan {
            writes: vec![RemoveWrite {
                provider: Provider::OpenCode,
                kind: FileKind::Rules,
                destination: PathBuf::from("/tmp/remove"),
            }],
            post_write_hooks: vec![],
        };
        assert_eq!(remove.writes.len(), 1);
        assert_eq!(remove.writes[0].kind, FileKind::Rules);
        assert!(remove.post_write_hooks.is_empty());
    }
}
