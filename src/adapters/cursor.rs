use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
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

// See: https://cursor.com/docs/subagents#configuration-fields
#[derive(Serialize)]
struct CursorAgentFrontmatter {
    name: String,
    description: String,
    model: Option<String>,
}

// See: https://cursor.com/docs/skills#frontmatter-fields
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CursorSkillFrontmatter {
    name: String,
    description: String,
    disable_model_invocation: bool,
}

// See: https://cursor.com/docs/rules#rule-file-format
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CursorRuleFrontmatter {
    description: String,
    always_apply: bool,
}

pub fn adapt_cursor(
    spec: NormalizedSpec,
    presets: &ProviderPresetsMap,
    cfg: Option<&AdapterConfig>,
) -> Result<Vec<GeneratedFile>> {
    match spec {
        NormalizedSpec::Agent(s) => adapt_agent_spec(s, presets, cfg),
        NormalizedSpec::Skill(s) => adapt_skill_spec(s, cfg),
        NormalizedSpec::Rule(s) => adapt_rule_spec(s, cfg),
        // Hook scripts are emitted by `synthesize_hooks` once per provider —
        // see the matching note in `claude.rs::adapt_claude` for the rationale.
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
        .and_then(|x| x.cursor.clone())
        .and_then(|x| x.model);

    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let path = Path::new("agents").join(format!("{file_prefix}{id}.md"));

    // Cursor agents get frontmatter name prefix with "-" delimiter
    let name = match cfg.and_then(|c| c.prefix.as_deref()) {
        Some(prefix) => format!("{prefix}-{id}"),
        None => id,
    };

    let frontmatter = CursorAgentFrontmatter {
        name,
        description,
        model,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}---\n\n{body}");

    Ok(vec![GeneratedFile::text(Provider::Cursor, path, content)])
}

fn adapt_skill_spec(
    spec: NormalizedSkillSpec,
    cfg: Option<&AdapterConfig>,
) -> Result<Vec<GeneratedFile>> {
    let id = spec.frontmatter.id;
    let description = spec.frontmatter.description.unwrap_or_default();

    let name = match cfg.and_then(|c| c.prefix.as_deref()) {
        Some(prefix) => format!("{prefix}-{id}"),
        None => id.clone(),
    };

    let frontmatter = CursorSkillFrontmatter {
        name,
        description,
        disable_model_invocation: !spec.frontmatter.agent_invocable,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}---\n\n{body}");

    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let skill_dir = Path::new("skills").join(format!("{file_prefix}{id}"));

    let mut files = vec![GeneratedFile::text(
        Provider::Cursor,
        skill_dir.join("SKILL.md"),
        content,
    )];

    for sf in spec.supporting_files {
        files.push(GeneratedFile::binary(
            Provider::Cursor,
            skill_dir.join(&sf.relative_path),
            sf.content,
            if sf.executable { Some(0o755) } else { None },
        ));
    }

    Ok(files)
}

fn adapt_rule_spec(
    spec: NormalizedRuleSpec,
    cfg: Option<&AdapterConfig>,
) -> Result<Vec<GeneratedFile>> {
    let description = spec.frontmatter.description.unwrap_or_default();

    let frontmatter = CursorRuleFrontmatter {
        description,
        always_apply: true,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}---\n\n{body}");

    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let path = Path::new("rules").join(format!("{file_prefix}{}.mdc", spec.frontmatter.id));

    Ok(vec![GeneratedFile::text(Provider::Cursor, path, content)])
}

/// Resolve a canonical tool to the name a Cursor spec body should reference.
///
/// Returns either a display label sourced from `cursor.com/docs/agent/tools`
/// (or `cursor.com/docs/subagents` for `Subagent`) or a descriptive phrase
/// when Cursor documents no equivalent capability. Descriptive-phrase arms
/// are marked inline.
pub fn body_tool_name(tool: &ToolFrontmatter) -> &'static str {
    match tool {
        ToolFrontmatter::Read => "Read files",
        ToolFrontmatter::Write | ToolFrontmatter::Edit => "Edit files",
        ToolFrontmatter::Grep | ToolFrontmatter::Glob => "Search files and folders",
        ToolFrontmatter::Bash => "Run shell commands",
        ToolFrontmatter::WebSearch => "Web",
        ToolFrontmatter::WebFetch => "URL fetcher", // descriptive: Cursor docs name no URL-fetch tool
        ToolFrontmatter::Question => "Ask questions",
        ToolFrontmatter::Tasks => "TODO tracker", // descriptive: Cursor docs name no TODO-list tool
        ToolFrontmatter::Subagent => "Task",
        ToolFrontmatter::Skill => "Skill runner", // descriptive: Cursor docs name no skill-invocation tool
    }
}

