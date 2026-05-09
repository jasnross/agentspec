use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use jsonc_parser::cst::{CstInputValue, CstObject};
use serde::Serialize;

use super::hook_compile::{build_emitted_hook_entries, build_hook_script_files};
use super::hooks_helpers::{
    is_owned_entry, open_or_create_array, open_or_create_object, prune_empty_event_arrays,
    value_to_cst_input,
};
use super::{
    Adapter, AdapterOutput, CompileCtx, HookAdapter, ProviderAdapter, RemoveCtx,
    SyncDestinationMode, TidyOutcome,
};
use crate::compile::{AdapterConfig, EmittedHookEntry, GeneratedFile, HookEmitMode, HookSynthesis};
use crate::hooks_merge::{HooksPatch, RemoveHooksPatch};
use crate::plan::{ConfigPatch, FileKind, PatchBridge, PostWriteHook, expand_tilde};
use crate::presets::ProviderPresetsMap;
use crate::provider::Provider;
use crate::spec::{AgentSpec, HookEvent, HookSpec, RuleSpec, SkillSpec, Spec, ToolFrontmatter};

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

/// Zero-sized adapter for the Cursor provider.
#[derive(Debug)]
pub struct CursorAdapter;

impl Adapter for CursorAdapter {
    fn compile(&self, specs: &[Spec], ctx: &CompileCtx<'_>) -> Result<AdapterOutput> {
        let mut files = Vec::new();
        for spec in specs {
            let mut adapted =
                ProviderAdapter::adapt(self, spec.clone(), ctx.presets, ctx.adapter_config)?;
            files.append(&mut adapted);
        }

        let hook_specs: Vec<&HookSpec> = specs
            .iter()
            .filter_map(|s| if let Spec::Hook(h) = s { Some(h) } else { None })
            .collect();
        let synthesis = HookAdapter::synthesize_hooks(self, &hook_specs, ctx.adapter_config)?;
        files.extend(synthesis.files);
        let owned_entries = synthesis.entries;

        let dest_root = ProviderAdapter::config_dir(
            self,
            ctx.mode,
            ctx.target_dir.and_then(Path::to_str),
            ctx.home,
            ctx.cwd,
        );
        let emit_mode = ctx.mode.to_hook_emit_mode();

        let mut patches: Vec<Box<dyn ConfigPatch>> = Vec::new();
        for &kind in ProviderAdapter::file_kinds(self) {
            let dest = dest_root.join(kind.dir_name());
            if let Some(hook) = ProviderAdapter::post_write_hook(
                self,
                kind,
                &dest,
                &dest_root,
                emit_mode,
                &owned_entries,
                ctx.overwrite,
            ) {
                let host_path = dest_root.join(HookAdapter::host_filename(self));
                patches.push(Box::new(PatchBridge::forward(hook, host_path)));
            }
        }

        Ok(AdapterOutput {
            files,
            patches,
            dest_root,
        })
    }

    fn removal_patches(&self, ctx: &RemoveCtx<'_>) -> Vec<Box<dyn ConfigPatch>> {
        let dest_root = ProviderAdapter::config_dir(
            self,
            ctx.mode,
            ctx.target_dir.and_then(Path::to_str),
            ctx.home,
            ctx.cwd,
        );
        let emit_mode = ctx.mode.to_hook_emit_mode();
        let mut patches: Vec<Box<dyn ConfigPatch>> = Vec::new();
        for &kind in ProviderAdapter::file_kinds(self) {
            let dest = dest_root.join(kind.dir_name());
            if let Some(hook) =
                ProviderAdapter::remove_post_write_hook(self, kind, &dest, &dest_root, emit_mode)
            {
                let host_path = dest_root.join(HookAdapter::host_filename(self));
                patches.push(Box::new(PatchBridge::reverse(hook, host_path)));
            }
        }
        patches
    }

    fn body_tool_name(&self, tool: &ToolFrontmatter) -> &'static str {
        ProviderAdapter::body_tool_name(self, tool)
    }

    fn model_facing_name(&self, spec: &Spec, cfg: Option<&AdapterConfig>) -> String {
        ProviderAdapter::model_facing_name(self, spec, cfg)
    }
}

