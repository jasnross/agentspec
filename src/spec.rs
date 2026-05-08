use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub enum Spec {
    Agent(AgentSpec),
    Skill(SkillSpec),
    Rule(RuleSpec),
    Hook(HookSpec),
}

impl Spec {
    pub fn body(&self) -> &str {
        match self {
            Spec::Agent(agent_spec) => &agent_spec.body,
            Spec::Skill(skill_spec) => &skill_spec.body,
            Spec::Rule(rule_spec) => &rule_spec.body,
            Spec::Hook(hook_spec) => &hook_spec.body,
        }
    }

    pub fn path(&self) -> &Path {
        match self {
            Spec::Agent(agent_spec) => &agent_spec.path,
            Spec::Skill(skill_spec) => &skill_spec.path,
            Spec::Rule(rule_spec) => &rule_spec.path,
            Spec::Hook(hook_spec) => &hook_spec.path,
        }
    }
}

#[derive(Clone, Debug)]
pub enum NormalizedSpec {
    Agent(NormalizedAgentSpec),
    Skill(NormalizedSkillSpec),
    Rule(NormalizedRuleSpec),
    Hook(NormalizedHookSpec),
}

impl NormalizedSpec {
    pub fn id(&self) -> &str {
        match self {
            NormalizedSpec::Agent(agent_spec) => &agent_spec.frontmatter.id,
            NormalizedSpec::Skill(skill_spec) => &skill_spec.frontmatter.id,
            NormalizedSpec::Rule(rule_spec) => &rule_spec.frontmatter.id,
            NormalizedSpec::Hook(hook_spec) => &hook_spec.frontmatter.id,
        }
    }

    pub fn body(&self) -> &str {
        match self {
            NormalizedSpec::Agent(agent_spec) => &agent_spec.body,
            NormalizedSpec::Skill(skill_spec) => &skill_spec.body,
            NormalizedSpec::Rule(rule_spec) => &rule_spec.body,
            NormalizedSpec::Hook(hook_spec) => &hook_spec.body,
        }
    }

    pub fn path(&self) -> &Path {
        match self {
            NormalizedSpec::Agent(agent_spec) => &agent_spec.path,
            NormalizedSpec::Skill(skill_spec) => &skill_spec.path,
            NormalizedSpec::Rule(rule_spec) => &rule_spec.path,
            NormalizedSpec::Hook(hook_spec) => &hook_spec.path,
        }
    }

    pub fn description(&self) -> &str {
        match self {
            NormalizedSpec::Agent(s) => &s.frontmatter.description,
            NormalizedSpec::Skill(s) => s.frontmatter.description.as_deref().unwrap_or_default(),
            NormalizedSpec::Rule(s) => s.frontmatter.description.as_deref().unwrap_or_default(),
            NormalizedSpec::Hook(s) => s.frontmatter.description.as_deref().unwrap_or_default(),
        }
    }

    pub fn tags(&self) -> &[String] {
        match self {
            NormalizedSpec::Agent(s) => s.frontmatter.tags.as_deref().unwrap_or_default(),
            NormalizedSpec::Skill(s) => s.frontmatter.tags.as_deref().unwrap_or_default(),
            NormalizedSpec::Rule(s) => s.frontmatter.tags.as_deref().unwrap_or_default(),
            NormalizedSpec::Hook(s) => s.frontmatter.tags.as_deref().unwrap_or_default(),
        }
    }

    pub fn spec_type(&self) -> &'static str {
        match self {
            NormalizedSpec::Agent(_) => "agent",
            NormalizedSpec::Skill(_) => "skill",
            NormalizedSpec::Rule(_) => "rule",
            NormalizedSpec::Hook(_) => "hook",
        }
    }
}

