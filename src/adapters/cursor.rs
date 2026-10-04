use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use jsonc_parser::cst::{CstInputValue, CstObject};
use serde::Serialize;

use super::hook_compile::{self, HookSynthesis};
use super::hooks_helpers::{
    has_agentspec_entries, is_owned_entry, open_or_create_array, open_or_create_object,
    prune_empty_event_arrays, value_to_cst_input,
};
use super::{
    Adapter, AdapterOutput, CompileCtx, Degradation, DegradationKind, Delivery, RemovalOutput,
    RemoveCtx, SyncDestinationMode, TidyOutcome,
};
use crate::compile::{
    AdapterConfig, EmittedHookEntry, GeneratedFile, HookEmitMode,
    PluginManifest as SpecPluginManifest,
};
use crate::declarations::Declarations;
use crate::hooks_merge::{merge_owned, remove_owned};
use crate::mcp::{CursorMcpServer, McpServer, is_mcp_name};
use crate::plan::{FileKind, ForwardPatch, ReversePatch};
use crate::presets::{CursorPreset, ProviderPresets, ProviderPresetsMap};
use crate::provider::Provider;
use crate::setting::{Carries, SettingKey, SettingKind};
use crate::spec::{AgentSpec, HookEvent, HookSpec, RuleSpec, SkillSpec, Spec, ToolFrontmatter};

// See: https://cursor.com/docs/subagents#configuration-fields
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
struct CursorAgentFrontmatter {
    name: String,
    description: String,
    model: Option<String>,
    /// The record the composed `model` value cannot supply.
    ///
    /// Cursor spells every model option as a bracket suffix on the model id,
    /// so by the time `model` is `Some` the individual settings have been
    /// concatenated into one string that reading back would require a second
    /// parser. The same expression that builds the bracket collects these
    /// keys, which is what keeps the record from drifting from the output.
    #[serde(skip)]
    carried: Vec<SettingKey>,
}

impl Carries for CursorAgentFrontmatter {
    fn carried(&self) -> Vec<SettingKey> {
        self.carried.clone()
    }
}

// See: https://cursor.com/docs/skills#frontmatter-fields
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CursorSkillFrontmatter {
    name: String,
    description: String,
    disable_model_invocation: bool,
}

impl Carries for CursorSkillFrontmatter {
    /// Nothing. Cursor's skill schema has no model field, so no value from a
    /// preset's Cursor block reaches a generated Cursor skill file — the drop
    /// `README.md` documents under "Execution presets reach skill files on
    /// Claude only".
    fn carried(&self) -> Vec<SettingKey> {
        Vec::new()
    }
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

impl Carries for CursorRuleFrontmatter {
    fn carried(&self) -> Vec<SettingKey> {
        self.globs
            .is_some()
            .then_some(SettingKey::Paths)
            .into_iter()
            .collect()
    }
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
        let mut deliveries = Vec::new();
        for spec in specs {
            match spec {
                Spec::Agent(s) => {
                    let (f, d) = adapt_agent_spec(s.clone(), ctx.presets, ctx.adapter_config)?;
                    files.extend(f);
                    deliveries.extend(d);
                }
                Spec::Skill(s) => {
                    let (f, d) = adapt_skill_spec(s.clone(), ctx.adapter_config)?;
                    files.extend(f);
                    deliveries.extend(d);
                }
                Spec::Rule(s) => {
                    let (f, d) = adapt_rule_spec(s.clone(), ctx.adapter_config)?;
                    files.extend(f);
                    deliveries.extend(d);
                }
                // Hook scripts are emitted by `synthesize_hooks` once per provider —
                // see the matching note in `claude::ClaudeAdapter::compile`.
                Spec::Hook(_) => {}
            }
        }

        let hook_specs: Vec<&HookSpec> = specs
            .iter()
            .filter_map(|s| if let Spec::Hook(h) = s { Some(h) } else { None })
            .collect();

        // Pushed here rather than from a post-loop orchestrator scan: this is
        // the walk that already knows whether any hook spec exists, and the
        // accessor below is Cursor's own claim about its runtime.
        let mut degradations = Vec::new();
        if !hook_specs.is_empty() && !self.fully_implements_canonical_output() {
            degradations.push(Degradation::provider_wide(
                Provider::Cursor,
                DegradationKind::PartialOutputImpl,
            ));
        }

        let emit_mode = ctx.mode.to_hook_emit_mode();
        let HookSynthesis {
            entries: owned_entries,
            files: hook_files,
            deliveries: hook_deliveries,
        } = synthesize_hooks(&hook_specs, emit_mode)?;
        files.extend(hook_files);
        deliveries.extend(hook_deliveries);

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
            degradations,
            deliveries,
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

    /// `<server>:<tool>`, where `<server>` is `[mcp.<name>.cursor] server` or the
    /// logical name. Descriptive: Cursor docs name no model-facing id for an MCP
    /// tool, and `server:tool` is the only form they use to name one tool by
    /// server and tool (its permission rules). Unlike a description, it reads
    /// correctly inside backticks, where most tool references sit.
    fn body_mcp_tool_name(&self, server: &McpServer, logical: &str, tool: &str) -> String {
        let McpServer {
            claude: _,
            cursor,
            opencode: _,
        } = server;
        let resolved = cursor
            .as_ref()
            .and_then(|c| c.server.as_deref())
            .unwrap_or(logical);
        format!("{resolved}:{tool}")
    }

    fn body_skill_root(&self) -> Option<&'static str> {
        None
    }

