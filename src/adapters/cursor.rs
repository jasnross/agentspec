use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use jsonc_parser::cst::{CstInputValue, CstObject};
use serde::Serialize;

use super::hook_compile::{self, HookSynthesis};
use super::hooks_helpers::{
    has_agentspec_entries, is_owned_entry, open_or_create_array, open_or_create_object,
    prune_empty_event_arrays, value_to_cst_input,
};
use super::{
    Adapter, AdapterOutput, CompileCtx, RemovalOutput, RemoveCtx, SyncDestinationMode, TidyOutcome,
};
use crate::compile::{
    AdapterConfig, EmittedHookEntry, GeneratedFile, HookEmitMode,
    PluginManifest as SpecPluginManifest,
};
use crate::hooks_merge::{merge_owned, remove_owned};
use crate::plan::{FileKind, ForwardPatch, ReversePatch};
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
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CursorRuleFrontmatter {
    description: String,
    globs: Option<String>,
    always_apply: bool,
}

const HOST_FILENAME: &str = "hooks.json";
const HOOK_DOTDIR: &str = ".cursor";
/// Cursor's plugin-root env var. The host runtime sets `${CURSOR_PLUGIN_ROOT}`
/// in plugin scope; agentspec assigns it inline in merged-mode hook commands
/// so plugin-shaped scripts can reference sibling assets like
/// `${CURSOR_PLUGIN_ROOT}/rules` when synced project/user-wide. See
/// <https://cursor.com/docs/plugins>.
const PLUGIN_ROOT_ENV_VAR: &str = "CURSOR_PLUGIN_ROOT";
const PLUGIN_MANIFEST_DIR: &str = ".cursor-plugin";

/// Zero-sized adapter for the Cursor provider.
#[derive(Debug)]
pub struct CursorAdapter;

impl Adapter for CursorAdapter {
    fn compile(&self, specs: &[Spec], ctx: &CompileCtx<'_>) -> Result<AdapterOutput> {
        let mut files = Vec::new();
        for spec in specs {
            match spec {
                Spec::Agent(s) => files.extend(adapt_agent_spec(
                    s.clone(),
                    ctx.presets,
                    ctx.adapter_config,
                )?),
                Spec::Skill(s) => files.extend(adapt_skill_spec(s.clone(), ctx.adapter_config)?),
                Spec::Rule(s) => files.extend(adapt_rule_spec(s.clone(), ctx.adapter_config)?),
                // Hook scripts are emitted by `synthesize_hooks` once per provider —
                // see the matching note in `claude::ClaudeAdapter::compile`.
                Spec::Hook(_) => {}
            }
        }

        let hook_specs: Vec<&HookSpec> = specs
            .iter()
            .filter_map(|s| if let Spec::Hook(h) = s { Some(h) } else { None })
            .collect();
        let emit_mode = ctx.mode.to_hook_emit_mode();
        let HookSynthesis {
            entries: owned_entries,
            files: hook_files,
        } = synthesize_hooks(&hook_specs, emit_mode)?;
        files.extend(hook_files);

        // Cursor's plugin manifest is conditionally emitted: only when
        // `mode == Plugin` AND the binary supplied manifest fields. Cursor
        // installs cleanly with no manifest file at all, so omitting here is
        // safe when no plugin-* fields are configured.
        if ctx.mode == SyncDestinationMode::Plugin
            && let Some(manifest) = ctx.adapter_config.and_then(|c| c.plugin_manifest.as_ref())
        {
            files.push(build_plugin_manifest_file(manifest)?);
        }

        let dest_root = config_dir(ctx.mode, ctx.target_dir, ctx.home, ctx.cwd);

        let mut patches: Vec<Box<dyn ForwardPatch>> = Vec::new();
        if emit_mode.is_merged() {
            patches.push(Box::new(CursorHooksPatch {
                host_path: dest_root.join(HOST_FILENAME),
                owned_entries,
                force: ctx.overwrite,
            }));
        }

        Ok(AdapterOutput {
            files,
            patches,
            dest_root,
        })
    }

