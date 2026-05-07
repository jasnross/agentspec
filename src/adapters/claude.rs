use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde::Serialize;

use crate::compile::{
    AdapterConfig, EmittedHookEntry, GeneratedFile, HookEmitMode, HookSynthesis,
    build_emitted_hook_entries, build_hook_script_files,
};
use crate::plan::{FileKind, PostWriteHook};
use crate::presets::ProviderPresetsMap;
use crate::provider::Provider;
use crate::spec::{
    HookEvent, NormalizedAgentSpec, NormalizedHookSpec, NormalizedRuleSpec, NormalizedSkillSpec,
    NormalizedSpec, ToolFrontmatter,
};

// See: https://code.claude.com/docs/en/sub-agents#supported-frontmatter-fields
#[derive(Serialize)]
struct ClaudeAgentFrontmatter {
    name: String,
    description: String,
    model: Option<String>,
    tools: Option<Vec<ClaudeTool>>,
}

// See: https://code.claude.com/docs/en/skills#frontmatter-reference
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ClaudeSkillFrontmatter {
    description: String,
    model: Option<String>,
    user_invocable: Option<bool>,
    disable_model_invocation: Option<bool>,
    allowed_tools: Option<Vec<ClaudeTool>>,
}

// FIXME: Should we consider setting all default Claude tools in the generated file? Otherwise Claude's default behavior is to disallow any unlisted tools.
// See: https://code.claude.com/docs/en/tools-reference
#[derive(Serialize)]
#[allow(dead_code)] // FIXME: Consider removing unused tools if we figure out something better
enum ClaudeTool {
    Agent,
    AskUserQuestion,
    Bash,
    CronCreate,
    CronDelete,
    CronList,
    Edit,
    EnterPlanMode,
    EnterWorktree,
    ExitPlanMode,
    ExitWorktree,
    Glob,
    Grep,
    ListMcpResourcesTool,
    Lsp,
    NotebookEdit,
    PowerShell,
    Read,
    ReadMcpResourceTool,
    Skill,
    TaskCreate,
    TaskGet,
    TaskList,
    TaskOutput,
    TaskStop,
    TaskUpdate,
    TodoWrite,
    ToolSearch,
    WebFetch,
    WebSearch,
    Write,
}

pub fn adapt_claude(
    spec: NormalizedSpec,
    presets: &ProviderPresetsMap,
    cfg: Option<&AdapterConfig>,
) -> Result<Vec<GeneratedFile>> {
    match spec {
        NormalizedSpec::Agent(s) => adapt_agent_spec(s, presets, cfg),
        NormalizedSpec::Skill(s) => adapt_skill_spec(s, presets, cfg),
        NormalizedSpec::Rule(s) => Ok(adapt_rule_spec(&s, cfg)),
        // Hook scripts (entry scripts AND helpers under `scripts/`) are emitted
        // by `synthesize_hooks` exactly once per provider, drawn from
        // `supporting_files` collected by `load_hook_specs`. Per-spec dispatch
        // contributes nothing — emitting per spec would duplicate every helper
        // for every hook entry.
        NormalizedSpec::Hook(_) => Ok(Vec::new()),
    }
}

fn adapt_agent_spec(
    spec: NormalizedAgentSpec,
    presets: &ProviderPresetsMap,
    cfg: Option<&AdapterConfig>,
) -> Result<Vec<GeneratedFile>> {
    let id = spec.frontmatter.id;
    let description = spec.frontmatter.description;

    let model = spec
        .frontmatter
        .execution
        .and_then(|x| x.preset)
        .and_then(|x| presets.get(&x))
        .and_then(|x| x.claude.clone())
        .and_then(|x| x.model);

    let tools: Option<Vec<ClaudeTool>> = spec
        .frontmatter
        .capabilities
        .and_then(|x| x.tools)
        .map(|tool_specs| -> Result<Vec<ClaudeTool>> {
            // Sort by serialized name — the value that appears in generated files.
            let mut keyed: Vec<(String, ClaudeTool)> = tool_specs
                .iter()
                .flat_map(adapt_tool)
                .map(|t| Ok((serde_yml::to_string(&t)?, t)))
                .collect::<Result<_>>()?;
            keyed.sort_by(|(a, _), (b, _)| a.cmp(b));
            Ok(keyed.into_iter().map(|(_, t)| t).collect())
        })
        .transpose()?;

    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let path = Path::new("agents").join(format!("{file_prefix}{id}.md"));

    let name = match cfg.and_then(|c| c.prefix.as_deref()) {
        Some(prefix) => format!("{prefix}-{id}"),
        None => id,
    };

    let frontmatter = ClaudeAgentFrontmatter {
        name,
        description,
        model,
        tools,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}---\n\n{body}");

    Ok(vec![GeneratedFile::text(Provider::Claude, path, content)])
}

