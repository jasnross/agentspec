use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::de::{self, MapAccess, SeqAccess, Unexpected, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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

    /// The execution preset this spec names, if any.
    ///
    /// `Spec::Hook` has none: `HookFrontmatter` is `deny_unknown_fields` and
    /// declares no execution field, so a hooks entry naming a preset fails to
    /// parse. The same holds for [`Spec::declares_tools`] and
    /// [`Spec::declares_paths`].
    pub fn execution_preset(&self) -> Option<&str> {
        let execution = match self {
            Spec::Agent(s) => s.frontmatter.execution.as_ref(),
            Spec::Skill(s) => s.frontmatter.execution.as_ref(),
            Spec::Rule(_) | Spec::Hook(_) => None,
        };
        execution.and_then(|e| e.preset.as_deref())
    }

    /// Whether this spec declares `capabilities.tools`.
    pub fn declares_tools(&self) -> bool {
        let capabilities = match self {
            Spec::Agent(s) => s.frontmatter.capabilities.as_ref(),
            Spec::Skill(s) => s.frontmatter.capabilities.as_ref(),
            Spec::Rule(_) | Spec::Hook(_) => None,
        };
        capabilities.is_some_and(|c| c.tools.is_some())
    }

    /// This spec's `capabilities.mcp` grants, keyed by declared server name.
    pub fn mcp_grants(&self) -> Option<&BTreeMap<String, McpGrant>> {
        let capabilities = match self {
            Spec::Agent(s) => s.frontmatter.capabilities.as_ref(),
            Spec::Skill(s) => s.frontmatter.capabilities.as_ref(),
            Spec::Rule(_) | Spec::Hook(_) => None,
        };
        capabilities.and_then(|c| c.mcp.as_ref())
    }

    /// Whether this spec declares `paths`. Only rules carry the field.
    pub fn declares_paths(&self) -> bool {
        match self {
            Spec::Rule(s) => s.frontmatter.paths.is_some(),
            Spec::Agent(_) | Spec::Skill(_) | Spec::Hook(_) => false,
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
    /// Positional arguments passed to the script, after the canonical
    /// payload on stdin. agentspec quotes each value; authors write
    /// literal text, never shell syntax.
    pub args: Option<Vec<String>>,
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
    /// MCP grants: declared logical server name → the tools granted from it.
    #[serde(default, deserialize_with = "deserialize_mcp_grants")]
    pub mcp: Option<BTreeMap<String, McpGrant>>,
}

/// The tools one spec is granted from one declared MCP server.
///
/// A struct rather than a bare [`McpTools`] so a later grant of the server's
/// MCP resources is an added key, not a change of shape.
///
/// `Deserialize` is written by hand, driven by `deserialize_any`: frontmatter
/// is parsed through `gray_matter`'s `Pod` deserializer, whose
/// `deserialize_struct` fails any non-map with "Type error, expected: hash
/// map" and never consults a derived visitor's `expecting` text. Only
/// `deserialize_any` dispatches on the value's actual type, which is what lets
/// a mistyped grant name the forms it accepts.
#[derive(Clone, Debug, PartialEq)]
pub struct McpGrant {
    pub tools: McpTools,
}

/// Which of a server's tools a grant names.
#[derive(Clone, Debug, PartialEq)]
pub enum McpTools {
    /// `tools: all` — every tool the server offers.
    All,
    /// `tools: [<tool>, ...]` — exactly these tools.
    Named(Vec<String>),
}

impl<'de> Deserialize<'de> for McpGrant {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct GrantVisitor;

        impl<'de> Visitor<'de> for GrantVisitor {
            type Value = McpGrant;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an MCP grant table, `{ tools: [<tool>, ...] }` or `{ tools: all }`")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<McpGrant, A::Error> {
                let mut tools: Option<McpTools> = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key != "tools" {
                        return Err(de::Error::unknown_field(&key, &["tools"]));
                    }
                    if tools.is_some() {
                        return Err(de::Error::duplicate_field("tools"));
                    }
                    tools = Some(map.next_value()?);
                }
                let tools = tools.ok_or_else(|| de::Error::missing_field("tools"))?;
                Ok(McpGrant { tools })
            }
        }

        deserializer.deserialize_any(GrantVisitor)
    }
}