    fn removal_patches(&self, ctx: &RemoveCtx<'_>) -> RemovalOutput {
        let dest_root = config_dir(ctx.mode, ctx.target_dir, ctx.home, ctx.cwd);
        let emit_mode = ctx.mode.to_hook_emit_mode();
        let mut patches: Vec<Box<dyn ReversePatch>> = Vec::new();
        if emit_mode.is_merged() {
            patches.push(Box::new(CursorRemoveHooksPatch {
                host_path: dest_root.join(HOST_FILENAME),
            }));
        }
        RemovalOutput { patches, dest_root }
    }

    fn prune_patches(&self, home: &Path, cwd: &Path) -> Vec<Box<dyn ReversePatch>> {
        let candidates = [
            home.join(HOOK_DOTDIR).join(HOST_FILENAME),
            cwd.join(HOOK_DOTDIR).join(HOST_FILENAME),
        ];
        candidates
            .into_iter()
            .filter(|p| has_agentspec_entries(p))
            .map(|host_path| -> Box<dyn ReversePatch> {
                Box::new(CursorRemoveHooksPatch { host_path })
            })
            .collect()
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
            ToolFrontmatter::Shell => "Run shell commands",
            ToolFrontmatter::WebSearch => "Web",
            ToolFrontmatter::WebFetch => "URL fetcher", // descriptive: Cursor docs name no URL-fetch tool
            ToolFrontmatter::Question => "Ask questions",
            ToolFrontmatter::Tasks => "TODO tracker", // descriptive: Cursor docs name no TODO-list tool
            ToolFrontmatter::Subagent => "Task",
            ToolFrontmatter::Skill => "Skill runner", // descriptive: Cursor docs name no skill-invocation tool
        }
    }

    #[allow(clippy::match_same_arms)] // exhaustive to catch new ToolFrontmatter variants
    fn matcher_tool_name(&self, tool: &ToolFrontmatter) -> Option<&'static str> {
        match tool {
            ToolFrontmatter::Read => Some("Read"),
            ToolFrontmatter::Write => Some("Write"),
            ToolFrontmatter::Edit => Some("Edit"),
            ToolFrontmatter::Grep => Some("Grep"),
            ToolFrontmatter::Shell => Some("Shell"),
            ToolFrontmatter::WebSearch => Some("WebSearch"),
            ToolFrontmatter::Subagent => Some("Task"),
            ToolFrontmatter::Glob => None,
            ToolFrontmatter::WebFetch => None,
            ToolFrontmatter::Question => None,
            ToolFrontmatter::Tasks => None,
            ToolFrontmatter::Skill => None,
        }
    }

    fn matcher_subagent_type<'a>(&self, canonical: &'a str) -> &'a str {
        match canonical {
            "general" => "generalPurpose",
            "explore" => "explore",
            other => other,
        }
    }

    /// Returns the name which should be used to refer to the spec in the generated body content.
    ///
    /// For Cursor, all spec types use `{content_prefix}{id}` when a content prefix
    /// is configured (either explicitly or derived from `prefix`).
    fn body_spec_name(&self, spec: &Spec, cfg: Option<&AdapterConfig>) -> String {
        let id = spec.id();
        match cfg.and_then(AdapterConfig::content_prefix) {
            Some(prefix) => format!("{prefix}{id}"),
            None => id.to_owned(),
        }
    }

    fn body_skill_root(&self) -> Option<&'static str> {
        None
    }

    fn emits_hooks(&self) -> bool {
        true
    }

    fn plugin_manifest_dir(&self) -> Option<&'static str> {
        Some(PLUGIN_MANIFEST_DIR)
    }

    fn fully_implements_canonical_output(&self) -> bool {
        // `user_message` does not render in the Cursor UI — a denial shows a
        // generic message instead. That alone is why this is `false`.
        //
        // `agent_message` *does* reach the agent context, measured against
        // Cursor 3.16.17 by `experiments/cursor-gate-19-output-json`. An
        // earlier note here claimed otherwise; that claim was refuted.
        // Documented at `docs/hooks-canonical.md#cursor-known-limitations`.
        false
    }

    fn session_start_fires_on_resume(&self) -> bool {
        // Cursor's `sessionStart` fires only on initial conversation
        // creation, not on resume. Measured against Cursor 3.16.17 by
        // `experiments/cursor-session-start`; documented at
        // `docs/hooks-canonical.md#session-start-asymmetry`.
        false
    }
}

