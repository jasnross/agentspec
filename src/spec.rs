use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub enum Spec {
    Agent(AgentSpec),
    Skill(SkillSpec),
    Rule(RuleSpec),
    Hook(HookSpec),
}

impl Spec {
    pub fn id(&self) -> &str {
        match self {
            Spec::Agent(s) => &s.frontmatter.id,
            Spec::Skill(s) => &s.frontmatter.id,
            Spec::Rule(s) => &s.frontmatter.id,
            Spec::Hook(s) => &s.frontmatter.id,
        }
    }

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

    pub fn description(&self) -> &str {
        match self {
            Spec::Agent(s) => &s.frontmatter.description,
            Spec::Skill(s) => s.frontmatter.description.as_deref().unwrap_or_default(),
            Spec::Rule(s) => s.frontmatter.description.as_deref().unwrap_or_default(),
            Spec::Hook(s) => s.frontmatter.description.as_deref().unwrap_or_default(),
        }
    }

    pub fn tags(&self) -> &[String] {
        match self {
            Spec::Agent(s) => s.frontmatter.tags.as_deref().unwrap_or_default(),
            Spec::Skill(s) => s.frontmatter.tags.as_deref().unwrap_or_default(),
            Spec::Rule(s) => s.frontmatter.tags.as_deref().unwrap_or_default(),
            Spec::Hook(s) => s.frontmatter.tags.as_deref().unwrap_or_default(),
        }
    }

    pub fn spec_type(&self) -> &'static str {
        match self {
            Spec::Agent(_) => "agent",
            Spec::Skill(_) => "skill",
            Spec::Rule(_) => "rule",
            Spec::Hook(_) => "hook",
        }
    }
}

#[derive(Clone, Debug)]
pub struct AgentSpec {
    /// Absolute path to the spec
    pub path: PathBuf,
    /// Parsed frontmatter
    pub frontmatter: AgentFrontmatter,
    /// Spec body (Markdown content after frontmatter)
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct SkillSpec {
    /// Absolute path to the spec root
    pub path: PathBuf,
    /// Parsed frontmatter
    pub frontmatter: SkillFrontmatter,
    /// Spec body (Markdown content after frontmatter)
    pub body: String,
    /// Additional files bundled with the skill, keyed by path relative to the
    /// skill directory.
    pub supporting_files: IndexMap<PathBuf, SupportingFile>,
}

#[derive(Clone, Debug)]
pub struct RuleSpec {
    /// Absolute path to the spec root
    pub path: PathBuf,
    /// Parsed frontmatter
    pub frontmatter: RuleFrontmatter,
    /// Spec body (Markdown content after frontmatter)
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct HookSpec {
    /// Absolute path to the `hooks.toml` file the spec was loaded from.
    pub path: PathBuf,
    /// Parsed metadata for a single hook entry.
    pub frontmatter: HookFrontmatter,
    /// Always empty for hooks; the empty-body validation check is exempt for this variant.
    pub body: String,
    /// Files under `spec/hooks/scripts/` (recursive), keyed by path relative to
    /// the hooks dir (so `scripts/init.sh`). Every `HookSpec` produced from one
    /// `hooks.toml` carries the same map — emission is deduplicated by emitting
    /// from a single provider-level synthesis pass, not per spec.
    pub supporting_files: IndexMap<PathBuf, SupportingFile>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFrontmatter {
    pub id: String,
    pub description: String,
    pub tags: Option<Vec<String>>,
    pub execution: Option<ExecutionFrontmatter>,
    pub capabilities: Option<CapabilitiesFrontmatter>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuleFrontmatter {
    pub id: String,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub paths: Option<Vec<String>>,
}

/// A single hook entry, parsed from a `[hooks.<id>]` table in `hooks.toml`.
///
/// `id` is captured from the TOML table key (not the inner table) when loaded;
/// it is included as a struct field after construction so downstream code can
/// treat it like every other spec frontmatter.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HookFrontmatter {
    /// Stable identifier; populated from the `[hooks.<id>]` TOML table key.
    #[serde(skip)]
    pub id: String,
    /// Provider-neutral event(s) this hook targets.
    pub events: Vec<HookEvent>,
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

/// The provider-neutral event surface for hooks.
///
/// Variants map to provider-specific event names inside each adapter
/// (`ClaudeAdapter::event_name` / `CursorAdapter::event_name`); the enum
/// itself only expresses semantic identity, not naming.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, clap::ValueEnum)]
#[clap(rename_all = "snake_case")]
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
    /// Whether this event accepts a `matcher` field (tool-execute and subagent events).
    pub fn allows_matcher(self) -> bool {
        matches!(
            self,
            Self::PreToolUse
                | Self::PostToolUse
                | Self::PostToolUseFailure
                | Self::SubagentStart
                | Self::SubagentStop
        )
    }

    /// Whether this event targets subagent lifecycle (`SubagentStart` / `SubagentStop`).
    pub fn is_subagent_event(self) -> bool {
        matches!(self, Self::SubagentStart | Self::SubagentStop)
    }

    /// Canonical `snake_case` name (matches the `#[serde(rename_all = "snake_case")]`
    /// wire form). Used by the shim codegen and snapshot-file naming.
    pub fn snake_case(self) -> &'static str {
        match self {
            Self::PreToolUse => "pre_tool_use",
            Self::PostToolUse => "post_tool_use",
            Self::PostToolUseFailure => "post_tool_use_failure",
            Self::SessionStart => "session_start",
            Self::SessionEnd => "session_end",
            Self::Stop => "stop",
            Self::PreCompact => "pre_compact",
            Self::SubagentStart => "subagent_start",
            Self::SubagentStop => "subagent_stop",
            Self::UserPromptSubmit => "user_prompt_submit",
        }
    }

    /// `PascalCase` event name — the Claude wire form for
    /// `hookEventName` and the Rust variant identifier.
    pub fn pascal_case(self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
            Self::Stop => "Stop",
            Self::PreCompact => "PreCompact",
            Self::SubagentStart => "SubagentStart",
            Self::SubagentStop => "SubagentStop",
            Self::UserPromptSubmit => "UserPromptSubmit",
        }
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
    Shell,
    WebFetch,
    WebSearch,
    Question,
    Tasks,
    Subagent,
    Skill,
}

#[derive(Clone, Debug)]
pub struct SupportingFile {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_spec(id: &str, description: &str, tags: Option<Vec<String>>) -> Spec {
        Spec::Agent(AgentSpec {
            path: PathBuf::from("/tmp/agent.md"),
            frontmatter: AgentFrontmatter {
                id: id.to_string(),
                description: description.to_string(),
                tags,
                execution: None,
                capabilities: None,
            },
            body: String::new(),
        })
    }