// ── hooks.json synthesis ────────────────────────────────────────────────────

/// Cursor's documented `hooks.json` shape: a top-level object with a fixed
/// `version: 1` field plus a `hooks` map. Each event maps directly to a list
/// of entries — matchers live on the entry, not on a wrapping group.
/// See: <https://cursor.com/docs/hooks>
#[derive(Serialize)]
struct CursorHooksJson {
    version: u32,
    hooks: BTreeMap<&'static str, Vec<CursorHookEntry>>,
}

#[serde_with::skip_serializing_none]
#[derive(Serialize)]
struct CursorHookEntry {
    #[serde(rename = "type")]
    type_field: &'static str,
    matcher: Option<String>,
    command: String,
    timeout: Option<u32>,
    #[serde(rename = "_agentspec_id")]
    agentspec_id: String,
}

/// Translate a canonical `HookEvent` to Cursor's camelCase event name.
///
/// Note that `user_prompt_submit` maps to `beforeSubmitPrompt` — not a simple
/// casing transform. See `thoughts/research/2026-05-03-provider-agnostic-hooks-comparison.md`
/// §2.3 for the documented event list. Several mappings (postToolUseFailure,
/// sessionStart, sessionEnd, subagentStart, subagentStop) are based on the
/// research doc's listing and may need adjustment if Cursor's docs diverge —
/// the plan calls out an empirical verification step before Phase 2 ships.
fn cursor_event_name(event: HookEvent) -> &'static str {
    match event {
        HookEvent::PreToolUse => "preToolUse",
        HookEvent::PostToolUse => "postToolUse",
        HookEvent::PostToolUseFailure => "postToolUseFailure",
        HookEvent::SessionStart => "sessionStart",
        HookEvent::SessionEnd => "sessionEnd",
        HookEvent::Stop => "stop",
        HookEvent::PreCompact => "preCompact",
        HookEvent::SubagentStart => "subagentStart",
        HookEvent::SubagentStop => "subagentStop",
        HookEvent::UserPromptSubmit => "beforeSubmitPrompt",
    }
}

/// Synthesize the per-provider `hooks/hooks.json` plus the canonical entry
/// list for downstream merge in Phase 2.
pub fn synthesize_hooks(
    specs: &[&NormalizedHookSpec],
    cfg: Option<&AdapterConfig>,
) -> Result<HookSynthesis> {
    if specs.is_empty() {
        return Ok(HookSynthesis::default());
    }

    let mode = cfg
        .and_then(|c| c.hook_emit_mode)
        .unwrap_or(HookEmitMode::Bundled);
    if matches!(mode, HookEmitMode::Merged) {
        bail!("hooks emit for Project/User mode is not yet implemented (lands in Phase 2)");
    }

    let entries = build_emitted_hook_entries(specs);
    let json = build_cursor_hooks_json(&entries)?;
    let mut files = build_hook_script_files(Provider::Cursor, specs);
    files.push(GeneratedFile::text(
        Provider::Cursor,
        Path::new("hooks").join("hooks.json"),
        json,
    ));
    Ok(HookSynthesis { entries, files })
}