impl CursorAdapter {
    /// Translate a canonical `HookEvent` to Cursor's camelCase event name.
    ///
    /// Note that `user_prompt_submit` maps to `beforeSubmitPrompt` — not a simple
    /// casing transform.
    pub(crate) fn event_name(event: HookEvent) -> &'static str {
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
    pub(crate) fn entry_to_json(e: &EmittedHookEntry) -> serde_json::Value {
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

    /// Merge agentspec-owned entries into a parsed top-level `hooks.json`
    /// CST object. Sets `version: 1` if missing (without overwriting a
    /// user-authored value), opens the `hooks` object, and appends entries
    /// directly under their event arrays — Cursor's shape is one nesting
    /// level shallower than Claude's (no matcher-group wrapper).
    pub(crate) fn merge_into_hooks(
        top: &CstObject,
        owned_entries: &[EmittedHookEntry],
        force: bool,
    ) -> Result<()> {
        // Set `version: 1` if missing. Don't overwrite a user-authored value,
        // even if it's a different version — the user's intent wins.
        // Order matters: the version injection runs before the hooks-object
        // open. The shell's no-op-skip guard returns early only when both
        // `entries` is empty AND `top.get("hooks")` is `None`, so a user with
        // `hooks: { ... }` but no agentspec entries this run still gets
        // `version: 1` injected if absent.
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
                .entry(Self::event_name(e.event))
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
                event_arr.append(value_to_cst_input(Self::entry_to_json(e)));
            }
        }
        Ok(())
    }

    /// Strip agentspec-owned entries from a parsed `hooks.json` CST top
    /// object, prune emptied containers, and report whether the host file
    /// should be deleted.
    ///
    /// Cursor predicate: delete iff at least one `_agentspec_id`-tagged entry
    /// was removed AND the residual is either empty OR exactly one `version`
    /// key (any value). Cursor-exclusive — sync injects `version: 1` if
    /// absent and never overwrites a user value, so a residual
    /// `{version: <n>}` carries no information beyond file existence.
    pub(crate) fn tidy_hooks(top: &CstObject) -> TidyOutcome {
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

/// Cursor's `.cursor-plugin/plugin.json` shape.
///
/// Emits `name` (required), `version`, `description`, `author { name, email? }`,
/// `repository`, and `license`. Cursor's schema additionally supports
/// `displayName`, `category`, `tags`, `logo`, `publisher`, etc.; those are
/// out of scope.
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
struct CursorPluginManifestJson<'a> {
    name: &'a str,
    version: Option<&'a str>,
    description: Option<&'a str>,
    author: Option<PluginAuthorJson<'a>>,
    repository: Option<&'a str>,
    license: Option<&'a str>,
}

#[serde_with::skip_serializing_none]
#[derive(Serialize)]
struct PluginAuthorJson<'a> {
    name: &'a str,
    email: Option<&'a str>,
}

/// Build the `.cursor-plugin/plugin.json` `GeneratedFile`.
fn build_plugin_manifest_file(manifest: &SpecPluginManifest) -> Result<GeneratedFile> {
    let json = CursorPluginManifestJson {
        name: &manifest.name,
        version: manifest.version.as_deref(),
        description: manifest.description.as_deref(),
        author: manifest.author.as_ref().map(|a| PluginAuthorJson {
            name: &a.name,
            email: a.email.as_deref(),
        }),
        repository: manifest.repository.as_deref(),
        license: manifest.license.as_deref(),
    };
    let mut content = serde_json::to_vec_pretty(&json)
        .context("failed to serialize Cursor .cursor-plugin/plugin.json")?;
    content.push(b'\n');
    Ok(GeneratedFile::binary(
        Provider::Cursor,
        FileKind::PluginManifest,
        Path::new(PLUGIN_MANIFEST_DIR).join("plugin.json"),
        content,
        None,
    ))
}