impl Serialize for McpGrant {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Self { tools } = self;
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry("tools", tools)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for McpTools {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ToolsVisitor;

        impl<'de> Visitor<'de> for ToolsVisitor {
            type Value = McpTools;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("the keyword `all` or a list of tool names")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<McpTools, E> {
                if v == "all" {
                    Ok(McpTools::All)
                } else {
                    Err(E::invalid_value(Unexpected::Str(v), &self))
                }
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<McpTools, A::Error> {
                let mut tools = Vec::new();
                while let Some(tool) = seq.next_element::<String>()? {
                    tools.push(tool);
                }
                Ok(McpTools::Named(tools))
            }
        }

        deserializer.deserialize_any(ToolsVisitor)
    }
}

impl Serialize for McpTools {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::All => serializer.serialize_str("all"),
            Self::Named(tools) => tools.serialize(serializer),
        }
    }
}

/// `capabilities.mcp`, keyed by server, with each value read as an
/// [`McpGrant`].
///
/// A value error is rewrapped to name `capabilities.mcp.<server>`, because
/// frontmatter is parsed with no `serde_path_to_error` and the error would
/// otherwise name neither the field nor the server.
fn deserialize_mcp_grants<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<BTreeMap<String, McpGrant>>, D::Error> {
    struct GrantsVisitor;

    impl<'de> Visitor<'de> for GrantsVisitor {
        type Value = Option<BTreeMap<String, McpGrant>>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("`capabilities.mcp` as a table of MCP grants keyed by declared server name")
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        /// Reads every entry before failing, and reports the failing grant
        /// whose server sorts first: `gray_matter` hands the map over in hash
        /// order, so stopping at the first failure would report a different
        /// grant from run to run. An empty table grants nothing, as `null`
        /// does.
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut grants = BTreeMap::new();
            let mut first_error: Option<(String, String)> = None;
            while let Some(server) = map.next_key::<String>()? {
                match map.next_value::<McpGrant>() {
                    Ok(grant) => {
                        grants.insert(server, grant);
                    }
                    Err(e) => {
                        if first_error
                            .as_ref()
                            .is_none_or(|(first, _)| server < *first)
                        {
                            first_error = Some((server, e.to_string()));
                        }
                    }
                }
            }
            if let Some((server, e)) = first_error {
                return Err(de::Error::custom(format!("capabilities.mcp.{server}: {e}")));
            }
            Ok((!grants.is_empty()).then_some(grants))
        }
    }

    deserializer.deserialize_any(GrantsVisitor)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionFrontmatter {
    pub preset: Option<String>,
}

#[derive(
    Clone,
    Debug,
    Deserialize,
    Eq,
    strum::EnumString,
    strum::IntoStaticStr,
    PartialEq,
    Serialize,
    strum::VariantArray,
)]
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
                args: None,
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

#[cfg(test)]
mod mcp_grant_parsing_tests {
    use gray_matter::Matter;
    use gray_matter::engine::YAML;

    use super::{AgentFrontmatter, McpGrant, McpTools};

    /// Parses through `gray_matter`, as the loader does: a derived visitor's
    /// error text behaves differently there than under `serde_yml`.
    fn parse(mcp_yaml: &str) -> Result<AgentFrontmatter, String> {
        let content = format!(
            "---\nid: a\ndescription: d\ncapabilities:\n  tools: []\n  mcp:\n    {mcp_yaml}\n---\nBody.\n"
        );
        Matter::<YAML>::new()
            .parse::<AgentFrontmatter>(&content)
            .map_err(|e| format!("{e:#}"))?
            .data
            .ok_or_else(|| "no frontmatter".to_owned())
    }

    fn grant(frontmatter: &AgentFrontmatter, server: &str) -> McpGrant {
        frontmatter
            .capabilities
            .as_ref()
            .and_then(|c| c.mcp.as_ref())
            .and_then(|m| m.get(server))
            .cloned()
            .expect("grant present")
    }

