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
/// Uses `WriteMode::CleanSlate` — agentspec owns the output directory entirely, so each
/// provider subdirectory is deleted and rewritten from scratch on every compile.
pub fn compile_plan(
    result: &CompileResult,
    output_dir: &Path,
    providers: &[Provider],
) -> WritePlan {
    let writes = providers
        .iter()
        .copied()
        .map(|provider| {
            let files: Vec<_> = result.files_for(provider).cloned().collect();
            FileWrite {
                provider,
                kind: None,
                destination: output_dir.join(provider.to_string()),
                files,
                mode: WriteMode::CleanSlate,
                overwrite: true,
            }
        })
        .collect();
    WritePlan {
        writes,
        post_write_hooks: vec![],
    }
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
    /// fresh-config path. Both the live and dry-run flows share the same
    /// gate — the user-visible "[dry-run] " prefix is set elsewhere.
    pub fn print_summary(&self) {
        if self.user_entries_remaining == 0 {
            return;
        }
        let entry_word = if self.user_entries_remaining == 1 {
            "entry"
        } else {
            "entries"
        };
        eprintln!(
            "{count} user-authored {entry_word} remain in {path}",
            count = self.user_entries_remaining,
            path = self.host_path.display(),
        );
    }
}

/// A complete write plan: files to write followed by post-write hooks to run.
///
/// Hooks always run after all writes (e.g. `OpenCode` patching needs rule files
/// to exist first).
#[derive(Debug)]
pub struct WritePlan {
    pub writes: Vec<FileWrite>,
    pub post_write_hooks: Vec<Box<dyn PostWriteHook>>,
}

/// How the executor treats the destination directory.
#[derive(Debug)]
pub enum WriteMode {
    /// agentspec owns this directory exclusively — delete it and rewrite from scratch.
    /// Safe only for directories like `generated/` that agentspec controls entirely.
    CleanSlate,
    /// This directory may contain files agentspec does not own (e.g. user-created skills).
    /// Only create, update, or remove files tracked in the manifest.
    ManifestTracked,
    /// Reverses `ManifestTracked`: read the manifest, delete every file it
    /// tracks, delete the manifest itself, then rmdir the dest dir if empty.
    /// `FileWrite.files` and `FileWrite.overwrite` are unused for this variant.
    Remove,
}

/// A batch of files to write to a single destination directory.
#[derive(Debug)]
pub struct FileWrite {
    pub provider: Provider,
    /// Present for `ManifestTracked` writes (sync); `None` for `CleanSlate` (compile).
    pub kind: Option<FileKind>,
    pub destination: PathBuf,
    pub files: Vec<GeneratedFile>,
    pub mode: WriteMode,
    pub overwrite: bool,
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
        let plan = WritePlan {
            writes: vec![FileWrite {
                provider: Provider::Claude,
                kind: None,
                destination: PathBuf::from("/tmp/test"),
                files: vec![],
                mode: WriteMode::CleanSlate,
                overwrite: false,
            }],
            post_write_hooks: vec![],
        };
        assert_eq!(plan.writes.len(), 1);
        assert!(plan.post_write_hooks.is_empty());
    }
}