impl ProviderAdapter for CursorAdapter {
    fn adapt(
        &self,
        spec: Spec,
        presets: &ProviderPresetsMap,
        cfg: Option<&AdapterConfig>,
    ) -> Result<Vec<GeneratedFile>> {
        match spec {
            Spec::Agent(s) => adapt_agent_spec(s, presets, cfg),
            Spec::Skill(s) => adapt_skill_spec(s, cfg),
            Spec::Rule(s) => adapt_rule_spec(s, cfg),
            // Hook scripts are emitted by `synthesize_hooks` once per provider —
            // see the matching note in `claude::ClaudeAdapter::adapt` for the
            // rationale.
            Spec::Hook(_) => Ok(Vec::new()),
        }
    }

    /// Resolve a canonical tool to the name a Cursor spec body should reference.
    ///
    /// Returns either a display label sourced from `cursor.com/docs/agent/tools`
    /// (or `cursor.com/docs/subagents` for `Subagent`) or a descriptive phrase
    /// when Cursor documents no equivalent capability. Descriptive-phrase arms
    /// are marked inline.
    fn body_tool_name(&self, tool: &ToolFrontmatter) -> &'static str {
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

    /// Returns the name the AI model uses to reference this spec.
    ///
    /// For Cursor, all spec types use `{content_prefix}{id}` when a content prefix
    /// is configured (either explicitly or derived from `prefix`).
    fn model_facing_name(&self, spec: &Spec, cfg: Option<&AdapterConfig>) -> String {
        let id = spec.id();
        match cfg.and_then(AdapterConfig::content_prefix) {
            Some(prefix) => format!("{prefix}{id}"),
            None => id.to_owned(),
        }
    }

    fn post_write_hook(
        &self,
        kind: FileKind,
        _dest: &Path,
        config_dir: &Path,
        emit_mode: HookEmitMode,
        owned_entries: &[EmittedHookEntry],
        overwrite: bool,
    ) -> Option<Box<dyn PostWriteHook>> {
        if kind != FileKind::Hooks {
            return None;
        }
        if !emit_mode.is_merged() {
            return None;
        }
        Some(Box::new(HooksPatch {
            adapter: &CursorAdapter,
            host_path: config_dir.join(self.host_filename()),
            owned_entries: owned_entries.to_vec(),
            force: overwrite,
        }))
    }

    /// Factory for Cursor's remove post-write hook.
    ///
    /// `_dest` is accepted for signature symmetry — Cursor identifies its
    /// targets by on-disk `_agentspec_id` sentinels.
    fn remove_post_write_hook(
        &self,
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
        Some(Box::new(RemoveHooksPatch {
            adapter: &CursorAdapter,
            host_path: config_dir.join(self.host_filename()),
        }))
    }

    fn file_kinds(&self) -> &'static [FileKind] {
        &[
            FileKind::Agents,
            FileKind::Rules,
            FileKind::Skills,
            FileKind::Hooks,
        ]
    }

    fn user_dest_dir(&self, home: &Path, kind: FileKind) -> PathBuf {
        home.join(".cursor").join(kind.dir_name())
    }

    fn project_dest_dir(&self, cwd: &Path, kind: FileKind) -> PathBuf {
        cwd.join(".cursor").join(kind.dir_name())
    }

    fn config_dir(
        &self,
        mode: SyncDestinationMode,
        dir: Option<&str>,
        home: &Path,
        cwd: &Path,
    ) -> PathBuf {
        match mode {
            SyncDestinationMode::User => home.join(".cursor"),
            SyncDestinationMode::Project => cwd.join(".cursor"),
            SyncDestinationMode::Path => {
                dir.map_or_else(|| home.join(".cursor"), |d| expand_tilde(d, home))
            }
        }
    }
}

impl HookAdapter for CursorAdapter {
    /// Synthesize the per-provider `hooks/hooks.json` plus the canonical entry
    /// list for the downstream merged-mode merge.
    fn synthesize_hooks(
        &self,
        specs: &[&HookSpec],
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

    /// Translate a canonical `HookEvent` to Cursor's camelCase event name.
    ///
    /// Note that `user_prompt_submit` maps to `beforeSubmitPrompt` — not a simple
    /// casing transform. See `thoughts/research/2026-05-03-provider-agnostic-hooks-comparison.md`
    /// §2.3 for the documented event list. Several mappings (postToolUseFailure,
    /// sessionStart, sessionEnd, subagentStart, subagentStop) are based on the
    /// research doc's listing and may need adjustment if Cursor's docs diverge —
    /// empirical verification against a real Cursor build is still pending.
    fn event_name(&self, event: HookEvent) -> &'static str {
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

    /// Build the JSON object for one entry in Cursor's `hooks.json` event array.
    /// Cursor differs from Claude by placing `matcher` on each entry directly
    /// (Claude wraps entries in matcher groups). The `_agentspec_id` sentinel is
    /// emitted in both shapes for symmetric ownership tracking.
    fn entry_to_json(&self, e: &EmittedHookEntry) -> serde_json::Value {
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

    fn hook_command_dotdir(&self) -> &'static str {
        ".cursor"
    }

    fn host_filename(&self) -> &'static str {
        "hooks.json"
    }