    #[test]
    fn test_mcp_grant_parses_all_and_named() {
        let all = parse("quip: { tools: all }").expect("parses");
        assert_eq!(grant(&all, "quip").tools, McpTools::All);

        let named = parse("quip: { tools: [a, b] }").expect("parses");
        assert_eq!(
            grant(&named, "quip").tools,
            McpTools::Named(vec!["a".to_owned(), "b".to_owned()])
        );
    }

    #[test]
    fn test_mcp_grant_rejects_unknown_keyword() {
        let err = parse("quip: { tools: everything }").expect_err("rejected");
        assert!(err.contains("capabilities.mcp.quip"), "{err}");
        assert!(err.contains("`all`"), "{err}");
        assert!(err.contains("list of tool names"), "{err}");
    }

    #[test]
    fn test_mcp_grant_rejects_non_table_value() {
        for value in ["quip: [a]", "quip: all"] {
            let err = parse(value).expect_err("rejected");
            assert!(err.contains("capabilities.mcp.quip"), "{value}: {err}");
            assert!(err.contains("MCP grant table"), "{value}: {err}");
        }
    }

    #[test]
    fn test_mcp_grant_rejects_unknown_field() {
        let err = parse("quip: { tool: [a] }").expect_err("rejected");
        assert!(err.contains("capabilities.mcp.quip"), "{err}");
        assert!(err.contains("`tool`"), "{err}");
    }

    /// Parses a whole `mcp:` value, rather than one grant under it.
    fn parse_mcp(mcp_value: &str) -> Result<AgentFrontmatter, String> {
        let content = format!(
            "---\nid: a\ndescription: d\ncapabilities:\n  tools: []\n  mcp: {mcp_value}\n---\nBody.\n"
        );
        Matter::<YAML>::new()
            .parse::<AgentFrontmatter>(&content)
            .map_err(|e| format!("{e:#}"))?
            .data
            .ok_or_else(|| "no frontmatter".to_owned())
    }

    #[test]
    fn test_mcp_null_and_empty_table_grant_nothing() {
        for value in ["", "null", "{}"] {
            let parsed = parse_mcp(value).expect("parses");
            let mcp = parsed.capabilities.as_ref().and_then(|c| c.mcp.as_ref());
            assert!(mcp.is_none(), "{value:?}: {mcp:?}");
        }
    }

    #[test]
    fn test_mcp_non_table_names_the_field() {
        for value in ["[quip]", "quip"] {
            let err = parse_mcp(value).expect_err("rejected");
            assert!(
                err.contains("`capabilities.mcp` as a table"),
                "{value}: {err}"
            );
        }
    }

    #[test]
    fn test_mcp_grant_rejects_non_string_tool_and_missing_tools() {
        for value in ["quip: { tools: [1] }", "quip: {}"] {
            let err = parse(value).expect_err("rejected");
            assert!(err.contains("capabilities.mcp.quip"), "{value}: {err}");
        }
        let missing = parse("quip: {}").expect_err("rejected");
        assert!(missing.contains("missing field `tools`"), "{missing}");
    }

    /// `gray_matter` hands the map over in hash order; the report must not
    /// depend on it.
    #[test]
    fn test_mcp_grant_reports_first_failing_server_by_name() {
        for _ in 0..8 {
            let err = parse("zz: { tools: x }\n    aa: { tools: y }\n    mm: { tools: all }")
                .expect_err("rejected");
            assert!(err.contains("capabilities.mcp.aa"), "{err}");
        }
    }

    #[test]
    fn test_mcp_grant_serializes_as_authored() {
        let parsed = parse("quip: { tools: [a] }\n    jira: { tools: all }").expect("parses");
        let mcp = parsed
            .capabilities
            .as_ref()
            .and_then(|c| c.mcp.as_ref())
            .expect("grants present");
        let yaml = serde_yml::to_string(mcp).expect("serializes");
        assert_eq!(yaml, "jira:\n  tools: all\nquip:\n  tools:\n  - a\n");
    }
}