fn adapt_skill_spec(
    spec: NormalizedSkillSpec,
    presets: &ProviderPresetsMap,
    cfg: Option<&AdapterConfig>,
) -> Result<Vec<GeneratedFile>> {
    let id = spec.frontmatter.id;
    let description = spec.frontmatter.description.unwrap_or_default();

    let model = spec
        .frontmatter
        .execution
        .and_then(|x| x.preset)
        .and_then(|x| presets.get(&x))
        .and_then(|x| x.claude.clone())
        .and_then(|x| x.model);

    let allowed_tools: Option<Vec<ClaudeTool>> = spec
        .frontmatter
        .capabilities
        .and_then(|x| x.tools)
        .map(|x| x.iter().flat_map(adapt_tool).collect());

    let user_invocable = if spec.frontmatter.user_invocable {
        None
    } else {
        Some(false)
    };

    let disable_model_invocation = if spec.frontmatter.agent_invocable {
        None
    } else {
        Some(true)
    };

    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let skill_dir = Path::new("skills").join(format!("{file_prefix}{id}"));

    let frontmatter = ClaudeSkillFrontmatter {
        description,
        model,
        user_invocable,
        disable_model_invocation,
        allowed_tools,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}---\n\n{body}");

    let mut files = vec![GeneratedFile::text(
        Provider::Claude,
        skill_dir.join("SKILL.md"),
        content,
    )];

    for sf in spec.supporting_files {
        files.push(GeneratedFile::binary(
            Provider::Claude,
            skill_dir.join(&sf.relative_path),
            sf.content,
            if sf.executable { Some(0o755) } else { None },
        ));
    }

    Ok(files)
}

fn adapt_rule_spec(spec: &NormalizedRuleSpec, cfg: Option<&AdapterConfig>) -> Vec<GeneratedFile> {
    let content = format!("{}\n", spec.body.trim()).into_bytes();
    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let path = Path::new("rules").join(format!("{file_prefix}{}.md", spec.frontmatter.id));

    vec![GeneratedFile {
        provider: Provider::Claude,
        path,
        content,
        mode: None,
    }]
}

fn adapt_tool(tool: &ToolFrontmatter) -> Vec<ClaudeTool> {
    match tool {
        ToolFrontmatter::Read => vec![ClaudeTool::Read],
        ToolFrontmatter::Write => vec![ClaudeTool::Write],
        ToolFrontmatter::Edit => vec![ClaudeTool::Edit],
        ToolFrontmatter::Grep => vec![ClaudeTool::Grep],
        ToolFrontmatter::Glob => vec![ClaudeTool::Glob],
        ToolFrontmatter::Bash => vec![ClaudeTool::Bash],
        ToolFrontmatter::WebFetch => vec![ClaudeTool::WebFetch],
        ToolFrontmatter::WebSearch => vec![ClaudeTool::WebSearch],
        ToolFrontmatter::Question => vec![ClaudeTool::AskUserQuestion],
        ToolFrontmatter::Tasks => vec![
            ClaudeTool::TaskCreate,
            ClaudeTool::TaskGet,
            ClaudeTool::TaskList,
            ClaudeTool::TaskUpdate,
            ClaudeTool::TaskStop,
            ClaudeTool::TodoWrite,
        ],
        ToolFrontmatter::Subagent => vec![ClaudeTool::Agent],
        ToolFrontmatter::Skill => vec![ClaudeTool::Skill],
    }
}

/// Resolve a canonical tool to the name a Claude spec body should reference.
///
/// For tools that fan out to multiple frontmatter entries (e.g., `Tasks`),
/// returns a single representative name — the one most commonly referenced
/// in spec prose.
pub fn body_tool_name(tool: &ToolFrontmatter) -> &'static str {
    match tool {
        ToolFrontmatter::Read => "Read",
        ToolFrontmatter::Write => "Write",
        ToolFrontmatter::Edit => "Edit",
        ToolFrontmatter::Grep => "Grep",
        ToolFrontmatter::Glob => "Glob",
        ToolFrontmatter::Bash => "Bash",
        ToolFrontmatter::WebFetch => "WebFetch",
        ToolFrontmatter::WebSearch => "WebSearch",
        ToolFrontmatter::Question => "AskUserQuestion",
        ToolFrontmatter::Tasks => "TodoWrite",
        ToolFrontmatter::Subagent => "Agent",
        ToolFrontmatter::Skill => "Skill",
    }
}

