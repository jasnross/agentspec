use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
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

// Cursor's documented `hooks.json` shape (see <https://cursor.com/docs/hooks>):
//   { "version": 1, "hooks": { "<eventName>": [<entry>, <entry>, ...] } }
//
// `entry_to_cursor_json` is the per-entry shape (matcher per-entry, sentinel
// field). Mirrors `claude::entry_to_claude_json`; Phase 2's merge layer
// imports both helpers so the two emission paths stay in lockstep.

/// Build the JSON object for one entry in Cursor's `hooks.json` event array.
/// Cursor differs from Claude by placing `matcher` on each entry directly
/// (Claude wraps entries in matcher groups). The `_agentspec_id` sentinel is
/// emitted in both shapes for symmetric ownership tracking.
pub fn entry_to_cursor_json(e: &EmittedHookEntry) -> serde_json::Value {
    use serde_json::{Map, json};
    let mut obj = Map::new();
    obj.insert("type".to_string(), json!("command"));
    if let Some(m) = &e.matcher {
        obj.insert("matcher".to_string(), json!(m));
    }
    obj.insert("command".to_string(), json!(e.command));
    if let Some(t) = e.timeout {
        obj.insert("timeout".to_string(), json!(t));
    }
    obj.insert("_agentspec_id".to_string(), json!(e.agentspec_id));
    serde_json::Value::Object(obj)
}

/// Translate a canonical `HookEvent` to Cursor's camelCase event name.
///
/// Exposed publicly so the Phase 2 CST merge layer (`hooks_merge`) can
/// resolve event names without re-deriving the mapping.
///
/// Note that `user_prompt_submit` maps to `beforeSubmitPrompt` — not a simple
/// casing transform. See `thoughts/research/2026-05-03-provider-agnostic-hooks-comparison.md`
/// §2.3 for the documented event list. Several mappings (postToolUseFailure,
/// sessionStart, sessionEnd, subagentStart, subagentStop) are based on the
/// research doc's listing and may need adjustment if Cursor's docs diverge —
/// the plan calls out an empirical verification step before Phase 2 ships.
pub fn cursor_event_name(event: HookEvent) -> &'static str {
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

    let emit_mode = cfg
        .and_then(|c| c.hook_emit_mode)
        .unwrap_or(HookEmitMode::Bundled);

    let entries = build_emitted_hook_entries(specs, Provider::Cursor, emit_mode);
    let mut files = build_hook_script_files(Provider::Cursor, specs);
    if matches!(emit_mode, HookEmitMode::Bundled) {
        let json = build_cursor_hooks_json(&entries)?;
        files.push(GeneratedFile::text(
            Provider::Cursor,
            Path::new("hooks").join("hooks.json"),
            json,
        ));
    }
    Ok(HookSynthesis { entries, files })
}

/// Cursor places the `matcher` on each entry directly; entries within an
/// event preserve insertion order from the spec list. Per-entry serialization
/// delegates to the local `entry_to_cursor_json` helper so Phase 1 and Phase 2
/// share one source of truth for the entry shape.
fn build_cursor_hooks_json(entries: &[EmittedHookEntry]) -> Result<String> {
    use serde_json::{Map, Value, json};

    let mut by_event: BTreeMap<&'static str, Vec<Value>> = BTreeMap::new();
    for entry in entries {
        by_event
            .entry(cursor_event_name(entry.event))
            .or_default()
            .push(entry_to_cursor_json(entry));
    }

    let mut hooks_map = Map::new();
    for (event, hook_entries) in by_event {
        hooks_map.insert(event.to_string(), Value::Array(hook_entries));
    }

    let top = json!({ "version": 1, "hooks": hooks_map });
    let json =
        serde_json::to_string_pretty(&top).context("failed to serialize Cursor hooks.json")?;
    Ok(format!("{json}\n"))
}

/// Post-write hook that merges agentspec-owned hook entries into Cursor's
/// hand-edited `hooks.json` via the CST patcher in `hooks_merge`.
///
/// Cursor's host config file is the same name as the Path-mode bundle
/// (`hooks.json`) but lives at the config-dir root rather than under
/// `hooks/`. The patcher is constructed only for `MergedUser`/`MergedProject`
/// modes; Path mode emits the file directly.
#[derive(Debug)]
pub struct CursorHooksPatch {
    hooks_path: std::path::PathBuf,
    owned_entries: Vec<EmittedHookEntry>,
    /// `--force`/`overwrite=true`: replace a non-object `hooks` value with
    /// `{}` before merging, instead of erroring.
    force: bool,
}

impl PostWriteHook for CursorHooksPatch {
    fn run(&self, dry_run: bool) -> Result<()> {
        crate::hooks_merge::merge_cursor_hooks(
            &self.hooks_path,
            &self.owned_entries,
            self.force,
            dry_run,
        )
    }
}

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
        return None;
    }
    Some(Box::new(CursorHooksPatch {
        hooks_path: config_dir.join("hooks.json"),
        owned_entries: owned_entries.to_vec(),
        force,
    }))
}

/// Post-write hook that strips agentspec-owned hook entries from Cursor's
/// `hooks.json` and tidies emptied containers, paralleling
/// [`CursorHooksPatch`] but in reverse. Ownership is identified by the
/// on-disk `_agentspec_id` sentinel.
#[derive(Debug)]
pub struct CursorRemoveHooksPatch {
    hooks_path: std::path::PathBuf,
}

impl PostWriteHook for CursorRemoveHooksPatch {
    fn run(&self, dry_run: bool) -> Result<()> {
        let report = crate::hooks_merge::remove_cursor_hooks(&self.hooks_path, dry_run)?;
        report.print_summary();
        Ok(())
    }
}

/// Factory for Cursor's remove post-write hook.
///
/// `_dest` is accepted for signature symmetry with the `OpenCode` factory and
/// `sync`'s `post_write_hook` — Cursor identifies its targets by on-disk
/// `_agentspec_id` sentinels. The uniform signature lets `remove.rs`
/// dispatch through identically-shaped match arms.
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
    Some(Box::new(CursorRemoveHooksPatch {
        hooks_path: config_dir.join("hooks.json"),
    }))
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
    fn test_synthesize_hooks_does_not_serialize_description() {
        // `HookFrontmatter::description` is documented as informational;
        // neither provider's host runtime consumes it. Lock that contract:
        // a description on the spec must not appear in the emitted JSON.
        let mut spec = make_hook_spec("init", HookEvent::SessionStart, None);
        spec.frontmatter.description = Some("informational note".to_string());
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
            !content.contains("description") && !content.contains("informational note"),
            "description must not be serialized into Cursor hooks.json, got: {content}"
        );
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
    fn test_synthesize_hooks_merged_user_emits_scripts_no_hooks_json() {
        // Merged modes hand JSON emission to `CursorHooksPatch`. Scripts still
        // flow through to disk; hooks.json itself does not (the patcher edits
        // the host `<config>/hooks.json` instead).
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
            "Merged mode must NOT emit hooks/hooks.json"
        );
        // Anchor uses $HOME for User mode AND sets CLAUDE_PLUGIN_ROOT inline
        // (Cursor aliases the var at plugin scope; outside that scope agentspec
        // sets it explicitly so plugin-shaped scripts keep working).
        assert_eq!(
            result.entries[0].command,
            "CLAUDE_PLUGIN_ROOT=$HOME/.cursor $HOME/.cursor/hooks/scripts/init.sh"
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

        let files = adapt_cursor(spec, &HashMap::new(), Some(&cfg)).expect("expected value");
        assert_eq!(files[0].path.to_str(), Some("rules/tw-test-rule.mdc"));
    }
}