    fn merge_into(
        &self,
        top: &CstObject,
        owned_entries: &[EmittedHookEntry],
        force: bool,
    ) -> Result<()> {
        // Set `version: 1` if missing. Don't overwrite a user-authored value,
        // even if it's a different version — the user's intent wins.
        // Order matters: the version injection runs before the hooks-object
        // open. The shell's no-op-skip guard in `hooks_merge::merge_owned`
        // returns early only when both `owned_entries` is empty AND
        // `top.get("hooks")` is `None`, so a user with `hooks: { ... }` but no
        // agentspec entries this run still gets `version: 1` injected if absent.
        if top.get("version").is_none() {
            top.append("version", CstInputValue::Number("1".to_string()));
        }

        let hooks_obj = open_or_create_object(top, "hooks", force, "hooks")?;

        // Step 1 — remove every agentspec-owned entry under every event. Sync
        // doesn't care about the removed-count, so discard.
        let _ = remove_owned_entries(&hooks_obj);

        // Step 2 — append new entries directly under their event arrays.
        // `BTreeMap` sort order matches `build_cursor_hooks_json`'s emission
        // order, so newly-created event keys land alphabetically.
        let mut by_event: BTreeMap<&'static str, Vec<&EmittedHookEntry>> = BTreeMap::new();
        for e in owned_entries {
            by_event
                .entry(self.event_name(e.event))
                .or_default()
                .push(e);
        }
        for (event_name, entries) in &by_event {
            let event_arr = open_or_create_array(
                &hooks_obj,
                event_name,
                force,
                &format!("hooks.{event_name}"),
            )?;
            for &e in entries {
                event_arr.append(value_to_cst_input(self.entry_to_json(e)));
            }
        }
        Ok(())
    }

    fn tidy_after_remove(&self, top: &CstObject) -> TidyOutcome {
        let Some(hooks_obj) = top.object_value("hooks") else {
            return TidyOutcome {
                user_entries_remaining: 0,
                file_should_be_deleted: false,
            };
        };

        let removed_owned = remove_owned_entries(&hooks_obj);
        prune_empty_event_arrays(&hooks_obj);

        if hooks_obj.properties().is_empty()
            && let Some(hooks_prop) = top.get("hooks")
        {
            hooks_prop.remove();
        }

        // Cursor predicate: delete iff we actually removed at least one
        // agentspec-owned entry AND the residual is either empty OR exactly
        // one `version` key (any value). Cursor-exclusive — sync injects
        // `version: 1` if absent and never overwrites a user value, so a
        // residual `{version: <n>}` carries no information beyond file existence.
        let surviving = top.properties();
        let only_version_remains = surviving.len() == 1 && top.get("version").is_some();
        let file_should_be_deleted =
            removed_owned > 0 && (surviving.is_empty() || only_version_remains);

        TidyOutcome {
            user_entries_remaining: count_user_entries(top),
            file_should_be_deleted,
        }
    }
}

/// Cursor analog of `claude::remove_owned_entries`. Cursor's shape is one
/// nesting level shallower (no matcher-group wrapper), so this walks
/// `hooks.<event>[]` directly. Returns the count of `_agentspec_id`-tagged
/// entries removed; merge callers can ignore the count.
fn remove_owned_entries(hooks_obj: &CstObject) -> usize {
    let mut removed = 0usize;
    let event_props: Vec<_> = hooks_obj.properties();
    for event_prop in event_props {
        let Some(event_arr) = event_prop.array_value() else {
            continue;
        };
        let entries: Vec<_> = event_arr.elements();
        for entry in entries {
            if is_owned_entry(&entry) {
                entry.remove();
                removed += 1;
            }
        }
    }
    removed
}