// ── hooks.json synthesis ────────────────────────────────────────────────────

// Claude's documented `hooks.json` shape (see <https://code.claude.com/docs/en/hooks>):
//   { "hooks": { "<EventName>": [ { "matcher": "...", "hooks": [<entry>, ...] }, ... ] } }
//
// `entry_to_claude_json` is provider-specific JSON shaping that lives here
// (per CLAUDE.md's "Provider-specific logic belongs in adapters"); the Phase 2
// merge layer (`hooks_merge`) imports it so both emission paths share one
// source of truth for the `_agentspec_id` sentinel and entry shape.

/// Build the JSON object for one entry in Claude's `hooks.json` matcher group.
/// Used by both Phase 1's whole-file synthesis (`build_claude_hooks_json`)
/// and Phase 2's CST merge (`hooks_merge::merge_claude_settings`).
pub fn entry_to_claude_json(e: &EmittedHookEntry) -> serde_json::Value {
    use serde_json::{Map, json};
    let mut obj = Map::new();
    obj.insert("type".to_string(), json!("command"));
    obj.insert("command".to_string(), json!(e.command));
    if let Some(t) = e.timeout {
        obj.insert("timeout".to_string(), json!(t));
    }
    obj.insert("_agentspec_id".to_string(), json!(e.agentspec_id));
    serde_json::Value::Object(obj)
}

/// Translate a canonical `HookEvent` to Claude's `PascalCase` event name.
///
/// Provider-specific naming lives here per `CLAUDE.md`'s "Provider-specific
/// logic belongs in adapters" principle. Exposed publicly so the Phase 2 CST
/// merge layer (`hooks_merge`) can resolve event names without re-deriving
/// the mapping.
pub fn claude_event_name(event: HookEvent) -> &'static str {
    match event {
        HookEvent::PreToolUse => "PreToolUse",
        HookEvent::PostToolUse => "PostToolUse",
        HookEvent::PostToolUseFailure => "PostToolUseFailure",
        HookEvent::SessionStart => "SessionStart",
        HookEvent::SessionEnd => "SessionEnd",
        HookEvent::Stop => "Stop",
        HookEvent::PreCompact => "PreCompact",
        HookEvent::SubagentStart => "SubagentStart",
        HookEvent::SubagentStop => "SubagentStop",
        HookEvent::UserPromptSubmit => "UserPromptSubmit",
    }
}

/// Synthesize the per-provider `hooks/hooks.json` plus the canonical entry
/// list for downstream merge in Phase 2.
///
/// Returns an empty `HookSynthesis` when there are no hook specs. Returns an
/// error when `hook_emit_mode == Some(Merged)` (Phase 2's responsibility).
pub fn synthesize_hooks(
    specs: &[&NormalizedHookSpec],
    cfg: Option<&AdapterConfig>,
) -> Result<HookSynthesis> {
    if specs.is_empty() {
        return Ok(HookSynthesis::default());
    }

    let emit_mode = cfg
        .and_then(|c| c.hook_emit_mode)
        .unwrap_or(HookEmitMode::Bundled);

    let entries = build_emitted_hook_entries(specs, Provider::Claude, emit_mode);
    let mut files = build_hook_script_files(Provider::Claude, specs);
    if matches!(emit_mode, HookEmitMode::Bundled) {
        // Bundled (Path) mode: agentspec owns the whole `hooks/hooks.json`.
        // Merged modes hand emission to `ClaudeHooksPatch`, which surgically
        // edits the host `settings.json` instead.
        let json = build_claude_hooks_json(&entries)?;
        files.push(GeneratedFile::text(
            Provider::Claude,
            Path::new("hooks").join("hooks.json"),
            json,
        ));
    }
    Ok(HookSynthesis { entries, files })
}