    fn skill_spec(id: &str, description: Option<&str>, tags: Option<Vec<String>>) -> Spec {
        Spec::Skill(SkillSpec {
            path: PathBuf::from("/tmp/skill"),
            frontmatter: SkillFrontmatter {
                id: id.to_string(),
                description: description.map(str::to_string),
                tags,
                user_invocable: false,
                agent_invocable: true,
                execution: None,
                capabilities: None,
            },
            body: String::new(),
            supporting_files: IndexMap::new(),
        })
    }

    fn rule_spec(id: &str, description: Option<&str>) -> Spec {
        Spec::Rule(RuleSpec {
            path: PathBuf::from("/tmp/rule.md"),
            frontmatter: RuleFrontmatter {
                id: id.to_string(),
                description: description.map(str::to_string),
                tags: None,
                paths: None,
            },
            body: String::new(),
        })
    }

    fn hook_spec(id: &str, description: Option<&str>) -> Spec {
        Spec::Hook(HookSpec {
            path: PathBuf::from("/tmp/hooks.toml"),
            frontmatter: HookFrontmatter {
                id: id.to_string(),
                events: vec![HookEvent::SessionStart],
                script: PathBuf::from("scripts/init.sh"),
                matcher: None,
                timeout: None,
                description: description.map(str::to_string),
                tags: None,
            },
            body: String::new(),
            supporting_files: IndexMap::new(),
        })
    }

    #[test]
    fn test_spec_id_agent() {
        assert_eq!(agent_spec("agent-1", "desc", None).id(), "agent-1");
    }

    #[test]
    fn test_spec_id_skill() {
        assert_eq!(skill_spec("skill-1", None, None).id(), "skill-1");
    }

    #[test]
    fn test_spec_id_rule() {
        assert_eq!(rule_spec("rule-1", None).id(), "rule-1");
    }

    #[test]
    fn test_spec_id_hook() {
        assert_eq!(hook_spec("hook-1", None).id(), "hook-1");
    }

    #[test]
    fn test_spec_description_agent_required() {
        assert_eq!(
            agent_spec("a", "the description", None).description(),
            "the description"
        );
    }

    #[test]
    fn test_spec_description_optional_returns_empty_when_none() {
        assert_eq!(skill_spec("s", None, None).description(), "");
        assert_eq!(rule_spec("r", None).description(), "");
        assert_eq!(hook_spec("h", None).description(), "");
    }

    #[test]
    fn test_spec_tags_returns_empty_slice_when_none() {
        assert!(agent_spec("a", "d", None).tags().is_empty());
        assert!(skill_spec("s", None, None).tags().is_empty());
        assert!(rule_spec("r", None).tags().is_empty());
        assert!(hook_spec("h", None).tags().is_empty());
    }

    #[test]
    fn test_spec_tags_returns_populated_when_some() {
        let spec = agent_spec("a", "d", Some(vec!["x".into(), "y".into()]));
        assert_eq!(spec.tags(), &["x".to_string(), "y".to_string()]);
    }

    #[test]
    fn test_spec_spec_type() {
        assert_eq!(agent_spec("a", "d", None).spec_type(), "agent");
        assert_eq!(skill_spec("s", None, None).spec_type(), "skill");
        assert_eq!(rule_spec("r", None).spec_type(), "rule");
        assert_eq!(hook_spec("h", None).spec_type(), "hook");
    }

    #[test]
    fn test_spec_clone_round_trip() {
        let cases = [
            agent_spec("a", "d", Some(vec!["t".into()])),
            skill_spec("s", Some("d"), Some(vec!["t".into()])),
            rule_spec("r", Some("d")),
            hook_spec("h", Some("d")),
        ];
        for original in &cases {
            let cloned = original.clone();
            assert_eq!(original.id(), cloned.id());
            assert_eq!(original.description(), cloned.description());
            assert_eq!(original.tags(), cloned.tags());
            assert_eq!(original.spec_type(), cloned.spec_type());
            assert_eq!(original.body(), cloned.body());
            assert_eq!(original.path(), cloned.path());
        }
    }
}