/// Counts user-authored Cursor hook entries: walks every surviving event
/// array and counts elements lacking `_agentspec_id`.
fn count_user_entries(top: &CstObject) -> usize {
    let Some(hooks_obj) = top.object_value("hooks") else {
        return 0;
    };
    let mut count = 0;
    for event_prop in hooks_obj.properties() {
        let Some(event_arr) = event_prop.array_value() else {
            continue;
        };
        for entry in event_arr.elements() {
            if !is_owned_entry(&entry) {
                count += 1;
            }
        }
    }
    count
}

fn adapt_agent_spec(
    spec: AgentSpec,
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

fn adapt_skill_spec(spec: SkillSpec, cfg: Option<&AdapterConfig>) -> Result<Vec<GeneratedFile>> {
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
            Some(sf.mode),
        ));
    }

    Ok(files)
}

fn adapt_rule_spec(spec: RuleSpec, cfg: Option<&AdapterConfig>) -> Result<Vec<GeneratedFile>> {
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

// ── hooks.json synthesis ────────────────────────────────────────────────────

// Cursor's documented `hooks.json` shape (see <https://cursor.com/docs/hooks>):
//   { "version": 1, "hooks": { "<eventName>": [<entry>, <entry>, ...] } }
//
// Per-entry shape lives on `HookAdapter::entry_to_json` (matcher per-entry,
// sentinel field). The CST-aware merge layer calls it via the trait so the
// two emission paths stay in lockstep.

/// Cursor places the `matcher` on each entry directly; entries within an
/// event preserve insertion order from the spec list. Per-entry serialization
/// delegates to `CursorAdapter::entry_to_json` so the bundled emission path
/// and the merged-mode merge layer share one source of truth for the entry
/// shape.
fn build_cursor_hooks_json(entries: &[EmittedHookEntry]) -> Result<String> {
    use serde_json::{Map, Value, json};

    let mut by_event: BTreeMap<&'static str, Vec<Value>> = BTreeMap::new();
    for entry in entries {
        by_event
            .entry(CursorAdapter.event_name(entry.event))
            .or_default()
            .push(CursorAdapter.entry_to_json(entry));
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::spec::{
        AgentFrontmatter, AgentSpec, RuleFrontmatter, RuleSpec, SkillFrontmatter, SkillSpec,
    };

    #[test]
    fn test_adapt_agent_output_format() {
        let spec = Spec::Agent(AgentSpec {
            path: "test.md".into(),
            frontmatter: AgentFrontmatter {
                id: "test-agent".to_string(),
                description: "Test agent".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: "Body.".to_string(),
        });

        let files = CursorAdapter
            .adapt(spec, &HashMap::new(), None)
            .expect("expected value");
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
        let spec = Spec::Agent(AgentSpec {
            path: "test.md".into(),
            frontmatter: AgentFrontmatter {
                id: "test-agent".to_string(),
                description: "Test agent".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: "Body.".to_string(),
        });

        let files = CursorAdapter
            .adapt(spec, &HashMap::new(), Some(&cfg))
            .expect("expected value");
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
        let spec = Spec::Skill(SkillSpec {
            path: "test.md".into(),
            frontmatter: SkillFrontmatter {
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

        let files = CursorAdapter
            .adapt(spec, &HashMap::new(), Some(&cfg))
            .expect("expected value");
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
        assert_eq!(
            ProviderAdapter::body_tool_name(&CursorAdapter, &ToolFrontmatter::Read),
            "Read files"
        );
        assert_eq!(
            ProviderAdapter::body_tool_name(&CursorAdapter, &ToolFrontmatter::Write),
            "Edit files"
        );
        assert_eq!(
            ProviderAdapter::body_tool_name(&CursorAdapter, &ToolFrontmatter::Edit),
            "Edit files"
        );
        assert_eq!(
            ProviderAdapter::body_tool_name(&CursorAdapter, &ToolFrontmatter::Grep),
            "Search files and folders"
        );
        assert_eq!(
            ProviderAdapter::body_tool_name(&CursorAdapter, &ToolFrontmatter::Glob),
            "Search files and folders"
        );
        assert_eq!(
            ProviderAdapter::body_tool_name(&CursorAdapter, &ToolFrontmatter::Bash),
            "Run shell commands"
        );
        assert_eq!(
            ProviderAdapter::body_tool_name(&CursorAdapter, &ToolFrontmatter::WebSearch),
            "Web"
        );
        assert_eq!(
            ProviderAdapter::body_tool_name(&CursorAdapter, &ToolFrontmatter::WebFetch),
            "URL fetcher"
        );
        assert_eq!(
            ProviderAdapter::body_tool_name(&CursorAdapter, &ToolFrontmatter::Question),
            "Ask questions"
        );
        assert_eq!(
            ProviderAdapter::body_tool_name(&CursorAdapter, &ToolFrontmatter::Tasks),
            "TODO tracker"
        );
        assert_eq!(
            ProviderAdapter::body_tool_name(&CursorAdapter, &ToolFrontmatter::Subagent),
            "Task"
        );
        assert_eq!(
            ProviderAdapter::body_tool_name(&CursorAdapter, &ToolFrontmatter::Skill),
            "Skill runner"
        );
    }

    // -- Hook synthesis tests --

    fn make_hook_spec(id: &str, event: HookEvent, matcher: Option<&str>) -> HookSpec {
        HookSpec {
            path: std::path::PathBuf::from("/tmp/hooks.toml"),
            frontmatter: crate::spec::HookFrontmatter {
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
            CursorAdapter.event_name(HookEvent::UserPromptSubmit),
            "beforeSubmitPrompt"
        );
    }

    #[test]
    fn test_cursor_event_name_full_mapping() {
        assert_eq!(
            CursorAdapter.event_name(HookEvent::PreToolUse),
            "preToolUse"
        );
        assert_eq!(
            CursorAdapter.event_name(HookEvent::PostToolUse),
            "postToolUse"
        );
        assert_eq!(
            CursorAdapter.event_name(HookEvent::PostToolUseFailure),
            "postToolUseFailure"
        );
        assert_eq!(
            CursorAdapter.event_name(HookEvent::SessionStart),
            "sessionStart"
        );
        assert_eq!(
            CursorAdapter.event_name(HookEvent::SessionEnd),
            "sessionEnd"
        );
        assert_eq!(CursorAdapter.event_name(HookEvent::Stop), "stop");
        assert_eq!(
            CursorAdapter.event_name(HookEvent::PreCompact),
            "preCompact"
        );
        assert_eq!(
            CursorAdapter.event_name(HookEvent::SubagentStart),
            "subagentStart"
        );
        assert_eq!(
            CursorAdapter.event_name(HookEvent::SubagentStop),
            "subagentStop"
        );
    }

    #[test]
    fn test_synthesize_hooks_does_not_serialize_description() {
        // `HookFrontmatter::description` is documented as informational;
        // neither provider's host runtime consumes it. Lock that contract:
        // a description on the spec must not appear in the emitted JSON.
        let mut spec = make_hook_spec("init", HookEvent::SessionStart, None);
        spec.frontmatter.description = Some("informational note".to_string());
        let result = CursorAdapter
            .synthesize_hooks(&[&spec], None)
            .expect("expected value");
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
        let result = CursorAdapter
            .synthesize_hooks(&[&spec], None)
            .expect("expected value");
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
        let result = CursorAdapter
            .synthesize_hooks(&[&spec], None)
            .expect("expected value");
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
        // Merged modes hand JSON emission to the generic `HooksPatch`, which
        // dispatches through `HookAdapter::merge_into` to edit the host
        // `<config>/hooks.json` instead. Scripts still flow through to disk.
        let cfg = AdapterConfig {
            hook_emit_mode: Some(HookEmitMode::MergedUser),
            ..AdapterConfig::default()
        };
        let spec = make_hook_spec("init", HookEvent::SessionStart, None);
        let result = CursorAdapter
            .synthesize_hooks(&[&spec], Some(&cfg))
            .expect("expected ok");
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
        let spec = Spec::Rule(RuleSpec {
            path: "test.md".into(),
            frontmatter: RuleFrontmatter {
                id: "test-rule".to_string(),
                description: Some("A test rule".to_string()),
                tags: None,
            },
            body: "Rule body.".to_string(),
        });

        let files = CursorAdapter
            .adapt(spec, &HashMap::new(), Some(&cfg))
            .expect("expected value");
        assert_eq!(files[0].path.to_str(), Some("rules/tw-test-rule.mdc"));
    }

    #[test]
    fn test_file_kinds_includes_hooks() {
        assert!(CursorAdapter.file_kinds().contains(&FileKind::Hooks));
    }
}