    fn carriable(&self, kind: FileKind) -> &'static [SettingKind] {
        match kind {
            // `Tools` is absent because Cursor exposes no per-agent tool
            // restriction: its documented subagent fields are `name`,
            // `description`, `model`, `readonly`, and `is_background`, and
            // subagents inherit every tool from the parent. Custom Modes,
            // which could restrict tools per mode, were removed in Cursor
            // 2.1, and every remaining control gates approval rather than
            // availability. If Cursor adds one, this arm gains `Tools` and
            // `test_carriable_agrees_with_carried` fails until the adapter
            // threads the value.
            FileKind::Agents => &[
                SettingKind::Body,
                SettingKind::Model,
                SettingKind::Effort,
                SettingKind::Fast,
                SettingKind::Context,
                SettingKind::Param,
            ],
            FileKind::Rules => &[SettingKind::Body, SettingKind::Paths],
            // Distinct facts that happen to coincide: a Cursor skill file's
            // schema has no field a preset can reach, and a hook registration
            // carries only the hook itself.
            FileKind::Skills | FileKind::Hooks => &[SettingKind::Body],
            FileKind::Commands | FileKind::PluginManifest => &[],
        }
    }

    /// Cursor's bracket grammar for each preset's `cursor` block, then each
    /// MCP server's `cursor.server` override.
    ///
    /// Presets are keyed by a `HashMap`, so iteration order is
    /// nondeterministic and a multi-error run would report differently each
    /// time. Sort by preset name first. At most one error per preset —
    /// `validate_cursor_preset` reports its first failing check.
    fn validate_declarations(&self, declarations: &Declarations) -> Vec<String> {
        let Declarations { presets, mcp } = declarations;
        let mut names: Vec<&String> = presets.keys().collect();
        names.sort();
        let mut errors: Vec<String> = names
            .into_iter()
            .filter_map(|name| {
                let ProviderPresets {
                    claude: _,
                    cursor,
                    opencode: _,
                } = presets.get(name)?;
                validate_cursor_preset(cursor.as_ref()?, name)
                    .err()
                    .map(|e| e.to_string())
            })
            .collect();

        // Only explicit overrides: `validate.rs` checks every logical name, so
        // checking the resolved name here would report one bad name twice.
        for (name, server) in mcp {
            let McpServer {
                claude: _,
                cursor,
                opencode: _,
            } = server;
            if let Some(CursorMcpServer {
                server: Some(override_name),
            }) = cursor
                && !is_mcp_name(override_name)
            {
                errors.push(format!(
                    "[mcp.{name}.cursor] `server` must match [A-Za-z0-9_-]+ \
                     (got {override_name:?})"
                ));
            }
        }
        errors
    }

    fn plugin_manifest_dir(&self) -> Option<&'static str> {
        Some(PLUGIN_MANIFEST_DIR)
    }

    fn hook_command_preview(
        &self,
        event: HookEvent,
        script: &Path,
        hook_id: &str,
        args: &[String],
    ) -> String {
        let filename = hook_compile::script_filename(script);
        hook_compile::hook_command_anchor(
            HOOK_DOTDIR,
            PLUGIN_ROOT_ENV_VAR,
            HookEmitMode::Bundled,
            event,
            &filename,
            hook_id,
            args,
        )
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

/// The characters Cursor's `model[k=v,k=v]` grammar uses as delimiters.
///
/// None may appear in a `model` id or in an option value, because agentspec
/// composes the bracket by concatenation and Cursor documents no escaping
/// syntax to compose against. Without this, a single field forges a second
/// option — `effort = "high,context=1m"` emits `model[effort=high,context=1m]`
/// — which is exactly the "two spellings of one option cannot coexist"
/// guarantee the bracket ban exists to provide.
const CURSOR_BRACKET_DELIMITERS: [char; 4] = ['[', ']', ',', '='];

/// Option ids that have a named `CursorPreset` field. A `params` key matching
/// one of these — in any case — is rejected rather than merged, so an option can
/// only be spelled one way.
///
/// Hand-maintained, unlike the destructuring bindings that guard the validation
/// and composition sites. `test_named_cursor_options_are_real_fields` catches a
/// rename or removal by round-tripping each entry through `deny_unknown_fields`;
/// a field *added* without being listed here is not caught, and would let
/// `params` re-spell it into a duplicate option. See that test for why.
const NAMED_CURSOR_OPTIONS: [&str; 3] = ["effort", "fast", "context"];

/// Cross-field checks on one preset's `cursor` block, reached via
/// `validate_declarations` from `Specs::validate`, so every command that loads
/// specs surfaces them. `ValidatedSpecs` carries the declarations it was
/// validated against and `compile::run` reads presets from there, so on that
/// path the map reaching `adapt_agent_spec` is one that passed here.
///
/// A consumer calling `Provider::adapter().compile(...)` directly supplies
/// its own presets and bypasses this; the `debug_assert!`s in
/// `adapt_agent_spec` are all that stand there, and they compile out in
/// release.
fn validate_cursor_preset(preset: &CursorPreset, preset_name: &str) -> Result<()> {
    if let Some(model) = preset.model.as_deref() {
        if model.contains(CURSOR_BRACKET_DELIMITERS) {
            bail!(
                "[presets.{preset_name}.cursor] `model` must be a bare model id \
                 with no `[`, `]`, `,`, or `=`; set the `effort`, `fast`, or \
                 `context` field instead of Cursor's `[k=v]` syntax"
            );
        }
        check_composable("model", model, preset_name)?;
    }
    for (field, value) in [
        ("effort", preset.effort.as_deref()),
        ("context", preset.context.as_deref()),
    ] {
        let Some(value) = value else { continue };
        check_bracket_safe(field, value, preset_name)?;
    }

    // `params` keys must not collide with each other either, on the same
    // reasoning as the named-field check below: `optimize_for` beside
    // `Optimize_For` is one option spelled twice whichever way Cursor folds
    // ids. `BTreeMap` orders by byte, so case variants are not adjacent —
    // this needs a set, not a neighbour compare.
    let mut folded: HashMap<String, &String> = HashMap::new();
    for key in preset.params.keys() {
        if let Some(first) = folded.insert(key.to_ascii_lowercase(), key) {
            bail!(
                "[presets.{preset_name}.cursor] `params` keys `{first}` and \
                 `{key}` differ only in case; Cursor's option ids are not \
                 known to be case-folded, so one of them would be a silently \
                 duplicated option"
            );
        }
    }

    for (key, value) in &preset.params {
        // Case-insensitive: whether Cursor folds option-id case is
        // unmeasured, and both readings are bad. If it folds, an untyped
        // `params` entry silently overrides the typed field; if it does not,
        // the user gets an option they believe is set and Cursor ignores.
        // Either way `[effort=high,Effort=low]` is one option spelled twice.
        if let Some(named) = NAMED_CURSOR_OPTIONS
            .iter()
            .find(|n| n.eq_ignore_ascii_case(key))
        {
            bail!(
                "[presets.{preset_name}.cursor] `params.{key}` duplicates the \
                 `{named}` field; set one or the other, not both — two \
                 spellings of one option cannot coexist"
            );
        }
        // Labelled separately so a malformed key is distinguishable from a
        // malformed value — an empty key would otherwise report as
        // `params.`, naming nothing.
        check_bracket_safe(&format!("params key {key:?}"), key, preset_name)?;
        check_bracket_safe(&format!("params.{key}"), value, preset_name)?;
    }
    if preset.model.is_none() && any_option_set(preset) {
        bail!(
            "[presets.{preset_name}.cursor] model options require `model` \
             (Cursor encodes them as bracket options: `model[effort=high]`)"
        );
    }
    Ok(())
}

/// True when any bracket option is configured.
///
/// The destructuring binding is load-bearing: adding a fourth option to
/// `CursorPreset` fails to compile here until it is accounted for. A plain
/// `preset.effort.is_some() || …` chain would compile against the new field
/// and silently under-report, which is the failure this function exists to
/// prevent — an option set with no `model` would then pass validation and
/// be dropped at composition time with nothing said.
fn any_option_set(preset: &CursorPreset) -> bool {
    let CursorPreset {
        model: _,
        effort,
        fast,
        context,
        params,
    } = preset;
    effort.is_some() || fast.is_some() || context.is_some() || !params.is_empty()
}

/// `check_composable` plus the delimiter ban — the full set of rules a bracket
/// option id or value must satisfy.
fn check_bracket_safe(field: &str, value: &str, preset_name: &str) -> Result<()> {
    if value.contains(CURSOR_BRACKET_DELIMITERS) {
        bail!(
            "[presets.{preset_name}.cursor] `{field}` must not contain \
             `[`, `]`, `,`, or `=` — agentspec composes Cursor's bracket \
             syntax from these fields, and a delimiter here would forge \
             an option the preset did not declare"
        );
    }
    check_composable(field, value, preset_name)
}

/// Reject a `model` id or option value that cannot survive bracket composition.
///
/// Empty and whitespace-bearing values both compose something malformed, and
/// Cursor rejects nothing — so the result is silently discarded, plausibly
/// taking the well-formed options beside it down with the whole bracket:
///
/// - `model = ""` composes `[effort=high]`, a bracket with no model in front.
///   An empty `model` is still `Some`, so it satisfies the model-less-options
///   check without this.
/// - `effort = ""` composes `model[effort=]`.
/// - `model = " claude-opus-5 "` composes ` claude-opus-5 [effort=high]`, which
///   serde then emits as a *quoted* scalar — changing the model id itself.
///
/// No Cursor model id or documented option value contains whitespace, so
/// rejecting it outright costs nothing and needs no trimming rule to explain.
fn check_composable(field: &str, value: &str, preset_name: &str) -> Result<()> {
    if value.is_empty() {
        bail!(
            "[presets.{preset_name}.cursor] `{field}` must not be empty; \
             omit the key entirely to leave it unset"
        );
    }
    if value.contains(char::is_whitespace) {
        bail!(
            "[presets.{preset_name}.cursor] `{field}` must not contain whitespace \
             (got {value:?}) — agentspec composes Cursor's bracket syntax by \
             concatenation, and Cursor silently discards a malformed bracket"
        );
    }
    Ok(())
}

fn adapt_agent_spec(
    spec: AgentSpec,
    presets: &ProviderPresetsMap,
    cfg: Option<&AdapterConfig>,
) -> Result<(Vec<GeneratedFile>, Vec<Delivery>)> {
    let id = spec.frontmatter.id;
    let description = spec.frontmatter.description;

    let cursor_preset = spec
        .frontmatter
        .execution
        .and_then(|x| x.preset)
        .and_then(|x| presets.get(&x))
        .and_then(|x| x.cursor.clone());

    // Cursor encodes model options as a `model[k=v,k=v]` suffix on the model id.
    // Building that suffix is string *construction* of a provider-defined
    // identifier format — not string *manipulation of serialized output* — so it
    // does not violate the operate-on-structs rule. The typed fields are the
    // source of truth right up to this point; nothing parses the result back.
    //
    // The emission order is fixed by this array literal, and agentspec owns it.
    // Authors cannot produce a different order, so one deterministic order is
    // enough — and a stable order is what keeps the byte-level tests meaningful.
    //
    // The destructuring binding mirrors `any_option_set`: a fourth
    // option added to the struct fails to compile here rather than being
    // silently dropped from the bracket after passing validation as "set".
    //
    // Each element pairs the `SettingKey` the option came from with the
    // composed `k=v` fragment, so the bracket joins the strings and the
    // delivery record collects the keys from one expression. The composed
    // `model` value cannot be read back without a second parser, so a
    // separately-written record is exactly what would drift.
    let opts: Vec<(SettingKey, String)> = cursor_preset
        .as_ref()
        .map(
            |CursorPreset {
                 model: _,
                 effort,
                 fast,
                 context,
                 params,
             }| {
                [
                    effort
                        .as_ref()
                        .map(|v| (SettingKey::Effort, format!("effort={v}"))),
                    fast.map(|v| (SettingKey::Fast, format!("fast={v}"))),
                    context
                        .as_ref()
                        .map(|v| (SettingKey::Context, format!("context={v}"))),
                ]
                .into_iter()
                .flatten()
                // Named options first in their fixed order, then `params` —
                // a `BTreeMap`, so its own order is already deterministic and
                // needs no sort here. A `params` key cannot collide with a
                // named option; `validate_cursor_preset` rejects that.
                .chain(
                    params
                        .iter()
                        .map(|(k, v)| (SettingKey::Param(k.clone()), format!("{k}={v}"))),
                )
                .collect()
            },
        )
        .unwrap_or_default();

    let base = cursor_preset.and_then(|p| p.model);

    // Defense-in-depth only: `validate_cursor_preset` is the user-facing gate,
    // reached from `Specs::validate` before any adapter runs. These mirror all
    // four of its rules rather than a subset, so a direct `Adapter::compile`
    // call — the one route the gate cannot cover — trips on any of them in a
    // debug build. They still compile out in release.
    debug_assert!(
        base.as_deref()
            .is_none_or(|m| !m.contains(CURSOR_BRACKET_DELIMITERS)),
        "delimiter-bearing Cursor model should have been rejected by validate_cursor_preset"
    );
    debug_assert!(
        base.as_deref()
            .is_none_or(|m| !m.is_empty() && !m.contains(char::is_whitespace)),
        "empty or whitespace-bearing Cursor model should have been rejected by validate_cursor_preset"
    );
    debug_assert!(
        base.is_some() || opts.is_empty(),
        "model-less Cursor options should have been rejected by validate_cursor_preset"
    );
    // Checked on the composed `k=v` fragments rather than the fields, so one
    // assertion covers every option without naming them — and so a fourth
    // option is covered the moment the array literal above emits it.
    debug_assert!(
        opts.iter().map(|(_, s)| s).all(|opt| {
            // Exactly one `=`, from the `format!` above; a delimiter in the
            // value would add another, and a forged option would add a comma.
            opt.match_indices('=').count() == 1
                && !opt.contains([',', '[', ']'])
                && !opt.contains(char::is_whitespace)
                // Non-empty on both sides of the `=`. An empty `params` key
                // composes `=cost`, which satisfies every other condition here.
                && !opt.ends_with('=')
                && !opt.starts_with('=')
        }),
        "malformed Cursor bracket option should have been rejected by validate_cursor_preset: {opts:?}"
    );

    // `Model` is recorded from `base`, not from the composed `model` value,
    // because reading `base` names the setting the author actually set.
    //
    // The one shape where this record and the emitted value could disagree is
    // options with no `model`: the composition below drops the whole bracket
    // while these keys still record as delivered, hiding a real loss.
    // `validate_cursor_preset` rejects that preset outright and the
    // `base.is_some() || opts.is_empty()` assertion above re-checks it, so the
    // shape does not reach here — but it is the one to preserve those gates
    // for.
    let carried: Vec<SettingKey> = base
        .iter()
        .map(|_| SettingKey::Model)
        .chain(opts.iter().map(|(key, _)| key.clone()))
        .collect();

    let model = match (base, opts.is_empty()) {
        (Some(m), false) => {
            let joined: Vec<&str> = opts.iter().map(|(_, s)| s.as_str()).collect();
            Some(format!("{m}[{}]", joined.join(",")))
        }
        (m, _) => m,
    };

    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let path = Path::new("agents").join(format!("{file_prefix}{id}.md"));

    // Cursor agents get frontmatter name prefix with "-" delimiter
    let name = match cfg.and_then(|c| c.prefix.as_deref()) {
        Some(prefix) => format!("{prefix}-{id}"),
        None => id.clone(),
    };

    let frontmatter = CursorAgentFrontmatter {
        name,
        description,
        model,
        carried,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}---\n\n{body}");

    let file =
        GeneratedFile::text(Provider::Cursor, FileKind::Agents, path, content).with_spec_id(&id);
    let deliveries = Delivery::from_file(&id, &file, frontmatter.carried());
    Ok((vec![file], deliveries))
}

fn adapt_skill_spec(
    spec: SkillSpec,
    cfg: Option<&AdapterConfig>,
) -> Result<(Vec<GeneratedFile>, Vec<Delivery>)> {
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

    let skill_file = GeneratedFile::text(
        Provider::Cursor,
        FileKind::Skills,
        skill_dir.join("SKILL.md"),
        content,
    )
    .with_spec_id(&id);
    let deliveries = Delivery::from_file(&id, &skill_file, frontmatter.carried());
    let mut files = vec![skill_file];

    // Supporting files carry no settings, but still name their spec: `Body`
    // membership is read off `GeneratedFile.spec_id`.
    for (rel_path, sf) in spec.supporting_files {
        files.push(
            GeneratedFile::binary(
                Provider::Cursor,
                FileKind::Skills,
                skill_dir.join(&rel_path),
                sf.content,
                Some(sf.mode),
            )
            .with_spec_id(&id),
        );
    }

    Ok((files, deliveries))
}

fn adapt_rule_spec(
    spec: RuleSpec,
    cfg: Option<&AdapterConfig>,
) -> Result<(Vec<GeneratedFile>, Vec<Delivery>)> {
    let id = spec.frontmatter.id;
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
    let path = Path::new("rules").join(format!("{file_prefix}{id}.mdc"));

    let file =
        GeneratedFile::text(Provider::Cursor, FileKind::Rules, path, content).with_spec_id(&id);
    let deliveries = Delivery::from_file(&id, &file, frontmatter.carried());
    Ok((vec![file], deliveries))
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
    use crate::mcp::McpServers;
    use crate::presets::ProviderPresets;
    use crate::spec::{
        AgentFrontmatter, AgentSpec, ExecutionFrontmatter, RuleFrontmatter, RuleSpec,
        SkillFrontmatter, SkillSpec,
    };

    fn compile_one_with_presets(
        spec: Spec,
        cfg: Option<&AdapterConfig>,
        presets: &ProviderPresetsMap,
    ) -> Vec<GeneratedFile> {
        let home = Path::new("/tmp/home");
        let cwd = Path::new("/tmp/cwd");
        let ctx = CompileCtx {
            mode: SyncDestinationMode::Compile,
            home,
            cwd,
            target_dir: None,
            presets,
            mcp_servers: &McpServers::new(),
            adapter_config: cfg,
            overwrite: false,
        };
        CursorAdapter.compile(&[spec], &ctx).expect("compile").files
    }

    fn compile_one(spec: Spec, cfg: Option<&AdapterConfig>) -> Vec<GeneratedFile> {
        compile_one_with_presets(spec, cfg, &HashMap::new())
    }

    /// `agent`, but naming an execution preset so a test can exercise preset
    /// resolution without rebuilding `AgentSpec` inline.
    fn agent_with_preset(id: &str, preset_name: &str) -> Spec {
        Spec::Agent(AgentSpec {
            path: "test.md".into(),
            frontmatter: AgentFrontmatter {
                id: id.to_string(),
                description: "Test agent".to_string(),
                tags: None,
                execution: Some(ExecutionFrontmatter {
                    preset: Some(preset_name.to_string()),
                }),
                capabilities: None,
            },
            body: "Body.".to_string(),
        })
    }

    /// A single-entry presets map whose Cursor half is built from `preset`.
    fn presets_with_cursor(preset: CursorPreset) -> ProviderPresetsMap {
        HashMap::from([(
            "default".to_string(),
            ProviderPresets {
                claude: None,
                cursor: Some(preset),
                opencode: None,
            },
        )])
    }

    /// The generated `model:` line for an agent compiled against `preset`.
    fn model_line(preset: CursorPreset) -> String {
        let files = compile_one_with_presets(
            agent_with_preset("test-agent", "default"),
            None,
            &presets_with_cursor(preset),
        );
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");
        content
            .lines()
            .find(|l| l.starts_with("model:"))
            .unwrap_or("<no model line>")
            .to_string()
    }

    /// The composed `model` value and the delivery record come from one
    /// expression, and this is the guard on the one place they could drift:
    /// nothing can read the bracket back to recompute the record.
    #[test]
    fn test_cursor_bracket_and_record_agree() {
        let preset = CursorPreset {
            model: Some("claude-opus-5".to_string()),
            effort: Some("high".to_string()),
            fast: Some(false),
            context: Some("300k".to_string()),
            params: BTreeMap::from([("optimize_for".to_string(), "cost".to_string())]),
        };
        let presets = presets_with_cursor(preset.clone());
        let Spec::Agent(spec) = agent_with_preset("test-agent", "default") else {
            panic!("agent_with_preset built a non-agent spec")
        };
        let (files, deliveries) = adapt_agent_spec(spec, &presets, None).expect("adapt");

        let content = String::from_utf8(files[0].content.clone()).expect("utf8");
        assert!(
            content.contains(
                "model: claude-opus-5[effort=high,fast=false,context=300k,optimize_for=cost]"
            ),
            "unexpected model line in {content}"
        );

        let recorded: Vec<SettingKey> = deliveries.iter().map(|d| d.setting().clone()).collect();
        assert_eq!(
            recorded,
            vec![
                SettingKey::Model,
                SettingKey::Effort,
                SettingKey::Fast,
                SettingKey::Context,
                SettingKey::Param("optimize_for".to_string()),
            ]
        );
    }

    /// Cursor's skill schema has no model field, so no preset value reaches a
    /// generated Cursor skill file. Pinned rather than left to the empty
    /// `carriable(Skills)` table, which states the same fact from the other
    /// side.
    #[test]
    fn test_cursor_skill_frontmatter_carries_nothing() {
        let frontmatter = CursorSkillFrontmatter {
            name: "s".to_owned(),
            description: "d".to_owned(),
            disable_model_invocation: false,
        };
        assert!(frontmatter.carried().is_empty());
    }

    /// Pins both the composition and the emission order. Every other Cursor
    /// bracket assertion depends on that order, so a reordering of the adapter's
    /// array literal fails here with a clear message rather than several with
    /// opaque ones.
    #[test]
    fn test_adapt_agent_composes_all_options_in_order() {
        let line = model_line(CursorPreset {
            model: Some("claude-opus-5".to_string()),
            effort: Some("high".to_string()),
            fast: Some(false),
            context: Some("300k".to_string()),
            params: std::collections::BTreeMap::new(),
        });
        assert_eq!(
            line,
            "model: claude-opus-5[effort=high,fast=false,context=300k]"
        );
    }

    /// `params` composes after the named options, in `BTreeMap` key order, so a
    /// bracket option agentspec has no field for is still expressible. Cursor's
    /// option set is account- and model-specific — `optimize_for` is documented
    /// with no named field here — so without this the ban would delete the
    /// capability rather than relocate it.
    #[test]
    fn test_adapt_agent_composes_params_after_named_options() {
        let line = model_line(CursorPreset {
            model: Some("auto-smart".to_string()),
            effort: Some("high".to_string()),
            params: BTreeMap::from([
                ("optimize_for".to_string(), "cost".to_string()),
                ("a_first_by_key".to_string(), "1".to_string()),
            ]),
            ..CursorPreset::default()
        });
        assert_eq!(
            line,
            "model: auto-smart[effort=high,a_first_by_key=1,optimize_for=cost]"
        );
    }

    #[test]
    fn test_adapt_agent_params_alone_composes_bracket() {
        let line = model_line(CursorPreset {
            model: Some("auto-smart".to_string()),
            params: BTreeMap::from([("optimize_for".to_string(), "cost".to_string())]),
            ..CursorPreset::default()
        });
        assert_eq!(line, "model: auto-smart[optimize_for=cost]");
    }

    #[test]
    fn test_adapt_agent_single_option_has_no_trailing_comma() {
        let line = model_line(CursorPreset {
            model: Some("claude-opus-5".to_string()),
            effort: Some("high".to_string()),
            ..CursorPreset::default()
        });
        assert_eq!(line, "model: claude-opus-5[effort=high]");
    }

    /// `fast = true` composed alone, so a change to how the bool is stringified
    /// fails here rather than only inside the combined ordering case.
    #[test]
    fn test_adapt_agent_renders_fast_true() {
        let line = model_line(CursorPreset {
            model: Some("claude-opus-5".to_string()),
            fast: Some(true),
            ..CursorPreset::default()
        });
        assert_eq!(line, "model: claude-opus-5[fast=true]");
    }

    #[test]
    fn test_adapt_agent_no_options_emits_bare_model() {
        let line = model_line(CursorPreset {
            model: Some("claude-opus-5".to_string()),
            ..CursorPreset::default()
        });
        assert_eq!(line, "model: claude-opus-5");
    }

    /// `fast = false` renders rather than being skipped: the author asked for it
    /// explicitly, and dropping it would be agentspec deciding the value is
    /// redundant. Whether Cursor *acts* on it is unmeasured —
    /// `experiments/cursor-subagent-model-options/` records the arm as not
    /// discriminating, because a default-valued option is invisible to the
    /// flattened `subagent_model` oracle either way.
    #[test]
    fn test_adapt_agent_renders_fast_false() {
        let line = model_line(CursorPreset {
            model: Some("claude-opus-5".to_string()),
            fast: Some(false),
            ..CursorPreset::default()
        });
        assert_eq!(line, "model: claude-opus-5[fast=false]");
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
                args: None,
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

    /// Pins the README's claim that execution presets are inert on Cursor
    /// skills. `CursorSkillFrontmatter` has no model field, so `adapt_skill_spec`
    /// is not even handed the preset map — a change that started emitting one
    /// would otherwise pass the suite silently.
    #[test]
    fn test_adapt_skill_ignores_cursor_preset() {
        let spec = Spec::Skill(SkillSpec {
            path: "test.md".into(),
            frontmatter: SkillFrontmatter {
                id: "test-skill".to_string(),
                description: Some("Test skill".to_string()),
                tags: None,
                execution: Some(ExecutionFrontmatter {
                    preset: Some("default".to_string()),
                }),
                capabilities: None,
                user_invocable: true,
                agent_invocable: true,
            },
            body: "Body.".to_string(),
            supporting_files: IndexMap::new(),
        });

        let presets = presets_with_cursor(CursorPreset {
            model: Some("claude-opus-5".to_string()),
            effort: Some("high".to_string()),
            fast: Some(false),
            context: Some("300k".to_string()),
            params: std::collections::BTreeMap::new(),
        });

        // Every emitted file, not just the first: a preset leaking into a
        // supporting file or a future companion emission should fail too.
        for file in compile_one_with_presets(spec, None, &presets) {
            let content = String::from_utf8(file.content.clone()).expect("expected value");
            assert!(
                !content.contains("model:") && !content.contains("effort="),
                "no preset value should reach a Cursor skill file ({}):\n{content}",
                file.path.display()
            );
        }
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
            mcp_servers: &McpServers::new(),
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
            mcp_servers: &McpServers::new(),
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
            mcp_servers: &McpServers::new(),
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
            mcp_servers: &McpServers::new(),
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

    #[test]
    fn test_hook_command_preview_bundled_shape_with_quoted_args() {
        let preview = CursorAdapter.hook_command_preview(
            HookEvent::PreToolUse,
            std::path::Path::new("scripts/audit.sh"),
            "audit-bash",
            &["--strict".to_string(), "two words".to_string()],
        );
        assert_eq!(
            preview,
            "${CURSOR_PLUGIN_ROOT}/hooks/scripts/_wrappers/pre_tool_use.sh ${CURSOR_PLUGIN_ROOT}/hooks/scripts/audit.sh audit-bash '--strict' 'two words'"
        );
        assert!(
            preview.contains("${CURSOR_PLUGIN_ROOT}"),
            "the Bundled anchor must appear unexpanded, got: {preview}"
        );
    }

    #[test]
    fn test_hook_command_preview_normalizes_scripts_prefix_once() {
        // The preview takes the raw `hooks.toml` `script` path (still
        // carrying its `scripts/` prefix) and normalizes it internally via
        // `script_filename` — callers never derive the filename
        // themselves, which is what rules out `scripts/scripts/`.
        let preview = CursorAdapter.hook_command_preview(
            HookEvent::PreToolUse,
            std::path::Path::new("scripts/audit.sh"),
            "audit-bash",
            &[],
        );
        assert!(
            preview.contains("/hooks/scripts/audit.sh"),
            "expected exactly one scripts/ segment, got: {preview}"
        );
        assert!(
            !preview.contains("scripts/scripts/"),
            "scripts/ prefix should not double up, got: {preview}"
        );
    }
}

#[cfg(test)]
mod preset_validation_tests {
    use super::*;

    fn cursor(model: Option<&str>) -> CursorPreset {
        CursorPreset {
            model: model.map(str::to_string),
            ..CursorPreset::default()
        }
    }

    #[test]
    fn test_cursor_validate_rejects_bracketed_model() {
        let preset = cursor(Some("claude-opus-5[effort=high]"));
        let err = validate_cursor_preset(&preset, "x").expect_err("expected rejection");
        let msg = err.to_string();
        assert!(msg.contains("presets.x.cursor"), "error: {msg}");
        assert!(msg.contains("bare model id"), "error: {msg}");
    }

    /// Every option arm exercised separately, so `any_option_set` cannot pass
    /// by covering only the first field.
    #[test]
    fn test_cursor_validate_rejects_each_option_without_model() {
        let cases: [(&str, CursorPreset); 3] = [
            (
                "effort",
                CursorPreset {
                    effort: Some("high".to_string()),
                    ..CursorPreset::default()
                },
            ),
            (
                "fast",
                CursorPreset {
                    fast: Some(true),
                    ..CursorPreset::default()
                },
            ),
            (
                "context",
                CursorPreset {
                    context: Some("300k".to_string()),
                    ..CursorPreset::default()
                },
            ),
        ];

        for (field, preset) in cases {
            let Err(err) = validate_cursor_preset(&preset, "x") else {
                panic!("{field} alone should be rejected");
            };
            let msg = err.to_string();
            assert!(msg.contains("presets.x.cursor"), "{field}: {msg}");
            assert!(msg.contains("require `model`"), "{field}: {msg}");
        }
    }

    /// An empty value composes a malformed bracket rather than being skipped:
    /// `model = ""` yields `[effort=high]` and `effort = ""` yields
    /// `model[effort=]`. Cursor rejects nothing, so both degrade silently.
    #[test]
    fn test_cursor_validate_rejects_empty_values() {
        let cases: [(&str, CursorPreset); 4] = [
            ("model", cursor(Some(""))),
            ("model", cursor(Some("   "))),
            (
                "effort",
                CursorPreset {
                    model: Some("claude-opus-5".to_string()),
                    effort: Some(String::new()),
                    ..CursorPreset::default()
                },
            ),
            (
                "context",
                CursorPreset {
                    model: Some("claude-opus-5".to_string()),
                    context: Some("  ".to_string()),
                    ..CursorPreset::default()
                },
            ),
        ];

        for (field, preset) in cases {
            let Err(err) = validate_cursor_preset(&preset, "x") else {
                panic!("empty {field} should be rejected");
            };
            let msg = err.to_string();
            assert!(msg.contains("presets.x.cursor"), "{field}: {msg}");
            assert!(
                msg.contains("must not be empty") || msg.contains("must not contain whitespace"),
                "{field}: {msg}"
            );
        }
    }

    /// Whitespace anywhere in a value composes a malformed bracket, and a
    /// leading space additionally forces serde to emit a quoted scalar —
    /// changing the model id rather than only the option suffix.
    #[test]
    fn test_cursor_validate_rejects_whitespace_in_values() {
        let cases: [(&str, CursorPreset); 3] = [
            ("model", cursor(Some(" claude-opus-5 "))),
            (
                "effort",
                CursorPreset {
                    model: Some("claude-opus-5".to_string()),
                    effort: Some("high 5".to_string()),
                    ..CursorPreset::default()
                },
            ),
            (
                "context",
                CursorPreset {
                    model: Some("claude-opus-5".to_string()),
                    context: Some("300 k".to_string()),
                    ..CursorPreset::default()
                },
            ),
        ];

        for (field, preset) in cases {
            let Err(err) = validate_cursor_preset(&preset, "x") else {
                panic!("whitespace in {field} should be rejected");
            };
            let msg = err.to_string();
            assert!(msg.contains("presets.x.cursor"), "{field}: {msg}");
            assert!(
                msg.contains("must not contain whitespace"),
                "{field}: {msg}"
            );
        }
    }

    /// Every `NAMED_CURSOR_OPTIONS` entry names a real `CursorPreset` field.
    ///
    /// Deserializing `<name> = 0` fails either way — the three fields are
    /// `String`, `bool`, `String` — but the *error* distinguishes the cases: a
    /// live field gives a type error, a renamed or removed one gives
    /// `unknown field` under `deny_unknown_fields`. So renaming `context` to
    /// `thinking` without updating the array fails here, which is the drift that
    /// would otherwise let `params.context` re-spell a named option.
    ///
    /// The destructuring binding below is the other half: adding a field is a
    /// compile error here, forcing whoever adds it to look at this test. What
    /// neither half catches is a fourth field added, bound as `_`, and left out
    /// of the array — Rust has no field-name reflection to close that without a
    /// macro, so it is a known limit rather than a covered case.
    #[test]
    fn test_named_cursor_options_are_real_fields() {
        let CursorPreset {
            model: _,
            effort: _,
            fast: _,
            context: _,
            params: _,
        } = CursorPreset::default();

        for name in NAMED_CURSOR_OPTIONS {
            let err = toml::from_str::<CursorPreset>(&format!("{name} = 0"))
                .expect_err("0 is the wrong type for every named option")
                .to_string();
            assert!(
                !err.contains("unknown field"),
                "`{name}` is in NAMED_CURSOR_OPTIONS but is not a CursorPreset field: {err}"
            );
        }
    }

    /// `params` keys must not collide with each other, not just with the named
    /// fields — `BTreeMap` orders by byte, so case variants are not adjacent.
    #[test]
    fn test_cursor_validate_rejects_params_keys_colliding_with_each_other() {
        let preset = CursorPreset {
            model: Some("auto-smart".to_string()),
            params: BTreeMap::from([
                ("optimize_for".to_string(), "cost".to_string()),
                ("Optimize_For".to_string(), "balanced".to_string()),
            ]),
            ..CursorPreset::default()
        };
        let Err(err) = validate_cursor_preset(&preset, "x") else {
            panic!("params keys differing only in case should collide");
        };
        assert!(err.to_string().contains("differ only in case"), "{err}");
    }

    /// Case-insensitive, because `[effort=high,Effort=low]` is one option
    /// spelled twice whichever way Cursor folds ids.
    #[test]
    fn test_cursor_validate_rejects_params_key_colliding_case_insensitively() {
        let preset = CursorPreset {
            model: Some("claude-opus-5".to_string()),
            effort: Some("high".to_string()),
            params: BTreeMap::from([("Effort".to_string(), "low".to_string())]),
            ..CursorPreset::default()
        };
        let Err(err) = validate_cursor_preset(&preset, "x") else {
            panic!("differently-cased params key should collide");
        };
        let msg = err.to_string();
        assert!(msg.contains("params.Effort"), "{msg}");
        assert!(msg.contains("`effort` field"), "{msg}");
    }

    /// A `params` key that duplicates a named field would give one option two
    /// spellings — the exact thing the bracket ban exists to prevent.
    #[test]
    fn test_cursor_validate_rejects_params_key_colliding_with_named_field() {
        for named in ["effort", "fast", "context"] {
            let preset = CursorPreset {
                model: Some("claude-opus-5".to_string()),
                params: BTreeMap::from([(named.to_string(), "x".to_string())]),
                ..CursorPreset::default()
            };
            let Err(err) = validate_cursor_preset(&preset, "x") else {
                panic!("params.{named} should collide with the named field");
            };
            let msg = err.to_string();
            assert!(msg.contains(&format!("params.{named}")), "{named}: {msg}");
            assert!(msg.contains("duplicates"), "{named}: {msg}");
        }
    }

    /// Keys are composed into the bracket just like values, so they carry the
    /// same delimiter and whitespace rules.
    #[test]
    fn test_cursor_validate_rejects_malformed_params_key_or_value() {
        let cases = [
            ("bad=key", "cost"),
            ("optimize for", "cost"),
            ("optimize_for", "co,st"),
            ("optimize_for", ""),
        ];
        for (key, value) in cases {
            let preset = CursorPreset {
                model: Some("claude-opus-5".to_string()),
                params: BTreeMap::from([(key.to_string(), value.to_string())]),
                ..CursorPreset::default()
            };
            assert!(
                validate_cursor_preset(&preset, "x").is_err(),
                "params {key:?}={value:?} should be rejected"
            );
        }
    }

    /// `params` alone still requires a `model` — Cursor cannot express a bracket
    /// option apart from the id it suffixes.
    #[test]
    fn test_cursor_validate_rejects_params_without_model() {
        let preset = CursorPreset {
            params: BTreeMap::from([("optimize_for".to_string(), "cost".to_string())]),
            ..CursorPreset::default()
        };
        let Err(err) = validate_cursor_preset(&preset, "x") else {
            panic!("params with no model should be rejected");
        };
        assert!(err.to_string().contains("require `model`"), "{err}");
    }

    #[test]
    fn test_cursor_validate_accepts_bare_model() {
        validate_cursor_preset(&cursor(Some("claude-opus-5")), "x")
            .expect("bare model should validate");
    }

    #[test]
    fn test_cursor_validate_accepts_model_with_all_options() {
        let preset = CursorPreset {
            model: Some("claude-opus-5".to_string()),
            effort: Some("high".to_string()),
            fast: Some(false),
            context: Some("300k".to_string()),
            params: BTreeMap::from([("optimize_for".to_string(), "cost".to_string())]),
        };
        validate_cursor_preset(&preset, "x").expect("model plus all options should validate");
    }

    /// A preset configuring nothing at all is inert, not invalid.
    #[test]
    fn test_cursor_validate_accepts_empty() {
        validate_cursor_preset(&CursorPreset::default(), "x")
            .expect("empty preset should validate");
    }
}

#[cfg(test)]
mod mcp_declaration_tests {
    use super::*;
    use crate::mcp::McpServers;

    #[test]
    fn test_mcp_cursor_server_override_must_be_mcp_name() {
        let declarations = Declarations {
            mcp: McpServers::from([(
                "quip".to_owned(),
                McpServer {
                    cursor: Some(CursorMcpServer {
                        server: Some("a:b".to_owned()),
                    }),
                    ..McpServer::default()
                },
            )]),
            ..Declarations::default()
        };
        assert_eq!(
            CursorAdapter.validate_declarations(&declarations),
            ["[mcp.quip.cursor] `server` must match [A-Za-z0-9_-]+ (got \"a:b\")"]
        );
    }
}
