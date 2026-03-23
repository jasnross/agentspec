use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Cursor,
    Codex,
    #[serde(rename = "opencode")]
    #[value(name = "opencode")]
    OpenCode,
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Provider::Claude => write!(f, "claude"),
            Provider::Cursor => write!(f, "cursor"),
            Provider::Codex => write!(f, "codex"),
            Provider::OpenCode => write!(f, "opencode"),
        }
    }
}

impl Provider {
    pub const ALL: [Provider; 4] = [
        Provider::Claude,
        Provider::Cursor,
        Provider::Codex,
        Provider::OpenCode,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecKind {
    Agent,
    Skill,
    Rule,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // fields read in later phases
pub struct CanonicalSpec {
    /// Absolute path to the spec file
    pub path: PathBuf,
    /// Raw parsed frontmatter as JSON value (for schema validation)
    pub fm: serde_json::Value,
    /// Spec body (Markdown content after frontmatter)
    pub body: String,
    /// Whether this is an agent, skill, or rule spec
    pub kind: SpecKind,
    /// Non-Markdown files bundled with a skill (empty for agents)
    pub supporting_files: Vec<SupportingFile>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // fields read in later phases
pub struct SupportingFile {
    /// Path relative to the skill directory (e.g., "gh-safe.sh")
    pub relative_path: PathBuf,
    /// Raw file content
    pub content: Vec<u8>,
    /// Whether the file has executable permission
    pub executable: bool,
}

/// A spec after normalization: all fields are guaranteed present with defaults applied.
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields read in later phases
pub struct NormalizedSpec {
    /// Absolute path to the source spec file
    pub source_path: PathBuf,
    /// Spec identifier (kebab-case)
    pub id: String,
    /// Agent, Skill, or Rule
    pub kind: SpecKind,
    /// Glob patterns that scope when a rule is applied (rules only; `None` = unconditional)
    pub paths: Option<Vec<String>>,
    /// Display name (defaults to `id` if absent in frontmatter)
    pub name: String,
    /// One-line description
    pub description: String,
    /// Schema version
    pub version: i64,
    /// Whether users can invoke this spec directly
    pub user_invocable: bool,
    /// Whether agents can invoke this spec
    pub agent_invocable: bool,
    /// Instruction body (Markdown, after fragment resolution)
    pub body: String,
    /// Execution configuration
    pub execution: Execution,
    /// Canonical tool IDs, deduplicated and sorted
    pub tools: Vec<String>,
    /// Skill-specific metadata (only for skills)
    pub skill: Option<SkillMeta>,
    /// Non-Markdown files bundled with a skill
    pub supporting_files: Vec<SupportingFile>,
    /// Target providers this spec applies to
    pub targets: Vec<Provider>,
    /// Per-provider override bags
    pub provider_overrides: HashMap<String, serde_json::Value>,
    /// Routing configuration
    pub routing: Option<Routing>,
}

/// Execution configuration from frontmatter.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // fields read in later phases
pub struct Execution {
    pub preset: Option<String>,
    pub temperature: Option<f64>,
    pub mode: Option<String>,
    pub readonly: Option<bool>,
    pub background: Option<bool>,
}

/// Skill-specific metadata.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // fields read in later phases
pub struct SkillMeta {
    pub accepts_args: Option<bool>,
    pub args_schema: Option<String>,
    pub delegate_to: Option<String>,
}

/// Routing configuration.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // fields read in later phases
pub struct Routing {
    pub trigger: Option<String>,
    pub aliases: Vec<String>,
}

// ---------------------------------------------------------------------------
// Preset types
// ---------------------------------------------------------------------------

/// Resolved model presets: preset name → per-provider model config (string or object).
///
/// String shorthand (e.g., `"opus"`) or object form (`{model, variant, reasoning_effort}`).
/// Produced by `AgentspecConfig::resolve_presets` and consumed by adapters.
pub type PresetsMap = HashMap<String, HashMap<String, serde_json::Value>>;

/// Resolved model configuration for a specific provider.
#[derive(Debug, Clone, Default)]
pub struct ModelConfig {
    pub model: Option<String>,
    pub variant: Option<String>,
    pub reasoning_effort: Option<String>,
}

// ---------------------------------------------------------------------------
// Compilation output types
// ---------------------------------------------------------------------------

/// A single file produced by a provider adapter.
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields read in Phase 7 (emit)
pub struct GeneratedFile {
    /// Provider that produced this file
    pub provider: Provider,
    /// Relative path from project root (e.g., "generated/claude/skills/commit/SKILL.md")
    pub path: PathBuf,
    /// File content
    pub content: Vec<u8>,
    /// Optional file mode (e.g., 0o755 for executable scripts)
    pub mode: Option<u32>,
}

impl GeneratedFile {
    /// Create a text file with no special permissions.
    pub fn text(provider: Provider, path: impl AsRef<Path>, content: String) -> Self {
        Self {
            provider,
            path: path.as_ref().to_path_buf(),
            content: content.into_bytes(),
            mode: None,
        }
    }

    /// Create a binary file, optionally with a mode.
    pub fn binary(
        provider: Provider,
        path: impl AsRef<Path>,
        content: Vec<u8>,
        mode: Option<u32>,
    ) -> Self {
        Self {
            provider,
            path: path.as_ref().to_path_buf(),
            content,
            mode,
        }
    }
}

/// A warning emitted during compilation (non-fatal).
#[derive(Debug, Clone)]
pub struct CompileWarning {
    pub code: WarnKind,
    pub provider: Provider,
    pub spec_id: String,
    pub field: String,
    pub message: String,
}

impl fmt::Display for CompileWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {}/{} ({}): {}",
            self.code, self.provider, self.spec_id, self.field, self.message
        )
    }
}

// ---------------------------------------------------------------------------
// Sync types
// ---------------------------------------------------------------------------

/// Where synced files should be placed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    /// Sync to user-level config dirs (`~/.claude/`, `~/.config/opencode/`, etc.)
    #[default]
    User,
    /// Sync to project-local config dirs (`.claude/`, `.cursor/`, etc.)
    Project,
    /// Sync to explicit paths specified per-kind in `SyncTargetConfig`
    Path,
}

/// How files are distributed from `generated/` to the destination.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum SyncStrategy {
    /// Create symlinks from destination into `generated/`
    #[default]
    Symlink,
    /// Copy files and track ownership via `.agentspec-manifest.json`
    Copy,
}

/// Warning codes emitted during compilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarnKind {
    MissingMapping,
}

impl fmt::Display for WarnKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WarnKind::MissingMapping => write!(f, "MISSING_MAPPING"),
        }
    }
}

/// Result of compiling all specs for all target providers.
#[derive(Debug)]
pub struct CompileResult {
    pub files: Vec<GeneratedFile>,
    pub warnings: Vec<CompileWarning>,
}