/// Group entries by `(event, matcher)` and serialize Claude's documented shape.
///
/// Top-level event keys are sorted alphabetically (`BTreeMap`) for stable output.
/// Within an event, matcher groups preserve first-seen order — propagated from
/// the spec list, which itself preserves `IndexMap` authoring order from the
/// `hooks.toml` file. The per-entry serialization delegates to the local
/// `entry_to_claude_json` helper so Phase 1 and Phase 2 share one source of
/// truth for the entry shape (including the `_agentspec_id` sentinel).
fn build_claude_hooks_json(entries: &[EmittedHookEntry]) -> Result<String> {
    use serde_json::{Map, Value, json};

    let mut by_event: BTreeMap<&'static str, IndexMap<Option<String>, Vec<Value>>> =
        BTreeMap::new();
    for entry in entries {
        by_event
            .entry(claude_event_name(entry.event))
            .or_default()
            .entry(entry.matcher.clone())
            .or_default()
            .push(entry_to_claude_json(entry));
    }

    let mut hooks_map = Map::new();
    for (event, by_matcher) in by_event {
        let groups: Vec<Value> = by_matcher
            .into_iter()
            .map(|(matcher, hook_entries)| {
                let mut group = Map::new();
                if let Some(m) = matcher {
                    group.insert("matcher".to_string(), json!(m));
                }
                group.insert("hooks".to_string(), Value::Array(hook_entries));
                Value::Object(group)
            })
            .collect();
        hooks_map.insert(event.to_string(), Value::Array(groups));
    }

    let top = json!({ "hooks": hooks_map });
    let json =
        serde_json::to_string_pretty(&top).context("failed to serialize Claude hooks.json")?;
    Ok(format!("{json}\n"))
}

/// Post-write hook that merges agentspec-owned hook entries into Claude's
/// hand-edited `settings.json` via the CST patcher in `hooks_merge`.
///
/// Constructed once per `(provider, FileKind::Hooks)` sync call when the
/// emit mode is `MergedUser` or `MergedProject`. Path mode owns the entire
/// `hooks/hooks.json` file directly so no patcher is created for it.
#[derive(Debug)]
pub struct ClaudeHooksPatch {
    settings_path: std::path::PathBuf,
    owned_entries: Vec<EmittedHookEntry>,
    /// `--force`/`overwrite=true`: replace a non-object `hooks` value with
    /// `{}` before merging, instead of erroring.
    force: bool,
}

impl PostWriteHook for ClaudeHooksPatch {
    fn run(&self, dry_run: bool) -> Result<()> {
        crate::hooks_merge::merge_claude_settings(
            &self.settings_path,
            &self.owned_entries,
            self.force,
            dry_run,
        )
    }
}

/// Factory for Claude's post-write hooks.
///
/// `config_dir` is the parent of `dest` for hooks (e.g., `~/.claude` when
/// `dest` is `~/.claude/hooks`); the binary computes it at the call site
/// rather than the library inferring it from `dest`. `force` reflects the
/// `--force` flag and lets the merge replace user-authored non-object
/// `hooks` values rather than erroring.
pub fn post_write_hook(
    kind: FileKind,
    _dest: &Path,
    config_dir: &Path,
    emit_mode: HookEmitMode,
    owned_entries: &[EmittedHookEntry],
    force: bool,
) -> Option<Box<dyn PostWriteHook>> {
    if kind != FileKind::Hooks {
        return None;
    }
    if !emit_mode.is_merged() {
        // Bundled (Path) mode: agentspec owns the whole `hooks/hooks.json`
        // produced by `synthesize_hooks`. No merge needed.
        return None;
    }
    Some(Box::new(ClaudeHooksPatch {
        settings_path: config_dir.join("settings.json"),
        owned_entries: owned_entries.to_vec(),
        force,
    }))
}

/// Post-write hook that strips agentspec-owned hook entries from Claude's
/// `settings.json` and tidies emptied containers, paralleling
/// [`ClaudeHooksPatch`] but in reverse.
///
/// The patch identifies its own entries via the on-disk `_agentspec_id`
/// sentinel — no in-memory owned-entries list is needed (compare
/// [`ClaudeHooksPatch`], which receives the sync's emitted-entries vector).
#[derive(Debug)]
pub struct ClaudeRemoveHooksPatch {
    settings_path: std::path::PathBuf,
}

impl PostWriteHook for ClaudeRemoveHooksPatch {
    fn run(&self, dry_run: bool) -> Result<()> {
        let report = crate::hooks_merge::remove_claude_settings(&self.settings_path, dry_run)?;
        report.print_summary();
        Ok(())
    }
}

/// Factory for Claude's remove post-write hook.
///
/// `_dest` is accepted for signature symmetry with the `OpenCode` factory and
/// `sync`'s `post_write_hook` — Claude's remove patch identifies its targets
/// by reading on-disk `_agentspec_id` sentinels, so the per-kind dest dir
/// doesn't affect what gets removed. The uniform signature lets `remove.rs`
/// dispatch each provider's factory through the same `match` arm shape.
///
/// Returns `None` for non-`Hooks` kinds and for non-Merged emit modes (Path
/// mode owns `hooks/hooks.json` outright — its cleanup is handled by
/// `remove_batch`'s dest-dir teardown, not via a settings.json patch).
pub fn remove_post_write_hook(
    kind: FileKind,
    _dest: &Path,
    config_dir: &Path,
    emit_mode: HookEmitMode,
) -> Option<Box<dyn PostWriteHook>> {
    if kind != FileKind::Hooks {
        return None;
    }
    if !emit_mode.is_merged() {
        return None;
    }
    Some(Box::new(ClaudeRemoveHooksPatch {
        settings_path: config_dir.join("settings.json"),
    }))
}