#[derive(Debug)]
pub struct AgentSpec {
    /// Absolute path to the spec
    pub path: PathBuf,
    /// Parsed frontmatter
    pub frontmatter: AgentFrontmatter,
    /// Spec body (Markdown content after frontmatter)
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct NormalizedAgentSpec {
    /// Absolute path to the spec
    pub path: PathBuf,
    /// Parsed frontmatter
    pub frontmatter: NormalizedAgentFrontmatter,
    /// Spec body (Markdown content after frontmatter)
    pub body: String,
}

#[derive(Debug)]
pub struct SkillSpec {
    /// Absolute path to the spec root
    pub path: PathBuf,
    /// Parsed frontmatter
    pub frontmatter: SkillFrontmatter,
    /// Spec body (Markdown content after frontmatter)
    pub body: String,
    /// Additional files bundled with the skill
    pub supporting_files: Vec<SupportingFile>,
}

#[derive(Clone, Debug)]
pub struct NormalizedSkillSpec {
    /// Absolute path to the spec root
    pub path: PathBuf,
    /// Parsed frontmatter
    pub frontmatter: NormalizedSkillFrontmatter,
    /// Spec body (Markdown content after frontmatter)
    pub body: String,
    /// Additional files bundled with the skill
    pub supporting_files: Vec<SupportingFile>,
}

#[derive(Debug)]
pub struct RuleSpec {
    /// Absolute path to the spec root
    pub path: PathBuf,
    /// Parsed frontmatter
    pub frontmatter: RuleFrontmatter,
    /// Spec body (Markdown content after frontmatter)
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct NormalizedRuleSpec {
    /// Absolute path to the spec root
    pub path: PathBuf,
    /// Parsed frontmatter
    pub frontmatter: NormalizedRuleFrontmatter,
    /// Spec body (Markdown content after frontmatter)
    pub body: String,
}

#[derive(Debug)]
pub struct HookSpec {
    /// Absolute path to the `hooks.toml` file the spec was loaded from.
    pub path: PathBuf,
    /// Parsed metadata for a single hook entry.
    pub frontmatter: HookFrontmatter,
    /// Always empty for hooks; the empty-body validation check is exempt for this variant.
    pub body: String,
    /// Files under `spec/hooks/scripts/` (recursive), `relative_path` rooted at
    /// the hooks dir (so `scripts/init.sh`). Every `HookSpec` produced from one
    /// `hooks.toml` carries the same list — emission is deduplicated by emitting
    /// from a single provider-level synthesis pass, not per spec.
    pub supporting_files: Vec<SupportingFile>,
}

#[derive(Clone, Debug)]
pub struct NormalizedHookSpec {
    pub path: PathBuf,
    pub frontmatter: NormalizedHookFrontmatter,
    pub body: String,
    pub supporting_files: Vec<SupportingFile>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFrontmatter {
    pub id: String,
    pub description: String,
    pub tags: Option<Vec<String>>,
    pub execution: Option<ExecutionFrontmatter>,
    pub capabilities: Option<CapabilitiesFrontmatter>,
}

#[derive(Clone, Debug)]
pub struct NormalizedAgentFrontmatter {
    pub id: String,
    pub description: String,
    pub tags: Option<Vec<String>>,
    pub execution: Option<ExecutionFrontmatter>,
    pub capabilities: Option<CapabilitiesFrontmatter>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SkillFrontmatter {
    pub id: String,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub user_invocable: bool,
    pub agent_invocable: bool,
    pub execution: Option<ExecutionFrontmatter>,
    pub capabilities: Option<CapabilitiesFrontmatter>,
}

#[derive(Clone, Debug)]
pub struct NormalizedSkillFrontmatter {
    pub id: String,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub user_invocable: bool,
    pub agent_invocable: bool,
    pub execution: Option<ExecutionFrontmatter>,
    pub capabilities: Option<CapabilitiesFrontmatter>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuleFrontmatter {
    pub id: String,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct NormalizedRuleFrontmatter {
    pub id: String,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// A single hook entry, parsed from a `[hooks.<id>]` table in `hooks.toml`.
///
/// `id` is captured from the TOML table key (not the inner table) when loaded;
/// it is included as a struct field after construction so downstream code can
/// treat it like every other spec frontmatter.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HookFrontmatter {
    /// Stable identifier; populated from the `[hooks.<id>]` TOML table key.
    #[serde(skip)]
    pub id: String,
    /// Provider-neutral event name (e.g., `pre_tool_use`).
    pub event: HookEvent,
    /// Path to the script implementation, relative to `spec/hooks/`.
    pub script: PathBuf,
    /// Tool-name matcher; only valid on tool-execute events.
    pub matcher: Option<String>,
    /// Optional timeout in seconds.
    pub timeout: Option<u32>,
    /// Free-form description (informational; not consumed by either provider in v1).
    pub description: Option<String>,
    /// Free-form tags.
    pub tags: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct NormalizedHookFrontmatter {
    pub id: String,
    pub event: HookEvent,
    pub script: PathBuf,
    pub matcher: Option<String>,
    pub timeout: Option<u32>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// The provider-neutral event surface for hooks.
///
/// Variants map to provider-specific event names inside each adapter
/// via `HookAdapter::event_name`; the enum itself only expresses semantic
/// identity, not naming.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    SessionStart,
    SessionEnd,
    Stop,
    PreCompact,
    SubagentStart,
    SubagentStop,
    UserPromptSubmit,
}

impl HookEvent {
    /// Whether this event accepts a `matcher` field (true only for tool-execute events).
    pub fn allows_matcher(self) -> bool {
        matches!(
            self,
            Self::PreToolUse | Self::PostToolUse | Self::PostToolUseFailure
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilitiesFrontmatter {
    pub tools: Option<Vec<ToolFrontmatter>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionFrontmatter {
    pub preset: Option<String>,
}

#[derive(Clone, Debug, Deserialize, strum::EnumString, Serialize, strum::VariantArray)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ToolFrontmatter {
    Read,
    Write,
    Edit,
    Grep,
    Glob,
    Bash,
    WebFetch,
    WebSearch,
    Question,
    Tasks,
    Subagent,
    Skill,
}

#[derive(Clone, Debug)]
pub struct SupportingFile {
    /// Path relative to the skill directory
    pub relative_path: PathBuf,
    /// Raw file content
    pub content: Vec<u8>,
    /// Standard rwx permission bits (mode & 0o0777) sourced from the
    /// source file's filesystem mode at load time. Setuid/setgid/sticky
    /// bits (`0o7000`) are deliberately masked away — agentspec is a
    /// build tool and faithful copying of those bits would be a security
    /// footgun. Always populated; emitted unchanged at write time so
    /// user-set ergonomic modes (0o600, 0o400, etc.) survive the
    /// compile/sync pipeline.
    pub mode: u32,
}