/// Forwards to the shared `hook_compile::synthesize_hooks` with Cursor's
/// provider, dotdir, plugin-root env-var name, and JSON-builder bound.
/// Keeps the adapter-local call site stable while the shared synthesis
/// lives in one place.
fn synthesize_hooks(specs: &[&HookSpec], emit_mode: HookEmitMode) -> Result<HookSynthesis> {
    hook_compile::synthesize_hooks(
        Provider::Cursor,
        HOOK_DOTDIR,
        PLUGIN_ROOT_ENV_VAR,
        specs,
        emit_mode,
        build_cursor_hooks_json,
    )
}

fn config_dir(
    mode: SyncDestinationMode,
    target_dir: Option<&Path>,
    home: &Path,
    cwd: &Path,
) -> PathBuf {
    let dotdir = Path::new(HOOK_DOTDIR);
    super::resolve_config_dir(mode, target_dir, home, cwd, dotdir, dotdir)
}

/// Forward-direction hooks.json patch.
#[derive(Debug)]
pub(crate) struct CursorHooksPatch {
    host_path: PathBuf,
    owned_entries: Vec<EmittedHookEntry>,
    force: bool,
}

impl ForwardPatch for CursorHooksPatch {
    fn run(&self, dry_run: bool) -> Result<()> {
        let entries = &self.owned_entries;
        let force = self.force;
        merge_owned(
            &self.host_path,
            entries.is_empty(),
            |top| entries.is_empty() && top.get("hooks").is_none(),
            |top| CursorAdapter::merge_into_hooks(top, entries, force),
            dry_run,
        )
    }
}

/// Reverse-direction hooks.json patch.
#[derive(Debug)]
pub(crate) struct CursorRemoveHooksPatch {
    host_path: PathBuf,
}

impl ReversePatch for CursorRemoveHooksPatch {
    fn run_remove(&self, dry_run: bool) -> Result<()> {
        let report = remove_owned(&self.host_path, CursorAdapter::tidy_hooks, dry_run)?;
        report.print_summary(dry_run);
        Ok(())
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

    Ok(vec![GeneratedFile::text(
        Provider::Cursor,
        FileKind::Agents,
        path,
        content,
    )])
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
        FileKind::Skills,
        skill_dir.join("SKILL.md"),
        content,
    )];

    for (rel_path, sf) in spec.supporting_files {
        files.push(GeneratedFile::binary(
            Provider::Cursor,
            FileKind::Skills,
            skill_dir.join(&rel_path),
            sf.content,
            Some(sf.mode),
        ));
    }

    Ok(files)
}

fn adapt_rule_spec(spec: RuleSpec, cfg: Option<&AdapterConfig>) -> Result<Vec<GeneratedFile>> {
    let description = spec.frontmatter.description.unwrap_or_default();

    let (always_apply, globs) = if let Some(paths) = spec.frontmatter.paths {
        (false, Some(paths.join(", ")))
    } else {
        (true, None)
    };

    let frontmatter = CursorRuleFrontmatter {
        description,
        globs,
        always_apply,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}---\n\n{body}");

    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let path = Path::new("rules").join(format!("{file_prefix}{}.mdc", spec.frontmatter.id));

    Ok(vec![GeneratedFile::text(
        Provider::Cursor,
        FileKind::Rules,
        path,
        content,
    )])
}

// ── hooks.json synthesis ────────────────────────────────────────────────────

// Cursor's documented `hooks.json` shape (see <https://cursor.com/docs/hooks>):
//   { "version": 1, "hooks": { "<eventName>": [<entry>, <entry>, ...] } }
//
// Per-entry shape lives on `CursorAdapter::entry_to_json` (matcher per-entry,
// sentinel field). The CST-aware merge layer calls it via
// `CursorAdapter::merge_into_hooks` so the two emission paths stay in lockstep.