/// Cursor places the `matcher` on each entry directly; entries within an
/// event preserve insertion order from the spec list.
fn build_cursor_hooks_json(entries: &[EmittedHookEntry]) -> Result<String> {
    let mut by_event: BTreeMap<&'static str, Vec<CursorHookEntry>> = BTreeMap::new();
    for entry in entries {
        by_event
            .entry(cursor_event_name(entry.event))
            .or_default()
            .push(CursorHookEntry {
                type_field: "command",
                matcher: entry.matcher.clone(),
                command: entry.command.clone(),
                timeout: entry.timeout,
                agentspec_id: entry.agentspec_id.clone(),
            });
    }

    let json = serde_json::to_string_pretty(&CursorHooksJson {
        version: 1,
        hooks: by_event,
    })
    .context("failed to serialize Cursor hooks.json")?;
    Ok(format!("{json}\n"))
}

pub fn post_write_hook(
    _kind: FileKind,
    _dest: &Path,
    _config_dir: &Path,
) -> Option<Box<dyn PostWriteHook>> {
    None
}

/// Returns the name the AI model uses to reference this spec.
///
/// For Cursor, all spec types use `{content_prefix}{id}` when a content prefix
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

    use super::*;
    use crate::spec::{
        NormalizedAgentFrontmatter, NormalizedAgentSpec, NormalizedRuleFrontmatter,
        NormalizedRuleSpec, NormalizedSkillFrontmatter, NormalizedSkillSpec,
    };

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

        let files = adapt_cursor(spec, &HashMap::new(), None).expect("expected value");
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");

        let expected = concat!(
            "---\n",
            "name: test-agent\n",
            "description: Test agent\n",
            "model: null\n",
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

        let files = adapt_cursor(spec, &HashMap::new(), Some(&cfg)).expect("expected value");
        assert_eq!(files[0].path.to_string_lossy(), "agents/tw-test-agent.md");
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");
        assert!(
            content.contains("name: tw-test-agent"),
            "expected prefixed name with '-' delimiter, got: {content}"
        );
    }

    #[test]
    fn test_adapt_skill_with_prefix() {
        let cfg = AdapterConfig {
            prefix: Some("tw".to_string()),
            content_prefix: None,
            ..AdapterConfig::default()
        };
        let spec = NormalizedSpec::Skill(NormalizedSkillSpec {
            path: "test.md".into(),
            frontmatter: NormalizedSkillFrontmatter {
                id: "test-skill".to_string(),
                description: Some("A test skill".to_string()),
                tags: None,
                execution: None,
                capabilities: None,
                user_invocable: true,
                agent_invocable: true,
            },
            body: "Body.".to_string(),
            supporting_files: vec![],
        });

        let files = adapt_cursor(spec, &HashMap::new(), Some(&cfg)).expect("expected value");
        assert_eq!(
            files[0].path.to_string_lossy(),
            "skills/tw-test-skill/SKILL.md"
        );
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");
        assert!(
            content.contains("name: tw-test-skill"),
            "expected prefixed name with '-' delimiter, got: {content}"
        );
    }

    #[test]
    fn test_body_tool_name_full_mapping() {
        assert_eq!(body_tool_name(&ToolFrontmatter::Read), "Read files");
        assert_eq!(body_tool_name(&ToolFrontmatter::Write), "Edit files");
        assert_eq!(body_tool_name(&ToolFrontmatter::Edit), "Edit files");
        assert_eq!(
            body_tool_name(&ToolFrontmatter::Grep),
            "Search files and folders"
        );
        assert_eq!(
            body_tool_name(&ToolFrontmatter::Glob),
            "Search files and folders"
        );
        assert_eq!(body_tool_name(&ToolFrontmatter::Bash), "Run shell commands");
        assert_eq!(body_tool_name(&ToolFrontmatter::WebSearch), "Web");
        assert_eq!(body_tool_name(&ToolFrontmatter::WebFetch), "URL fetcher");
        assert_eq!(body_tool_name(&ToolFrontmatter::Question), "Ask questions");
        assert_eq!(body_tool_name(&ToolFrontmatter::Tasks), "TODO tracker");
        assert_eq!(body_tool_name(&ToolFrontmatter::Subagent), "Task");
        assert_eq!(body_tool_name(&ToolFrontmatter::Skill), "Skill runner");
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
    fn test_cursor_event_name_user_prompt_submit_special_case() {
        // The one mapping that isn't a simple casing transform.
        assert_eq!(
            cursor_event_name(HookEvent::UserPromptSubmit),
            "beforeSubmitPrompt"
        );
    }

    #[test]
    fn test_cursor_event_name_full_mapping() {
        assert_eq!(cursor_event_name(HookEvent::PreToolUse), "preToolUse");
        assert_eq!(cursor_event_name(HookEvent::PostToolUse), "postToolUse");
        assert_eq!(
            cursor_event_name(HookEvent::PostToolUseFailure),
            "postToolUseFailure"
        );
        assert_eq!(cursor_event_name(HookEvent::SessionStart), "sessionStart");
        assert_eq!(cursor_event_name(HookEvent::SessionEnd), "sessionEnd");
        assert_eq!(cursor_event_name(HookEvent::Stop), "stop");
        assert_eq!(cursor_event_name(HookEvent::PreCompact), "preCompact");
        assert_eq!(cursor_event_name(HookEvent::SubagentStart), "subagentStart");
        assert_eq!(cursor_event_name(HookEvent::SubagentStop), "subagentStop");
    }

    #[test]
    fn test_synthesize_hooks_emits_version_field() {
        let spec = make_hook_spec("init", HookEvent::SessionStart, None);
        let result = synthesize_hooks(&[&spec], None).expect("expected value");
        let content = String::from_utf8(
            result
                .files
                .iter()
                .find(|f| f.path.to_str() == Some("hooks/hooks.json"))
                .expect("hooks.json should be present")
                .content
                .clone(),
        )
        .expect("expected utf-8");
        assert!(
            content.contains("\"version\": 1"),
            "expected version field, got: {content}"
        );
    }

    #[test]
    fn test_synthesize_hooks_per_entry_matcher_placement() {
        // Cursor places `matcher` on each entry; verify it appears alongside
        // `command` in a single object literal (not as a group key).
        let spec = make_hook_spec("audit", HookEvent::PreToolUse, Some("Bash"));
        let result = synthesize_hooks(&[&spec], None).expect("expected value");
        let content = String::from_utf8(
            result
                .files
                .iter()
                .find(|f| f.path.to_str() == Some("hooks/hooks.json"))
                .expect("hooks.json should be present")
                .content
                .clone(),
        )
        .expect("expected utf-8");
        // Must contain matcher field on the entry (with same indentation as
        // sibling fields like `command`).
        assert!(
            content.contains("\"matcher\": \"Bash\""),
            "expected per-entry matcher, got: {content}"
        );
    }

    #[test]
    fn test_synthesize_hooks_merged_mode_errors_in_phase_one() {
        let cfg = AdapterConfig {
            hook_emit_mode: Some(HookEmitMode::Merged),
            ..AdapterConfig::default()
        };
        let spec = make_hook_spec("init", HookEvent::SessionStart, None);
        let err = synthesize_hooks(&[&spec], Some(&cfg)).expect_err("expected Phase 2 error");
        let msg = format!("{err:#}");
        assert!(msg.contains("Phase 2"), "got: {msg}");
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

        let files = adapt_cursor(spec, &HashMap::new(), Some(&cfg)).expect("expected value");
        assert_eq!(files[0].path.to_str(), Some("rules/tw-test-rule.mdc"));
    }
}
