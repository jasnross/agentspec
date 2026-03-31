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
}

impl FileKind {
    /// Directory name used under provider config dirs (e.g. `"agents"`, `"skills"`).
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Agents => "agents",
            Self::Commands => "commands",
            Self::Rules => "rules",
            Self::Skills => "skills",
        }
    }

    /// Returns all variants. Used for invariant checks (e.g. `files_for_kind`).
    pub fn all() -> &'static [Self] {
        &[Self::Agents, Self::Commands, Self::Rules, Self::Skills]
    }
}

/// Returns the file kinds generated for a given provider.
pub fn file_kinds(provider: Provider) -> Vec<FileKind> {
    match provider {
        Provider::Claude => vec![FileKind::Agents, FileKind::Rules, FileKind::Skills],
        Provider::Cursor => vec![FileKind::Rules, FileKind::Skills],
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
                destination: output_dir.join(provider.to_string()),
                files,
                mode: WriteMode::CleanSlate,
                allow_overwrite: true,
                file_prefix: None,
                name_prefix: None,
                strip_name: false,
            }
        })
        .collect();
    WritePlan {
        writes,
        patches: vec![],
    }
}

/// Returns the source directory within the generated output for a provider/kind pair.
pub fn generated_source_dir(provider: Provider, kind: FileKind, generated_root: &Path) -> PathBuf {
    generated_root
        .join(provider.to_string())
        .join(kind.dir_name())
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

/// Which `name:` frontmatter field format to use when applying a namespace prefix.
///
/// Previously defined in `sync/strategy.rs`; moved here so library consumers can
/// reference it in plan types without depending on binary-only modules.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NamePrefixMode {
    Agents,
    Skills,
}

/// A complete write plan: files to write followed by config patches to apply.
///
/// Patches always run after all writes (e.g. `OpenCode` patching needs rule files
/// to exist first), so two separate `Vec`s replace a single `Vec<Op>` enum.
pub struct WritePlan {
    pub writes: Vec<FileWrite>,
    pub patches: Vec<ConfigPatch>,
}

/// How the executor treats the destination directory.
pub enum WriteMode {
    /// agentspec owns this directory exclusively — delete it and rewrite from scratch.
    /// Safe only for directories like `generated/` that agentspec controls entirely.
    CleanSlate,
    /// This directory may contain files agentspec does not own (e.g. user-created skills).
    /// Only create, update, or remove files tracked in the manifest.
    ManifestTracked,
}

/// A batch of files to write to a single destination directory.
///
/// `kind` is intentionally absent: by the time a `FileWrite` is constructed,
/// the kind has already been compiled away into `destination` (resolved path)
/// and `files` (filtered set). The executor needs neither.
pub struct FileWrite {
    pub provider: Provider,
    pub destination: PathBuf,
    pub files: Vec<GeneratedFile>,
    pub mode: WriteMode,
    pub allow_overwrite: bool,
    pub file_prefix: Option<String>,
    pub name_prefix: Option<(String, NamePrefixMode)>,
    pub strip_name: bool,
}

/// A post-write config file patch.
pub enum ConfigPatch {
    /// Patch `opencode.json` `instructions` array with absolute paths to synced rule files.
    OpenCodeInstructions {
        rules_dest_dir: PathBuf,
        config_dir: PathBuf,
    },
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
        assert!(!kinds.contains(&FileKind::Commands));
    }

    #[test]
    fn test_file_kinds_opencode_all_four() {
        let kinds = file_kinds(Provider::OpenCode);
        assert!(kinds.contains(&FileKind::Agents));
        assert!(kinds.contains(&FileKind::Commands));
        assert!(kinds.contains(&FileKind::Rules));
        assert!(kinds.contains(&FileKind::Skills));
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
    fn test_plan_types_construct() {
        let plan = WritePlan {
            writes: vec![FileWrite {
                provider: Provider::Claude,
                destination: PathBuf::from("/tmp/test"),
                files: vec![],
                mode: WriteMode::CleanSlate,
                allow_overwrite: false,
                file_prefix: None,
                name_prefix: None,
                strip_name: false,
            }],
            patches: vec![ConfigPatch::OpenCodeInstructions {
                rules_dest_dir: PathBuf::from("/tmp/rules"),
                config_dir: PathBuf::from("/tmp/config"),
            }],
        };
        assert_eq!(plan.writes.len(), 1);
        assert_eq!(plan.patches.len(), 1);
    }
}