/// Returns the name the AI model uses to reference this spec.
///
/// For Claude, all spec types use `{content_prefix}{id}` when a content prefix
/// is configured (either explicitly or derived from `prefix`).
pub fn model_facing_name(spec: &NormalizedSpec, cfg: Option<&AdapterConfig>) -> String {
    let id = spec.id();
    match cfg.and_then(AdapterConfig::content_prefix) {
        Some(prefix) => format!("{prefix}{id}"),
        None => id.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde::Deserialize;

    use super::*;
    use crate::spec::{
        CapabilitiesFrontmatter, NormalizedAgentFrontmatter, NormalizedAgentSpec,
        NormalizedRuleFrontmatter, NormalizedRuleSpec,
    };

    #[test]
    fn test_adapt_agent_tools_are_sorted() {
        #[derive(Deserialize)]
        struct Frontmatter {
            tools: Option<Vec<String>>,
        }

        // Tools provided in reverse alphabetical order to confirm sorting.
        let spec = NormalizedSpec::Agent(NormalizedAgentSpec {
            path: "test.md".into(),
            frontmatter: NormalizedAgentFrontmatter {
                id: "test-agent".to_string(),
                description: "Test agent".to_string(),
                tags: None,
                execution: None,
                capabilities: Some(CapabilitiesFrontmatter {
                    tools: Some(vec![
                        ToolFrontmatter::Write,
                        ToolFrontmatter::Read,
                        ToolFrontmatter::Bash,
                    ]),
                }),
            },
            body: "Body.".to_string(),
        });

        let files = adapt_claude(spec, &HashMap::new(), None).expect("expected value");
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");

        // Parse the tools list back out of the generated YAML frontmatter.
        let yaml = content
            .strip_prefix("---\n")
            .and_then(|s| s.split_once("\n---\n"))
            .map(|(fm, _)| fm)
            .expect("expected YAML frontmatter");

        let fm: Frontmatter = serde_yml::from_str(yaml).expect("expected value");
        let tools = fm.tools.expect("expected tools list");

        let mut sorted = tools.clone();
        sorted.sort_unstable();
        assert_eq!(
            tools, sorted,
            "tools should be sorted alphabetically in generated output"
        );
    }

    #[test]
    fn test_adapt_agent_output_format() {
        let spec = NormalizedSpec::Agent(NormalizedAgentSpec {
            path: "test.md".into(),
            frontmatter: NormalizedAgentFrontmatter {
                id: "test-agent".to_string(),
                description: "Test agent".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: "Body.".to_string(),
        });

        let files = adapt_claude(spec, &HashMap::new(), None).expect("expected value");
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");

        let expected = concat!(
            "---\n",
            "name: test-agent\n",
            "description: Test agent\n",
            "model: null\n",
            "tools: null\n",
            "---\n",
            "\n",
            "Body.",
        );
        assert_eq!(content, expected);
    }

    #[test]
    fn test_adapt_agent_with_prefix() {
        let cfg = AdapterConfig {
            prefix: Some("tw".to_string()),
            content_prefix: None,
            ..AdapterConfig::default()
        };
        let spec = NormalizedSpec::Agent(NormalizedAgentSpec {
            path: "test.md".into(),
            frontmatter: NormalizedAgentFrontmatter {
                id: "test-agent".to_string(),
                description: "Test agent".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: "Body.".to_string(),
        });

        let files = adapt_claude(spec, &HashMap::new(), Some(&cfg)).expect("expected value");
        assert_eq!(files[0].path.to_str(), Some("agents/tw-test-agent.md"));

        let content = String::from_utf8(files[0].content.clone()).expect("expected value");
        assert!(
            content.contains("name: tw-test-agent"),
            "frontmatter should contain prefixed name, got: {content}"
        );
    }

    #[test]
    fn test_adapt_rule_with_prefix() {
        let cfg = AdapterConfig {
            prefix: Some("tw".to_string()),
            content_prefix: None,
            ..AdapterConfig::default()
        };
        let spec = NormalizedSpec::Rule(NormalizedRuleSpec {
            path: "test.md".into(),
            frontmatter: NormalizedRuleFrontmatter {
                id: "test-rule".to_string(),
                description: Some("A test rule".to_string()),
                tags: None,
            },
            body: "Rule body.".to_string(),
        });

        let files = adapt_claude(spec, &HashMap::new(), Some(&cfg)).expect("expected value");
        assert_eq!(files[0].path.to_str(), Some("rules/tw-test-rule.md"));
    }

    #[test]
    fn test_adapt_agent_content_prefix_does_not_affect_frontmatter() {
        let cfg = AdapterConfig {
            prefix: None,
            content_prefix: Some("tw:".to_string()),
            ..AdapterConfig::default()
        };
        let spec = NormalizedSpec::Agent(NormalizedAgentSpec {
            path: "test.md".into(),
            frontmatter: NormalizedAgentFrontmatter {
                id: "test-agent".to_string(),
                description: "Test agent".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: "Body.".to_string(),
        });

        let files = adapt_claude(spec, &HashMap::new(), Some(&cfg)).expect("expected value");
        // File path should be unprefixed (no file prefix set)
        assert_eq!(files[0].path.to_str(), Some("agents/test-agent.md"));
        // Frontmatter name should be unprefixed (controlled by `prefix`, not `content_prefix`)
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");
        assert!(
            content.contains("name: test-agent"),
            "frontmatter name should be bare (no content_prefix), got: {content}"
        );
    }

    #[test]
    fn test_model_facing_name_uses_content_prefix() {
        let cfg = AdapterConfig {
            prefix: None,
            content_prefix: Some("tw:".to_string()),
            ..AdapterConfig::default()
        };
        let spec = NormalizedSpec::Agent(NormalizedAgentSpec {
            path: "test.md".into(),
            frontmatter: NormalizedAgentFrontmatter {
                id: "test-agent".to_string(),
                description: "Test agent".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: String::new(),
        });

        assert_eq!(model_facing_name(&spec, Some(&cfg)), "tw:test-agent");
    }

    #[test]
    fn test_body_tool_name_question_maps_to_ask_user_question() {
        assert_eq!(
            body_tool_name(&ToolFrontmatter::Question),
            "AskUserQuestion"
        );
    }

    #[test]
    fn test_body_tool_name_tasks_maps_to_todo_write() {
        assert_eq!(body_tool_name(&ToolFrontmatter::Tasks), "TodoWrite");
    }

    #[test]
    fn test_body_tool_name_subagent_maps_to_agent() {
        assert_eq!(body_tool_name(&ToolFrontmatter::Subagent), "Agent");
    }

    #[test]
    fn test_body_tool_name_skill_maps_to_skill() {
        assert_eq!(body_tool_name(&ToolFrontmatter::Skill), "Skill");
    }

    #[test]
    fn test_adapt_tool_subagent_maps_to_agent() {
        let tools = adapt_tool(&ToolFrontmatter::Subagent);
        let yaml = serde_yml::to_string(&tools).expect("expected value");
        assert_eq!(yaml, "- Agent\n");
    }

    #[test]
    fn test_adapt_tool_skill_maps_to_skill() {
        let tools = adapt_tool(&ToolFrontmatter::Skill);
        let yaml = serde_yml::to_string(&tools).expect("expected value");
        assert_eq!(yaml, "- Skill\n");
    }

    // -- Hook synthesis tests --

    fn make_hook_spec(id: &str, event: HookEvent, matcher: Option<&str>) -> NormalizedHookSpec {
        NormalizedHookSpec {
            path: std::path::PathBuf::from("/tmp/hooks.toml"),
            frontmatter: crate::spec::NormalizedHookFrontmatter {
                id: id.to_string(),
                event,
                script: format!("scripts/{id}.sh").into(),
                matcher: matcher.map(str::to_string),
                timeout: None,
                description: None,
                tags: None,
            },
            body: String::new(),
            supporting_files: Vec::new(),
        }
    }

    #[test]
    fn test_claude_event_name_full_mapping() {
        assert_eq!(claude_event_name(HookEvent::PreToolUse), "PreToolUse");
        assert_eq!(claude_event_name(HookEvent::PostToolUse), "PostToolUse");
        assert_eq!(
            claude_event_name(HookEvent::PostToolUseFailure),
            "PostToolUseFailure"
        );
        assert_eq!(claude_event_name(HookEvent::SessionStart), "SessionStart");
        assert_eq!(claude_event_name(HookEvent::SessionEnd), "SessionEnd");
        assert_eq!(claude_event_name(HookEvent::Stop), "Stop");
        assert_eq!(claude_event_name(HookEvent::PreCompact), "PreCompact");
        assert_eq!(claude_event_name(HookEvent::SubagentStart), "SubagentStart");
        assert_eq!(claude_event_name(HookEvent::SubagentStop), "SubagentStop");
        assert_eq!(
            claude_event_name(HookEvent::UserPromptSubmit),
            "UserPromptSubmit"
        );
    }

    #[test]
    fn test_synthesize_hooks_empty_returns_default() {
        let result = synthesize_hooks(&[], None).expect("expected value");
        assert!(result.entries.is_empty());
        assert!(result.files.is_empty());
    }

    #[test]
    fn test_synthesize_hooks_preserves_nested_script_path_in_command() {
        // Regression: `build_emitted_hook_entries` previously stripped the
        // script path to its basename via `file_name()`, breaking any
        // non-flat layout (e.g., `scripts/git/pre-commit.sh` would emit a
        // command pointing to `${CLAUDE_PLUGIN_ROOT}/hooks/scripts/pre-commit.sh`
        // — a path that does not exist, since the file was correctly emitted
        // to `hooks/scripts/git/pre-commit.sh`).
        let mut spec = make_hook_spec("audit", HookEvent::PreToolUse, Some("Bash"));
        spec.frontmatter.script = std::path::PathBuf::from("scripts/git/pre-commit.sh");
        let result = synthesize_hooks(&[&spec], None).expect("expected value");
        assert_eq!(
            result.entries[0].command,
            "${CLAUDE_PLUGIN_ROOT}/hooks/scripts/git/pre-commit.sh"
        );
    }

    #[test]
    fn test_synthesize_hooks_normalizes_leading_dot_slash_in_script_path() {
        // Regression: `Path::strip_prefix("scripts")` returns Err for
        // `./scripts/init.sh` (the leading CurDir component breaks prefix
        // matching), so the previous fix using `strip_prefix` would emit a
        // command like `${ANCHOR}/hooks/scripts/./scripts/init.sh`.
        // The component-based normalization handles both forms.
        let mut spec = make_hook_spec("init", HookEvent::SessionStart, None);
        spec.frontmatter.script = std::path::PathBuf::from("./scripts/init.sh");
        let result = synthesize_hooks(&[&spec], None).expect("expected value");
        assert_eq!(
            result.entries[0].command,
            "${CLAUDE_PLUGIN_ROOT}/hooks/scripts/init.sh"
        );
    }

    #[test]
    fn test_synthesize_hooks_does_not_serialize_description() {
        // `HookFrontmatter::description` is documented as informational;
        // neither provider's host runtime consumes it. Lock that contract:
        // a description on the spec must not appear in the emitted JSON.
        let mut spec = make_hook_spec("init", HookEvent::UserPromptSubmit, None);
        spec.frontmatter.description = Some("informational note".to_string());
        let result = synthesize_hooks(&[&spec], None).expect("expected value");
        let file = result
            .files
            .iter()
            .find(|f| f.path.to_str() == Some("hooks/hooks.json"))
            .expect("hooks.json should be present");
        let content = String::from_utf8(file.content.clone()).expect("expected utf-8");
        assert!(
            !content.contains("description") && !content.contains("informational note"),
            "description must not be serialized into Claude hooks.json, got: {content}"
        );
    }

    #[test]
    fn test_synthesize_hooks_path_mode_emits_bundled_file() {
        let spec = make_hook_spec("init", HookEvent::UserPromptSubmit, None);
        let specs = vec![&spec];
        let result = synthesize_hooks(&specs, None).expect("expected value");
        assert_eq!(result.entries.len(), 1);
        let file = result
            .files
            .iter()
            .find(|f| f.path.to_str() == Some("hooks/hooks.json"))
            .expect("hooks.json should be present");
        let content = String::from_utf8(file.content.clone()).expect("expected utf-8");
        assert!(
            content.contains("\"UserPromptSubmit\""),
            "expected PascalCase event name, got: {content}"
        );
        assert!(
            content.contains("\"_agentspec_id\": \"init\""),
            "expected _agentspec_id sentinel, got: {content}"
        );
        assert!(
            content.contains("${CLAUDE_PLUGIN_ROOT}/hooks/scripts/init.sh"),
            "expected CLAUDE_PLUGIN_ROOT-anchored command, got: {content}"
        );
    }

    #[test]
    fn test_synthesize_hooks_groups_by_event_and_matcher() {
        // Two hooks share (UserPromptSubmit, None) → land in one matcher group
        // with both entries; insertion order preserved.
        let a = make_hook_spec("a", HookEvent::UserPromptSubmit, None);
        let b = make_hook_spec("b", HookEvent::UserPromptSubmit, None);
        let specs = vec![&a, &b];
        let result = synthesize_hooks(&specs, None).expect("expected value");
        let json_file = result
            .files
            .iter()
            .find(|f| f.path.to_str() == Some("hooks/hooks.json"))
            .expect("hooks.json should be present");
        let content = String::from_utf8(json_file.content.clone()).expect("utf-8");
        // Find the position of each agentspec_id sentinel and check ordering.
        let a_pos = content.find("\"a\"").expect("a id");
        let b_pos = content.find("\"b\"").expect("b id");
        assert!(a_pos < b_pos, "expected insertion order preserved");
        // Both entries should land in a single matcher group (one inner array).
        // The inner-array key is `"hooks": [` (top-level uses `"hooks": {`).
        let inner = content.matches("\"hooks\": [").count();
        assert_eq!(
            inner, 1,
            "expected exactly one matcher-group hooks array, got {inner}: {content}"
        );
    }

    #[test]
    fn test_synthesize_hooks_merged_user_mode_emits_scripts_only() {
        // In Phase 2's Merged modes, agentspec emits hook scripts but not the
        // host config file (hooks.json) — that's owned by `ClaudeHooksPatch`,
        // which surgically merges entries into `<config>/settings.json`.
        // `entries` is still populated so the patcher can consume them.
        let cfg = AdapterConfig {
            hook_emit_mode: Some(HookEmitMode::MergedUser),
            ..AdapterConfig::default()
        };
        let spec = make_hook_spec("init", HookEvent::SessionStart, None);
        let result = synthesize_hooks(&[&spec], Some(&cfg)).expect("expected ok");
        assert_eq!(result.entries.len(), 1);
        assert!(
            !result
                .files
                .iter()
                .any(|f| f.path.to_str() == Some("hooks/hooks.json")),
            "Merged mode must NOT emit hooks/hooks.json; the patcher owns the host config"
        );
    }

    #[test]
    fn test_synthesize_hooks_merged_user_anchor_includes_plugin_root_assignment() {
        // MergedUser: command anchors to $HOME (not ~/...) per the plan AND
        // sets `CLAUDE_PLUGIN_ROOT` inline so plugin-shaped scripts that
        // reference `$CLAUDE_PLUGIN_ROOT/rules` etc. resolve correctly when
        // the host runtime doesn't set that variable for non-plugin scope.
        let cfg = AdapterConfig {
            hook_emit_mode: Some(HookEmitMode::MergedUser),
            ..AdapterConfig::default()
        };
        let spec = make_hook_spec("init", HookEvent::SessionStart, None);
        let result = synthesize_hooks(&[&spec], Some(&cfg)).expect("expected ok");
        assert_eq!(
            result.entries[0].command,
            "CLAUDE_PLUGIN_ROOT=$HOME/.claude $HOME/.claude/hooks/scripts/init.sh"
        );
    }

    #[test]
    fn test_synthesize_hooks_merged_project_anchor_includes_plugin_root_assignment() {
        // MergedProject: ${CLAUDE_PROJECT_DIR} anchor + inline CLAUDE_PLUGIN_ROOT.
        let cfg = AdapterConfig {
            hook_emit_mode: Some(HookEmitMode::MergedProject),
            ..AdapterConfig::default()
        };
        let spec = make_hook_spec("init", HookEvent::SessionStart, None);
        let result = synthesize_hooks(&[&spec], Some(&cfg)).expect("expected ok");
        assert_eq!(
            result.entries[0].command,
            "CLAUDE_PLUGIN_ROOT=${CLAUDE_PROJECT_DIR}/.claude ${CLAUDE_PROJECT_DIR}/.claude/hooks/scripts/init.sh"
        );
    }

    #[test]
    fn test_model_facing_name_falls_back_to_prefix() {
        let cfg = AdapterConfig {
            prefix: Some("tw".to_string()),
            content_prefix: None,
            ..AdapterConfig::default()
        };
        let spec = NormalizedSpec::Agent(NormalizedAgentSpec {
            path: "test.md".into(),
            frontmatter: NormalizedAgentFrontmatter {
                id: "test-agent".to_string(),
                description: "Test agent".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: String::new(),
        });

        assert_eq!(model_facing_name(&spec, Some(&cfg)), "tw-test-agent");
    }
}