/// Cursor places the `matcher` on each entry directly; entries within an
/// event preserve insertion order from the spec list.
fn build_cursor_hooks_json(entries: &[EmittedHookEntry]) -> Result<String> {
    use serde_json::{Map, Value, json};

    let mut by_event: BTreeMap<&'static str, Vec<Value>> = BTreeMap::new();
    for entry in entries {
        by_event
            .entry(CursorAdapter::event_name(entry.event))
            .or_default()
            .push(CursorAdapter::entry_to_json(entry));
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

    use indexmap::IndexMap;

    use super::*;
    use crate::spec::{
        AgentFrontmatter, AgentSpec, RuleFrontmatter, RuleSpec, SkillFrontmatter, SkillSpec,
    };

    fn compile_one(spec: Spec, cfg: Option<&AdapterConfig>) -> Vec<GeneratedFile> {
        let presets = HashMap::new();
        let home = Path::new("/tmp/home");
        let cwd = Path::new("/tmp/cwd");
        let ctx = CompileCtx {
            mode: SyncDestinationMode::Compile,
            home,
            cwd,
            target_dir: None,
            presets: &presets,
            adapter_config: cfg,
            overwrite: false,
        };
        CursorAdapter.compile(&[spec], &ctx).expect("compile").files
    }

    fn make_hook_spec(id: &str, event: HookEvent, matcher: Option<&str>) -> HookSpec {
        HookSpec {
            path: std::path::PathBuf::from("/tmp/hooks.toml"),
            frontmatter: crate::spec::HookFrontmatter {
                id: id.to_string(),
                events: vec![event],
                script: format!("scripts/{id}.sh").into(),
                matcher: matcher.map(str::to_string),
                timeout: None,
                description: None,
                tags: None,
            },
            body: String::new(),
            supporting_files: IndexMap::new(),
        }
    }

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

        let files = compile_one(spec, None);
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

        let files = compile_one(spec, Some(&cfg));
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
            supporting_files: IndexMap::new(),
        });

        let files = compile_one(spec, Some(&cfg));
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
            CursorAdapter.body_tool_name(&ToolFrontmatter::Read),
            "Read files"
        );
        assert_eq!(
            CursorAdapter.body_tool_name(&ToolFrontmatter::Write),
            "Edit files"
        );
        assert_eq!(
            CursorAdapter.body_tool_name(&ToolFrontmatter::Edit),
            "Edit files"
        );
        assert_eq!(
            CursorAdapter.body_tool_name(&ToolFrontmatter::Grep),
            "Search files and folders"
        );
        assert_eq!(
            CursorAdapter.body_tool_name(&ToolFrontmatter::Glob),
            "Search files and folders"
        );
        assert_eq!(
            CursorAdapter.body_tool_name(&ToolFrontmatter::Shell),
            "Run shell commands"
        );
        assert_eq!(
            CursorAdapter.body_tool_name(&ToolFrontmatter::WebSearch),
            "Web"
        );
        assert_eq!(
            CursorAdapter.body_tool_name(&ToolFrontmatter::WebFetch),
            "URL fetcher"
        );
        assert_eq!(
            CursorAdapter.body_tool_name(&ToolFrontmatter::Question),
            "Ask questions"
        );
        assert_eq!(
            CursorAdapter.body_tool_name(&ToolFrontmatter::Tasks),
            "TODO tracker"
        );
        assert_eq!(
            CursorAdapter.body_tool_name(&ToolFrontmatter::Subagent),
            "Task"
        );
        assert_eq!(
            CursorAdapter.body_tool_name(&ToolFrontmatter::Skill),
            "Skill runner"
        );
    }

    #[test]
    fn test_matcher_tool_name_full_mapping() {
        assert_eq!(
            CursorAdapter.matcher_tool_name(&ToolFrontmatter::Read),
            Some("Read")
        );
        assert_eq!(
            CursorAdapter.matcher_tool_name(&ToolFrontmatter::Write),
            Some("Write")
        );
        assert_eq!(
            CursorAdapter.matcher_tool_name(&ToolFrontmatter::Edit),
            Some("Edit")
        );
        assert_eq!(
            CursorAdapter.matcher_tool_name(&ToolFrontmatter::Grep),
            Some("Grep")
        );
        assert_eq!(
            CursorAdapter.matcher_tool_name(&ToolFrontmatter::Shell),
            Some("Shell")
        );
        assert_eq!(
            CursorAdapter.matcher_tool_name(&ToolFrontmatter::WebSearch),
            Some("WebSearch")
        );
        assert_eq!(
            CursorAdapter.matcher_tool_name(&ToolFrontmatter::Subagent),
            Some("Task")
        );
        assert_eq!(
            CursorAdapter.matcher_tool_name(&ToolFrontmatter::Glob),
            None
        );
        assert_eq!(
            CursorAdapter.matcher_tool_name(&ToolFrontmatter::WebFetch),
            None
        );
        assert_eq!(
            CursorAdapter.matcher_tool_name(&ToolFrontmatter::Question),
            None
        );
        assert_eq!(
            CursorAdapter.matcher_tool_name(&ToolFrontmatter::Tasks),
            None
        );
        assert_eq!(
            CursorAdapter.matcher_tool_name(&ToolFrontmatter::Skill),
            None
        );
    }

    #[test]
    fn test_matcher_subagent_type_general() {
        assert_eq!(
            CursorAdapter.matcher_subagent_type("general"),
            "generalPurpose"
        );
    }

    #[test]
    fn test_matcher_subagent_type_explore() {
        assert_eq!(CursorAdapter.matcher_subagent_type("explore"), "explore");
    }

    #[test]
    fn test_matcher_subagent_type_plan_passes_through() {
        assert_eq!(CursorAdapter.matcher_subagent_type("plan"), "plan");
    }

    #[test]
    fn test_cursor_event_name_user_prompt_submit_special_case() {
        // The one mapping that isn't a simple casing transform.
        assert_eq!(
            CursorAdapter::event_name(HookEvent::UserPromptSubmit),
            "beforeSubmitPrompt"
        );
    }

    #[test]
    fn test_cursor_event_name_full_mapping() {
        assert_eq!(
            CursorAdapter::event_name(HookEvent::PreToolUse),
            "preToolUse"
        );
        assert_eq!(
            CursorAdapter::event_name(HookEvent::PostToolUse),
            "postToolUse"
        );
        assert_eq!(
            CursorAdapter::event_name(HookEvent::PostToolUseFailure),
            "postToolUseFailure"
        );
        assert_eq!(
            CursorAdapter::event_name(HookEvent::SessionStart),
            "sessionStart"
        );
        assert_eq!(
            CursorAdapter::event_name(HookEvent::SessionEnd),
            "sessionEnd"
        );
        assert_eq!(CursorAdapter::event_name(HookEvent::Stop), "stop");
        assert_eq!(
            CursorAdapter::event_name(HookEvent::PreCompact),
            "preCompact"
        );
        assert_eq!(
            CursorAdapter::event_name(HookEvent::SubagentStart),
            "subagentStart"
        );
        assert_eq!(
            CursorAdapter::event_name(HookEvent::SubagentStop),
            "subagentStop"
        );
    }

    #[test]
    fn test_synthesize_hooks_does_not_serialize_description() {
        let mut spec = make_hook_spec("init", HookEvent::SessionStart, None);
        spec.frontmatter.description = Some("informational note".to_string());
        let result = synthesize_hooks(&[&spec], HookEmitMode::Bundled).expect("expected value");
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
        let result = synthesize_hooks(&[&spec], HookEmitMode::Bundled).expect("expected value");
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
        let result = synthesize_hooks(&[&spec], HookEmitMode::Bundled).expect("expected value");
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
            content.contains("\"matcher\": \"Bash\""),
            "expected per-entry matcher, got: {content}"
        );
    }

    #[test]
    fn test_synthesize_hooks_merged_user_emits_scripts_no_hooks_json() {
        let spec = make_hook_spec("init", HookEvent::SessionStart, None);
        let result = synthesize_hooks(&[&spec], HookEmitMode::MergedUser).expect("expected ok");
        assert_eq!(result.entries.len(), 1);
        assert!(
            !result
                .files
                .iter()
                .any(|f| f.path.to_str() == Some("hooks/hooks.json")),
            "Merged mode must NOT emit hooks/hooks.json"
        );
        assert_eq!(
            result.entries[0].command,
            "CURSOR_PLUGIN_ROOT=$HOME/.cursor $HOME/.cursor/hooks/scripts/_wrappers/session_start.sh $HOME/.cursor/hooks/scripts/init.sh init"
        );
    }

    #[test]
    fn test_adapt_rule_with_prefix() {
        let cfg = AdapterConfig {
            prefix: Some("tw".to_string()),
            ..AdapterConfig::default()
        };
        let spec = Spec::Rule(RuleSpec {
            path: "test.md".into(),
            frontmatter: RuleFrontmatter {
                id: "test-rule".to_string(),
                description: Some("A test rule".to_string()),
                tags: None,
                paths: None,
            },
            body: "Rule body.".to_string(),
        });

        let files = compile_one(spec, Some(&cfg));
        assert_eq!(files[0].path.to_str(), Some("rules/tw-test-rule.mdc"));
    }

    #[test]
    fn test_build_plugin_manifest_file_emits_all_fields() {
        use crate::compile::{PluginAuthor, PluginManifest};

        let manifest = PluginManifest {
            name: "tw".to_string(),
            version: Some("0.1.0".to_string()),
            description: Some("Thoughts workflow plugin".to_string()),
            author: Some(PluginAuthor {
                name: "Jason".to_string(),
                email: Some("jason@example.com".to_string()),
            }),
            repository: Some("https://github.com/jasnross/tw".to_string()),
            license: Some("MIT".to_string()),
        };
        let file = build_plugin_manifest_file(&manifest).expect("manifest builds");
        assert_eq!(file.provider, Provider::Cursor);
        assert_eq!(file.kind, FileKind::PluginManifest);
        assert_eq!(file.path.to_str(), Some(".cursor-plugin/plugin.json"));

        let content = String::from_utf8(file.content.clone()).expect("utf-8");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("valid json");
        assert_eq!(parsed["name"], "tw");
        assert_eq!(parsed["version"], "0.1.0");
        assert_eq!(parsed["description"], "Thoughts workflow plugin");
        assert_eq!(parsed["author"]["name"], "Jason");
        assert_eq!(parsed["author"]["email"], "jason@example.com");
        assert_eq!(parsed["repository"], "https://github.com/jasnross/tw");
        assert_eq!(parsed["license"], "MIT");
    }

    #[test]
    fn test_compile_emits_cursor_manifest_in_plugin_mode_with_config() {
        use crate::compile::PluginManifest;

        let presets = HashMap::new();
        let cfg = AdapterConfig {
            plugin_manifest: Some(PluginManifest {
                name: "tw".to_string(),
                version: None,
                description: None,
                author: None,
                repository: None,
                license: None,
            }),
            ..AdapterConfig::default()
        };
        let ctx = CompileCtx {
            mode: SyncDestinationMode::Plugin,
            home: Path::new("/tmp/home"),
            cwd: Path::new("/tmp/cwd"),
            target_dir: Some(Path::new("/out")),
            presets: &presets,
            adapter_config: Some(&cfg),
            overwrite: false,
        };
        let output = CursorAdapter.compile(&[], &ctx).expect("compile");
        assert!(
            output
                .files
                .iter()
                .any(|f| f.kind == FileKind::PluginManifest
                    && f.path.to_str() == Some(".cursor-plugin/plugin.json")),
            "expected `.cursor-plugin/plugin.json` in plugin mode"
        );
    }

    #[test]
    fn test_compile_skips_cursor_manifest_in_plugin_mode_without_config() {
        // Per the plan: Cursor's manifest is conditionally emitted. When
        // `mode == Plugin` but no plugin-* fields are configured, the rest
        // of the tree still emits but `.cursor-plugin/plugin.json` is omitted.
        let presets = HashMap::new();
        let ctx = CompileCtx {
            mode: SyncDestinationMode::Plugin,
            home: Path::new("/tmp/home"),
            cwd: Path::new("/tmp/cwd"),
            target_dir: Some(Path::new("/out")),
            presets: &presets,
            adapter_config: None,
            overwrite: false,
        };
        let output = CursorAdapter.compile(&[], &ctx).expect("compile");
        assert!(
            output
                .files
                .iter()
                .all(|f| f.kind != FileKind::PluginManifest),
            "Cursor must NOT emit a manifest file when no plugin-* fields are configured"
        );
    }

    #[test]
    fn test_entry_to_cursor_json_places_matcher_on_entry() {
        let e = EmittedHookEntry {
            event: HookEvent::PreToolUse,
            matcher: Some("Bash".to_string()),
            command: "/path/to/script.sh".to_string(),
            timeout: None,
            agentspec_id: "audit".to_string(),
        };
        let v = CursorAdapter::entry_to_json(&e);
        assert_eq!(v["matcher"], "Bash");
        assert_eq!(v["_agentspec_id"], "audit");
    }

    #[test]
    fn test_user_dest_dir_is_dot_cursor_under_home() {
        let presets = HashMap::new();
        let home = Path::new("/home/user");
        let cwd = Path::new("/work");
        let ctx = CompileCtx {
            mode: SyncDestinationMode::User,
            home,
            cwd,
            target_dir: None,
            presets: &presets,
            adapter_config: None,
            overwrite: false,
        };
        let output = CursorAdapter.compile(&[], &ctx).expect("compile");
        assert_eq!(output.dest_root, PathBuf::from("/home/user/.cursor"));
    }

    #[test]
    fn test_project_dest_dir_is_dot_cursor_under_cwd() {
        let presets = HashMap::new();
        let home = Path::new("/home/user");
        let cwd = Path::new("/work/project");
        let ctx = CompileCtx {
            mode: SyncDestinationMode::Project,
            home,
            cwd,
            target_dir: None,
            presets: &presets,
            adapter_config: None,
            overwrite: false,
        };
        let output = CursorAdapter.compile(&[], &ctx).expect("compile");
        assert_eq!(output.dest_root, PathBuf::from("/work/project/.cursor"));
    }

    #[test]
    fn test_adapt_rule_without_paths() {
        let spec = Spec::Rule(RuleSpec {
            path: "test.md".into(),
            frontmatter: RuleFrontmatter {
                id: "my-rule".to_string(),
                description: Some("A rule".to_string()),
                tags: None,
                paths: None,
            },
            body: "Rule body.".to_string(),
        });

        let files = compile_one(spec, None);
        assert_eq!(files.len(), 1);
        let content = String::from_utf8(files[0].content.clone()).expect("utf8");
        assert!(
            content.contains("alwaysApply: true"),
            "rule without paths should have alwaysApply: true, got: {content}"
        );
        assert!(
            !content.contains("globs:"),
            "rule without paths should have no globs field, got: {content}"
        );
    }

    #[test]
    fn test_adapt_rule_with_paths() {
        let spec = Spec::Rule(RuleSpec {
            path: "test.md".into(),
            frontmatter: RuleFrontmatter {
                id: "react-rule".to_string(),
                description: Some("React conventions".to_string()),
                tags: None,
                paths: Some(vec![
                    "src/components/**/*.tsx".to_string(),
                    "src/hooks/**/*.ts".to_string(),
                ]),
            },
            body: "Rule body.".to_string(),
        });

        let files = compile_one(spec, None);
        assert_eq!(files.len(), 1);
        let content = String::from_utf8(files[0].content.clone()).expect("utf8");
        assert!(
            content.contains("alwaysApply: false"),
            "rule with paths should have alwaysApply: false, got: {content}"
        );
        assert!(
            content.contains("globs:"),
            "rule with paths should have globs field, got: {content}"
        );
        assert!(
            content.contains("src/components/**/*.tsx, src/hooks/**/*.ts"),
            "globs should be comma-separated, got: {content}"
        );
    }
}
