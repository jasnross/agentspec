use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use jsonc_parser::cst::CstObject;
use serde::Serialize;

use super::hook_compile::{self, HookSynthesis};
use super::hooks_helpers::{
    has_agentspec_entries, is_owned_entry, node_as_object, open_or_create_array,
    open_or_create_object, prune_empty_event_arrays, value_to_cst_input,
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
use crate::presets::{ClaudeEffort, ProviderPresetsMap};
use crate::provider::Provider;
use crate::spec::{AgentSpec, HookEvent, HookSpec, RuleSpec, SkillSpec, Spec, ToolFrontmatter};

// See: https://code.claude.com/docs/en/sub-agents#supported-frontmatter-fields
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
struct ClaudeAgentFrontmatter {
    name: String,
    description: String,
    model: Option<String>,
    effort: Option<ClaudeEffort>,
    tools: Option<Vec<ClaudeTool>>,
}

// See: https://code.claude.com/docs/en/memory#path-specific-rules
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
struct ClaudeRuleFrontmatter {
    paths: Option<Vec<String>>,
}

// See: https://code.claude.com/docs/en/skills#frontmatter-reference
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ClaudeSkillFrontmatter {
    description: String,
    model: Option<String>,
    effort: Option<ClaudeEffort>,
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
    SendMessage,
    Skill,
    TaskCreate,
    TaskGet,
    TaskList,
    TaskOutput,
    TaskStop,
    TaskUpdate,
    ToolSearch,
    WebFetch,
    WebSearch,
    Write,
}

const HOST_FILENAME: &str = "settings.json";
const HOOK_DOTDIR: &str = ".claude";
/// Claude's plugin-root env var. The host runtime sets `${CLAUDE_PLUGIN_ROOT}`
/// in plugin scope; agentspec also assigns it inline in merged-mode hook
/// commands so plugin-shaped scripts can reference sibling assets like
/// `${CLAUDE_PLUGIN_ROOT}/rules` when synced project/user-wide. See:
/// <https://code.claude.com/docs/en/plugins>.
const PLUGIN_ROOT_ENV_VAR: &str = "CLAUDE_PLUGIN_ROOT";
const PLUGIN_MANIFEST_DIR: &str = ".claude-plugin";

/// Zero-sized adapter for the Claude provider.
#[derive(Debug)]
pub struct ClaudeAdapter;

impl Adapter for ClaudeAdapter {
    fn compile(&self, specs: &[Spec], ctx: &CompileCtx<'_>) -> Result<AdapterOutput> {
        let mut files = Vec::new();
        for spec in specs {
            match spec {
                Spec::Agent(s) => files.extend(adapt_agent_spec(
                    s.clone(),
                    ctx.presets,
                    ctx.adapter_config,
                )?),
                Spec::Skill(s) => files.extend(adapt_skill_spec(
                    s.clone(),
                    ctx.presets,
                    ctx.adapter_config,
                )?),
                Spec::Rule(s) => files.extend(adapt_rule_spec(s, ctx.adapter_config)?),
                // Hook scripts (entry scripts AND helpers under `scripts/`) are
                // emitted by `synthesize_hooks` exactly once per provider, drawn
                // from `supporting_files` collected by `load_hook_specs`. Per-spec
                // dispatch contributes nothing — emitting per spec would duplicate
                // every helper for every hook entry.
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

        // Emit `.claude-plugin/plugin.json` in plugin mode whenever the
        // binary supplied manifest fields. Validation upstream guarantees
        // `plugin-name` is set when this is `Some`.
        if ctx.mode == SyncDestinationMode::Plugin
            && let Some(manifest) = ctx.adapter_config.and_then(|c| c.plugin_manifest.as_ref())
        {
            files.push(build_plugin_manifest_file(manifest)?);
        }

        let dest_root = config_dir(ctx.mode, ctx.target_dir, ctx.home, ctx.cwd);

        let mut patches: Vec<Box<dyn ForwardPatch>> = Vec::new();
        if emit_mode.is_merged() {
            patches.push(Box::new(ClaudeHooksPatch {
                host_path: dest_root.join(HOST_FILENAME),
                owned_entries,
                force: ctx.overwrite,
            }));
        }

        Ok(AdapterOutput {
            files,
            patches,
            dest_root,
            // Claude honors everything agentspec emits: `emits_hooks()` is
            // `true` and both `fully_implements_canonical_output()` and
            // `supports_path_scoped_rules()` take the permissive default.
            degradations: Vec::new(),
        })
    }

    fn removal_patches(&self, ctx: &RemoveCtx<'_>) -> RemovalOutput {
        let dest_root = config_dir(ctx.mode, ctx.target_dir, ctx.home, ctx.cwd);
        let emit_mode = ctx.mode.to_hook_emit_mode();
        let mut patches: Vec<Box<dyn ReversePatch>> = Vec::new();
        if emit_mode.is_merged() {
            patches.push(Box::new(ClaudeRemoveHooksPatch {
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
                Box::new(ClaudeRemoveHooksPatch { host_path })
            })
            .collect()
    }

    /// Resolve a canonical tool to the name a Claude spec body should reference.
    ///
    /// For tools that fan out to multiple frontmatter entries (e.g., `Tasks`),
    /// returns a single representative name — the one most commonly referenced
    /// in spec prose.
    fn body_tool_name(&self, tool: &ToolFrontmatter) -> &'static str {
        match tool {
            ToolFrontmatter::Read => "Read",
            ToolFrontmatter::Write => "Write",
            ToolFrontmatter::Edit => "Edit",
            ToolFrontmatter::Grep => "Grep",
            ToolFrontmatter::Glob => "Glob",
            ToolFrontmatter::Shell => "Bash",
            ToolFrontmatter::WebFetch => "WebFetch",
            ToolFrontmatter::WebSearch => "WebSearch",
            ToolFrontmatter::Question => "AskUserQuestion",
            ToolFrontmatter::Tasks => "TaskCreate",
            ToolFrontmatter::Subagent => "Agent",
            ToolFrontmatter::Skill => "Skill",
        }
    }

    fn matcher_subagent_type<'a>(&self, canonical: &'a str) -> &'a str {
        match canonical {
            "general" => "general-purpose",
            "explore" => "Explore",
            "plan" => "Plan",
            other => other,
        }
    }

    /// Returns the name which should be used to refer to the spec in the generated body content.
    ///
    /// For Claude, all spec types use `{content_prefix}{id}` when a content prefix
    /// is configured (either explicitly or derived from `prefix`).
    fn body_spec_name(&self, spec: &Spec, cfg: Option<&AdapterConfig>) -> String {
        let id = spec.id();
        match cfg.and_then(AdapterConfig::content_prefix) {
            Some(prefix) => format!("{prefix}{id}"),
            None => id.to_owned(),
        }
    }

    fn body_skill_root(&self) -> Option<&'static str> {
        Some("${CLAUDE_SKILL_DIR}")
    }

    fn emits_hooks(&self) -> bool {
        true
    }

    fn plugin_manifest_dir(&self) -> Option<&'static str> {
        Some(PLUGIN_MANIFEST_DIR)
    }
}

impl ClaudeAdapter {
    /// Translate a canonical `HookEvent` to Claude's `PascalCase` event name.
    pub(crate) fn event_name(event: HookEvent) -> &'static str {
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

    /// Build the JSON object for one entry in Claude's `hooks.json` matcher group.
    /// Used by both the bundled-mode whole-file synthesis (`build_claude_hooks_json`)
    /// and the merged-mode CST merge (`merge_into_settings`).
    pub(crate) fn entry_to_json(e: &EmittedHookEntry) -> serde_json::Value {
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

    /// Merge agentspec-owned entries into a parsed top-level `settings.json`
    /// CST object. Handles top-level extras (none for Claude), opening the
    /// `hooks` object, the per-event nesting depth, and the per-entry shape.
    /// Implementations MUST NOT prune empty event arrays — locked by
    /// `test_merge_claude_leaves_empty_event_array_after_removing_all_owned_entries`.
    pub(crate) fn merge_into_settings(
        top: &CstObject,
        owned_entries: &[EmittedHookEntry],
        force: bool,
    ) -> Result<()> {
        use serde_json::{Map, Value, json};

        let hooks_obj = open_or_create_object(top, "hooks", force, "hooks")?;

        // Step 1 — Remove every agentspec-owned entry under every event. We
        // can't restrict to events present in `owned_entries` because re-syncing
        // with one fewer hook must remove the orphan from its old event too.
        // Sync doesn't care about the removed-count, so discard.
        let _ = remove_owned_entries(&hooks_obj);

        // Step 2 — Append new entries grouped by `(event, matcher)`.
        // `BTreeMap` sort order matters: when no event key exists yet,
        // `array_value_or_create` creates it at end-of-object, so new keys land
        // in alphabetical order — matching `build_claude_hooks_json`.
        let mut grouped: BTreeMap<&'static str, Vec<&EmittedHookEntry>> = BTreeMap::new();
        for e in owned_entries {
            grouped
                .entry(Self::event_name(e.event))
                .or_default()
                .push(e);
        }
        for (event_name, entries) in &grouped {
            let event_arr = open_or_create_array(
                &hooks_obj,
                event_name,
                force,
                &format!("hooks.{event_name}"),
            )?;
            // Within an event, group entries by matcher into one matcher-wrapper
            // object (Claude's documented shape). Insertion order preserved
            // from the spec list (which preserves IndexMap order from hooks.toml).
            let mut by_matcher: IndexMap<Option<String>, Vec<&EmittedHookEntry>> = IndexMap::new();
            for &e in entries {
                by_matcher.entry(e.matcher.clone()).or_default().push(e);
            }
            for (matcher, group_entries) in by_matcher {
                let mut wrapper = Map::new();
                if let Some(m) = matcher {
                    wrapper.insert("matcher".to_string(), json!(m));
                }
                let inner: Vec<Value> = group_entries
                    .iter()
                    .map(|e| Self::entry_to_json(e))
                    .collect();
                wrapper.insert("hooks".to_string(), Value::Array(inner));
                event_arr.append(value_to_cst_input(Value::Object(wrapper)));
            }
        }
        Ok(())
    }

    /// Strip agentspec-owned entries from a parsed `settings.json` CST top
    /// object, prune emptied containers, and report whether the host file
    /// should be deleted.
    ///
    /// Claude predicate: delete iff at least one `_agentspec_id`-tagged entry
    /// was removed AND no top-level keys survive. `settings.json` doesn't
    /// carry a `version` key, so any surviving top-level key (e.g.
    /// `permissions`, `env`) keeps the file.
    pub(crate) fn tidy_settings(top: &CstObject) -> TidyOutcome {
        let Some(hooks_obj) = top.object_value("hooks") else {
            return TidyOutcome {
                user_entries_remaining: 0,
                file_should_be_deleted: false,
            };
        };

        // Reuse the merge-side `remove_owned_entries` for the inner-most layer
        // (entry removal + matcher-group pruning when the inner `hooks` array
        // empties); the tidy path adds the empty-event-array prune that the
        // merge path deliberately skips.
        let removed_owned = remove_owned_entries(&hooks_obj);
        prune_empty_event_arrays(&hooks_obj);

        if hooks_obj.properties().is_empty()
            && let Some(hooks_prop) = top.get("hooks")
        {
            hooks_prop.remove();
        }

        let file_should_be_deleted = removed_owned > 0 && top.properties().is_empty();

        TidyOutcome {
            user_entries_remaining: count_user_entries(top),
            file_should_be_deleted,
        }
    }
}

/// Claude's `.claude-plugin/plugin.json` shape.
///
/// Emits `name`, `version`, `description`, `author { name, email? }`, `repository`,
/// and `license`. Additional fields documented in Claude's plugin schema
/// (`dependencies`, `contributes`, `userConfig`, etc.) are out of scope.
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
struct ClaudePluginManifestJson<'a> {
    name: &'a str,
    version: Option<&'a str>,
    description: Option<&'a str>,
    author: Option<PluginAuthorJson<'a>>,
    repository: Option<&'a str>,
    license: Option<&'a str>,
}

/// Author sub-record. Both providers' author schemas accept `{ name, email? }`.
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
struct PluginAuthorJson<'a> {
    name: &'a str,
    email: Option<&'a str>,
}

/// Build the `.claude-plugin/plugin.json` `GeneratedFile`.
fn build_plugin_manifest_file(manifest: &SpecPluginManifest) -> Result<GeneratedFile> {
    let json = ClaudePluginManifestJson {
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
        .context("failed to serialize Claude .claude-plugin/plugin.json")?;
    content.push(b'\n');
    Ok(GeneratedFile::binary(
        Provider::Claude,
        FileKind::PluginManifest,
        Path::new(PLUGIN_MANIFEST_DIR).join("plugin.json"),
        content,
        None,
    ))
}

/// Forwards to the shared `hook_compile::synthesize_hooks` with Claude's
/// provider, dotdir, plugin-root env-var name, and JSON-builder bound.
/// Keeps the adapter-local call site stable while the shared synthesis
/// lives in one place.
fn synthesize_hooks(specs: &[&HookSpec], emit_mode: HookEmitMode) -> Result<HookSynthesis> {
    hook_compile::synthesize_hooks(
        Provider::Claude,
        HOOK_DOTDIR,
        PLUGIN_ROOT_ENV_VAR,
        specs,
        emit_mode,
        build_claude_hooks_json,
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

/// Forward-direction settings.json patch.
///
/// Constructed by `ClaudeAdapter::compile` in Merged (User/Project) modes.
/// Merges agentspec-owned hook entries into `settings.json`.
#[derive(Debug)]
pub(crate) struct ClaudeHooksPatch {
    host_path: PathBuf,
    owned_entries: Vec<EmittedHookEntry>,
    /// `--force`/`overwrite=true`: replace a non-object `hooks` (or non-array
    /// per-event) value with `{}`/`[]` before merging, instead of erroring.
    force: bool,
}

impl ForwardPatch for ClaudeHooksPatch {
    fn run(&self, dry_run: bool) -> Result<()> {
        let entries = &self.owned_entries;
        let force = self.force;
        merge_owned(
            &self.host_path,
            entries.is_empty(),
            |top| entries.is_empty() && top.get("hooks").is_none(),
            |top| ClaudeAdapter::merge_into_settings(top, entries, force),
            dry_run,
        )
    }
}

/// Reverse-direction settings.json patch.
///
/// Constructed by `ClaudeAdapter::removal_patches` in Merged modes. Strips
/// agentspec-owned entries and tidies emptied containers.
#[derive(Debug)]
pub(crate) struct ClaudeRemoveHooksPatch {
    host_path: PathBuf,
}

impl ReversePatch for ClaudeRemoveHooksPatch {
    fn run_remove(&self, dry_run: bool) -> Result<()> {
        let report = remove_owned(&self.host_path, ClaudeAdapter::tidy_settings, dry_run)?;
        report.print_summary(dry_run);
        Ok(())
    }
}

/// Walks `hooks.<event>[<matcher_group>].hooks[]` removing any entry tagged
/// with `_agentspec_id`. If a matcher group ends up with an empty `hooks`
/// array, the group itself is removed. Empty event arrays are left alone —
/// the user might still have entries to add later.
///
/// Returns the count of `_agentspec_id`-tagged entries that were removed.
/// `tidy_settings` uses the count to gate `file_should_be_deleted`; the
/// merge path discards it.
fn remove_owned_entries(hooks_obj: &CstObject) -> usize {
    let mut removed = 0usize;
    let event_props: Vec<_> = hooks_obj.properties();
    for event_prop in event_props {
        let Some(event_arr) = event_prop.array_value() else {
            continue;
        };
        let groups: Vec<_> = event_arr.elements();
        for group_node in groups {
            let Some(group_obj) = node_as_object(&group_node) else {
                continue;
            };
            let Some(inner) = group_obj.array_value("hooks") else {
                continue;
            };
            let inner_entries: Vec<_> = inner.elements();
            for entry in inner_entries {
                if is_owned_entry(&entry) {
                    entry.remove();
                    removed += 1;
                }
            }
            if group_obj
                .array_value("hooks")
                .is_some_and(|a| a.elements().is_empty())
            {
                group_obj.remove();
            }
        }
    }
    removed
}

/// Counts user-authored Claude hook entries: walks every surviving matcher
/// group's inner `hooks` array and counts entries lacking `_agentspec_id`.
fn count_user_entries(top: &CstObject) -> usize {
    let Some(hooks_obj) = top.object_value("hooks") else {
        return 0;
    };
    let mut count = 0;
    for event_prop in hooks_obj.properties() {
        let Some(event_arr) = event_prop.array_value() else {
            continue;
        };
        for group_node in event_arr.elements() {
            let Some(group_obj) = node_as_object(&group_node) else {
                continue;
            };
            let Some(inner) = group_obj.array_value("hooks") else {
                continue;
            };
            for entry in inner.elements() {
                if !is_owned_entry(&entry) {
                    count += 1;
                }
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

    let claude_preset = spec
        .frontmatter
        .execution
        .and_then(|x| x.preset)
        .and_then(|x| presets.get(&x))
        .and_then(|x| x.claude.clone());

    let model = claude_preset.as_ref().and_then(|x| x.model.clone());
    let effort = claude_preset.and_then(|x| x.effort);

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
        effort,
        tools,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}---\n\n{body}");

    Ok(vec![GeneratedFile::text(
        Provider::Claude,
        FileKind::Agents,
        path,
        content,
    )])
}

fn adapt_skill_spec(
    spec: SkillSpec,
    presets: &ProviderPresetsMap,
    cfg: Option<&AdapterConfig>,
) -> Result<Vec<GeneratedFile>> {
    let id = spec.frontmatter.id;
    let description = spec.frontmatter.description.unwrap_or_default();

    let claude_preset = spec
        .frontmatter
        .execution
        .and_then(|x| x.preset)
        .and_then(|x| presets.get(&x))
        .and_then(|x| x.claude.clone());

    let model = claude_preset.as_ref().and_then(|x| x.model.clone());
    let effort = claude_preset.and_then(|x| x.effort);

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
        effort,
        user_invocable,
        disable_model_invocation,
        allowed_tools,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}---\n\n{body}");

    let mut files = vec![GeneratedFile::text(
        Provider::Claude,
        FileKind::Skills,
        skill_dir.join("SKILL.md"),
        content,
    )];

    for (rel_path, sf) in spec.supporting_files {
        files.push(GeneratedFile::binary(
            Provider::Claude,
            FileKind::Skills,
            skill_dir.join(&rel_path),
            sf.content,
            Some(sf.mode),
        ));
    }

    Ok(files)
}

fn adapt_rule_spec(spec: &RuleSpec, cfg: Option<&AdapterConfig>) -> Result<Vec<GeneratedFile>> {
    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let path = Path::new("rules").join(format!("{file_prefix}{}.md", spec.frontmatter.id));
    let body = spec.body.trim();

    let content = if let Some(paths) = spec.frontmatter.paths.clone() {
        let frontmatter = ClaudeRuleFrontmatter { paths: Some(paths) };
        let frontmatter_str = serde_yml::to_string(&frontmatter)?;
        format!("---\n{frontmatter_str}---\n\n{body}\n")
    } else {
        format!("{body}\n")
    };

    Ok(vec![GeneratedFile {
        provider: Provider::Claude,
        kind: FileKind::Rules,
        path,
        content: content.into_bytes(),
        mode: None,
    }])
}

fn adapt_tool(tool: &ToolFrontmatter) -> Vec<ClaudeTool> {
    match tool {
        ToolFrontmatter::Read => vec![ClaudeTool::Read],
        ToolFrontmatter::Write => vec![ClaudeTool::Write],
        ToolFrontmatter::Edit => vec![ClaudeTool::Edit],
        ToolFrontmatter::Grep => vec![ClaudeTool::Grep],
        ToolFrontmatter::Glob => vec![ClaudeTool::Glob],
        ToolFrontmatter::Shell => vec![ClaudeTool::Bash],
        ToolFrontmatter::WebFetch => vec![ClaudeTool::WebFetch],
        ToolFrontmatter::WebSearch => vec![ClaudeTool::WebSearch],
        ToolFrontmatter::Question => vec![ClaudeTool::AskUserQuestion],
        ToolFrontmatter::Tasks => vec![
            ClaudeTool::TaskCreate,
            ClaudeTool::TaskGet,
            ClaudeTool::TaskList,
            ClaudeTool::TaskUpdate,
        ],
        ToolFrontmatter::Subagent => vec![ClaudeTool::Agent, ClaudeTool::SendMessage],
        ToolFrontmatter::Skill => vec![ClaudeTool::Skill],
    }
}

// ── hooks.json synthesis ────────────────────────────────────────────────────

// Claude's documented `hooks.json` shape (see <https://code.claude.com/docs/en/hooks>):
//   { "hooks": { "<EventName>": [ { "matcher": "...", "hooks": [<entry>, ...] }, ... ] } }
//
// Per-entry JSON shaping lives on `ClaudeAdapter::entry_to_json`; the
// CST-aware merge layer (`hooks_merge`) calls it via the inherent helper
// `ClaudeAdapter::merge_into_settings` so both emission paths share one
// source of truth for the `_agentspec_id` sentinel and entry shape.

/// Group entries by `(event, matcher)` and serialize Claude's documented shape.
///
/// Top-level event keys are sorted alphabetically (`BTreeMap`) for stable output.
/// Within an event, matcher groups preserve first-seen order — propagated from
/// the spec list, which itself preserves `IndexMap` authoring order from the
/// `hooks.toml` file.
fn build_claude_hooks_json(entries: &[EmittedHookEntry]) -> Result<String> {
    use serde_json::{Map, Value, json};

    let mut by_event: BTreeMap<&'static str, IndexMap<Option<String>, Vec<Value>>> =
        BTreeMap::new();
    for entry in entries {
        by_event
            .entry(ClaudeAdapter::event_name(entry.event))
            .or_default()
            .entry(entry.matcher.clone())
            .or_default()
            .push(ClaudeAdapter::entry_to_json(entry));
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde::Deserialize;

    use super::*;
    use crate::presets::{ClaudePreset, ProviderPresets};
    use crate::spec::{
        AgentFrontmatter, AgentSpec, CapabilitiesFrontmatter, ExecutionFrontmatter,
        RuleFrontmatter, RuleSpec, SkillFrontmatter,
    };

    fn agent(id: &str, capabilities: Option<CapabilitiesFrontmatter>) -> Spec {
        Spec::Agent(AgentSpec {
            path: "test.md".into(),
            frontmatter: AgentFrontmatter {
                id: id.to_string(),
                description: "Test agent".to_string(),
                tags: None,
                execution: None,
                capabilities,
            },
            body: "Body.".to_string(),
        })
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
            adapter_config: cfg,
            overwrite: false,
        };
        ClaudeAdapter.compile(&[spec], &ctx).expect("compile").files
    }

    fn compile_one(spec: Spec, cfg: Option<&AdapterConfig>) -> Vec<GeneratedFile> {
        compile_one_with_presets(spec, cfg, &HashMap::new())
    }

    /// A single-entry presets map whose Claude half sets both `model` and
    /// `effort`, so an emitted file proves where each key lands.
    fn presets_with_model_and_effort() -> ProviderPresetsMap {
        HashMap::from([(
            "default".to_string(),
            ProviderPresets {
                claude: Some(ClaudePreset {
                    model: Some("opus".to_string()),
                    effort: Some(ClaudeEffort::High),
                }),
                cursor: None,
                opencode: None,
            },
        )])
    }

    #[test]
    fn test_adapt_agent_tools_are_sorted() {
        #[derive(Deserialize)]
        struct Frontmatter {
            tools: Option<Vec<String>>,
        }

        // Tools provided in reverse alphabetical order to confirm sorting.
        let spec = agent(
            "test-agent",
            Some(CapabilitiesFrontmatter {
                tools: Some(vec![
                    ToolFrontmatter::Write,
                    ToolFrontmatter::Read,
                    ToolFrontmatter::Shell,
                ]),
            }),
        );

        let files = compile_one(spec, None);
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
        let spec = agent("test-agent", None);
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
    fn test_adapt_agent_preset_emits_model_and_effort() {
        let files = compile_one_with_presets(
            agent_with_preset("test-agent", "default"),
            None,
            &presets_with_model_and_effort(),
        );
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");

        let expected = concat!(
            "---\n",
            "name: test-agent\n",
            "description: Test agent\n",
            "model: opus\n",
            "effort: high\n",
            "---\n",
            "\n",
            "Body.",
        );
        assert_eq!(content, expected);
    }

    /// Claude's `effort` is independent of `model` — measured at
    /// `outbound-request` depth by `experiments/claude-agent-effort/` with no
    /// `model` key present at all.
    #[test]
    fn test_adapt_agent_preset_effort_without_model() {
        let presets = HashMap::from([(
            "default".to_string(),
            ProviderPresets {
                claude: Some(ClaudePreset {
                    model: None,
                    effort: Some(ClaudeEffort::High),
                }),
                cursor: None,
                opencode: None,
            },
        )]);

        let files =
            compile_one_with_presets(agent_with_preset("test-agent", "default"), None, &presets);
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");

        let expected = concat!(
            "---\n",
            "name: test-agent\n",
            "description: Test agent\n",
            "effort: high\n",
            "---\n",
            "\n",
            "Body.",
        );
        assert_eq!(content, expected);
    }

    #[test]
    fn test_adapt_skill_preset_emits_effort() {
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

        let files = compile_one_with_presets(spec, None, &presets_with_model_and_effort());
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");

        let expected = concat!(
            "---\n",
            "description: Test skill\n",
            "model: opus\n",
            "effort: high\n",
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
        let spec = agent("test-agent", None);
        let files = compile_one(spec, Some(&cfg));
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
        assert_eq!(files[0].path.to_str(), Some("rules/tw-test-rule.md"));
    }

    #[test]
    fn test_adapt_agent_content_prefix_does_not_affect_frontmatter() {
        let cfg = AdapterConfig {
            content_prefix: Some("tw:".to_string()),
            ..AdapterConfig::default()
        };
        let spec = agent("test-agent", None);
        let files = compile_one(spec, Some(&cfg));
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
            content_prefix: Some("tw:".to_string()),
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
            body: String::new(),
        });

        assert_eq!(
            ClaudeAdapter.body_spec_name(&spec, Some(&cfg)),
            "tw:test-agent"
        );
    }

    #[test]
    fn test_body_tool_name_question_maps_to_ask_user_question() {
        assert_eq!(
            ClaudeAdapter.body_tool_name(&ToolFrontmatter::Question),
            "AskUserQuestion"
        );
    }

    #[test]
    fn test_body_tool_name_tasks_maps_to_task_create() {
        assert_eq!(
            ClaudeAdapter.body_tool_name(&ToolFrontmatter::Tasks),
            "TaskCreate"
        );
    }

    #[test]
    fn test_body_tool_name_subagent_maps_to_agent() {
        assert_eq!(
            ClaudeAdapter.body_tool_name(&ToolFrontmatter::Subagent),
            "Agent"
        );
    }

    #[test]
    fn test_body_tool_name_skill_maps_to_skill() {
        assert_eq!(
            ClaudeAdapter.body_tool_name(&ToolFrontmatter::Skill),
            "Skill"
        );
    }

    #[test]
    fn test_matcher_subagent_type_general() {
        assert_eq!(
            ClaudeAdapter.matcher_subagent_type("general"),
            "general-purpose"
        );
    }

    #[test]
    fn test_matcher_subagent_type_explore() {
        assert_eq!(ClaudeAdapter.matcher_subagent_type("explore"), "Explore");
    }

    #[test]
    fn test_matcher_subagent_type_plan() {
        assert_eq!(ClaudeAdapter.matcher_subagent_type("plan"), "Plan");
    }

    #[test]
    fn test_adapt_tool_subagent_maps_to_agent_and_send_message() {
        let tools = adapt_tool(&ToolFrontmatter::Subagent);
        let yaml = serde_yml::to_string(&tools).expect("expected value");
        assert_eq!(yaml, "- Agent\n- SendMessage\n");
    }

    #[test]
    fn test_adapt_tool_skill_maps_to_skill() {
        let tools = adapt_tool(&ToolFrontmatter::Skill);
        let yaml = serde_yml::to_string(&tools).expect("expected value");
        assert_eq!(yaml, "- Skill\n");
    }

    #[test]
    fn test_adapt_tool_tasks_maps_to_task_tracking_tools() {
        let tools = adapt_tool(&ToolFrontmatter::Tasks);
        let yaml = serde_yml::to_string(&tools).expect("expected value");
        assert_eq!(yaml, "- TaskCreate\n- TaskGet\n- TaskList\n- TaskUpdate\n");
    }

    // -- Hook synthesis tests --

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
    fn test_claude_event_name_full_mapping() {
        assert_eq!(
            ClaudeAdapter::event_name(HookEvent::PreToolUse),
            "PreToolUse"
        );
        assert_eq!(
            ClaudeAdapter::event_name(HookEvent::PostToolUse),
            "PostToolUse"
        );
        assert_eq!(
            ClaudeAdapter::event_name(HookEvent::PostToolUseFailure),
            "PostToolUseFailure"
        );
        assert_eq!(
            ClaudeAdapter::event_name(HookEvent::SessionStart),
            "SessionStart"
        );
        assert_eq!(
            ClaudeAdapter::event_name(HookEvent::SessionEnd),
            "SessionEnd"
        );
        assert_eq!(ClaudeAdapter::event_name(HookEvent::Stop), "Stop");
        assert_eq!(
            ClaudeAdapter::event_name(HookEvent::PreCompact),
            "PreCompact"
        );
        assert_eq!(
            ClaudeAdapter::event_name(HookEvent::SubagentStart),
            "SubagentStart"
        );
        assert_eq!(
            ClaudeAdapter::event_name(HookEvent::SubagentStop),
            "SubagentStop"
        );
        assert_eq!(
            ClaudeAdapter::event_name(HookEvent::UserPromptSubmit),
            "UserPromptSubmit"
        );
    }

    #[test]
    fn test_synthesize_hooks_empty_returns_default() {
        let result = synthesize_hooks(&[], HookEmitMode::Bundled).expect("expected value");
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
        let result = synthesize_hooks(&[&spec], HookEmitMode::Bundled).expect("expected value");
        assert_eq!(
            result.entries[0].command,
            "${CLAUDE_PLUGIN_ROOT}/hooks/scripts/_wrappers/pre_tool_use.sh ${CLAUDE_PLUGIN_ROOT}/hooks/scripts/git/pre-commit.sh audit"
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
        let result = synthesize_hooks(&[&spec], HookEmitMode::Bundled).expect("expected value");
        assert_eq!(
            result.entries[0].command,
            "${CLAUDE_PLUGIN_ROOT}/hooks/scripts/_wrappers/session_start.sh ${CLAUDE_PLUGIN_ROOT}/hooks/scripts/init.sh init"
        );
    }

    #[test]
    fn test_synthesize_hooks_does_not_serialize_description() {
        // `HookFrontmatter::description` is documented as informational;
        // neither provider's host runtime consumes it. Lock that contract:
        // a description on the spec must not appear in the emitted JSON.
        let mut spec = make_hook_spec("init", HookEvent::UserPromptSubmit, None);
        spec.frontmatter.description = Some("informational note".to_string());
        let result = synthesize_hooks(&[&spec], HookEmitMode::Bundled).expect("expected value");
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
        let result = synthesize_hooks(&[&spec], HookEmitMode::Bundled).expect("expected value");
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
        let result = synthesize_hooks(&[&a, &b], HookEmitMode::Bundled).expect("expected value");
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
        let spec = make_hook_spec("init", HookEvent::SessionStart, None);
        let result = synthesize_hooks(&[&spec], HookEmitMode::MergedUser).expect("expected ok");
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
        // Phase 3 wraps the command with a per-event shim invocation:
        // `<shim> <user_script>`.
        let spec = make_hook_spec("init", HookEvent::SessionStart, None);
        let result = synthesize_hooks(&[&spec], HookEmitMode::MergedUser).expect("expected ok");
        assert_eq!(
            result.entries[0].command,
            "CLAUDE_PLUGIN_ROOT=$HOME/.claude $HOME/.claude/hooks/scripts/_wrappers/session_start.sh $HOME/.claude/hooks/scripts/init.sh init"
        );
    }

    #[test]
    fn test_synthesize_hooks_merged_project_anchor_includes_plugin_root_assignment() {
        // MergedProject: ${CLAUDE_PROJECT_DIR} anchor + inline CLAUDE_PLUGIN_ROOT.
        // Same Phase 3 shim wrapping as the MergedUser case above.
        let spec = make_hook_spec("init", HookEvent::SessionStart, None);
        let result = synthesize_hooks(&[&spec], HookEmitMode::MergedProject).expect("expected ok");
        assert_eq!(
            result.entries[0].command,
            "CLAUDE_PLUGIN_ROOT=${CLAUDE_PROJECT_DIR}/.claude ${CLAUDE_PROJECT_DIR}/.claude/hooks/scripts/_wrappers/session_start.sh ${CLAUDE_PROJECT_DIR}/.claude/hooks/scripts/init.sh init"
        );
    }

    #[test]
    fn test_model_facing_name_falls_back_to_prefix() {
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
            body: String::new(),
        });

        assert_eq!(
            ClaudeAdapter.body_spec_name(&spec, Some(&cfg)),
            "tw-test-agent"
        );
    }

    #[test]
    fn test_user_dest_dir_agents() {
        // `ClaudeAdapter::compile`'s `dest_root` helper is the only path
        // consumer; verify User-mode resolution via the public Adapter API.
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
        let output = ClaudeAdapter.compile(&[], &ctx).expect("compile");
        assert_eq!(output.dest_root, PathBuf::from("/home/user/.claude"));
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
        assert_eq!(file.provider, Provider::Claude);
        assert_eq!(file.kind, FileKind::PluginManifest);
        assert_eq!(file.path.to_str(), Some(".claude-plugin/plugin.json"));

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
    fn test_build_plugin_manifest_file_name_only_omits_optional_fields() {
        use crate::compile::PluginManifest;

        let manifest = PluginManifest {
            name: "tw".to_string(),
            version: None,
            description: None,
            author: None,
            repository: None,
            license: None,
        };
        let file = build_plugin_manifest_file(&manifest).expect("manifest builds");
        let content = String::from_utf8(file.content.clone()).expect("utf-8");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("valid json");

        // Only `name` should appear; serde_with skips the None fields.
        let obj = parsed.as_object().expect("object");
        assert_eq!(obj.len(), 1, "expected single key (name), got: {content}");
        assert_eq!(obj["name"], "tw");
    }

    #[test]
    fn test_build_plugin_manifest_file_omits_author_email_when_none() {
        use crate::compile::{PluginAuthor, PluginManifest};

        let manifest = PluginManifest {
            name: "tw".to_string(),
            version: None,
            description: None,
            author: Some(PluginAuthor {
                name: "Jason".to_string(),
                email: None,
            }),
            repository: None,
            license: None,
        };
        let file = build_plugin_manifest_file(&manifest).expect("manifest builds");
        let content = String::from_utf8(file.content.clone()).expect("utf-8");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("valid json");
        assert_eq!(parsed["author"]["name"], "Jason");
        assert!(
            parsed["author"].get("email").is_none(),
            "email key should be absent, not null: {content}"
        );
    }

    #[test]
    fn test_compile_emits_plugin_manifest_in_plugin_mode_with_config() {
        use crate::compile::{PluginAuthor, PluginManifest};

        let presets = HashMap::new();
        let home = Path::new("/tmp/home");
        let cwd = Path::new("/tmp/cwd");
        let cfg = AdapterConfig {
            plugin_manifest: Some(PluginManifest {
                name: "tw".to_string(),
                version: Some("1.0.0".to_string()),
                description: None,
                author: Some(PluginAuthor {
                    name: "Author".to_string(),
                    email: Some("author@example.com".to_string()),
                }),
                repository: None,
                license: None,
            }),
            ..AdapterConfig::default()
        };
        let ctx = CompileCtx {
            mode: SyncDestinationMode::Plugin,
            home,
            cwd,
            target_dir: Some(Path::new("/out")),
            presets: &presets,
            adapter_config: Some(&cfg),
            overwrite: false,
        };
        let output = ClaudeAdapter.compile(&[], &ctx).expect("compile");
        let manifest_file = output
            .files
            .iter()
            .find(|f| f.kind == FileKind::PluginManifest)
            .expect("plugin manifest emitted in plugin mode");
        assert_eq!(
            manifest_file.path.to_str(),
            Some(".claude-plugin/plugin.json")
        );
    }

    #[test]
    fn test_compile_does_not_emit_plugin_manifest_in_compile_mode() {
        // Compile mode (the internal `agentspec compile` default) must NOT
        // emit a plugin manifest even if AdapterConfig.plugin_manifest is set,
        // because `compile` produces canonical, provider-config-dir-agnostic
        // output.
        use crate::compile::PluginManifest;

        let presets = HashMap::new();
        let home = Path::new("/tmp/home");
        let cwd = Path::new("/tmp/cwd");
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
            mode: SyncDestinationMode::Compile,
            home,
            cwd,
            target_dir: None,
            presets: &presets,
            adapter_config: Some(&cfg),
            overwrite: false,
        };
        let output = ClaudeAdapter.compile(&[], &ctx).expect("compile");
        assert!(
            output
                .files
                .iter()
                .all(|f| f.kind != FileKind::PluginManifest),
            "Compile mode must not emit plugin manifest"
        );
    }

    #[test]
    fn test_entry_to_claude_json_includes_sentinel() {
        let e = EmittedHookEntry {
            event: HookEvent::SessionStart,
            matcher: None,
            command: "/path/to/script.sh".to_string(),
            timeout: None,
            agentspec_id: "init".to_string(),
        };
        let v = ClaudeAdapter::entry_to_json(&e);
        assert_eq!(v["type"], "command");
        assert_eq!(v["command"], "/path/to/script.sh");
        assert_eq!(v["_agentspec_id"], "init");
        assert!(
            v.get("matcher").is_none(),
            "claude entry: matcher is on the wrapper, not the entry"
        );
    }

    #[test]
    fn test_adapt_rule_without_paths() {
        let spec = Spec::Rule(RuleSpec {
            path: "test.md".into(),
            frontmatter: RuleFrontmatter {
                id: "my-rule".to_string(),
                description: None,
                tags: None,
                paths: None,
            },
            body: "Rule body.".to_string(),
        });

        let files = compile_one(spec, None);
        assert_eq!(files.len(), 1);
        let content = String::from_utf8(files[0].content.clone()).expect("utf8");
        assert!(
            !content.starts_with("---"),
            "rule without paths should have no frontmatter, got: {content}"
        );
        assert!(content.contains("Rule body."), "body should be present");
    }

    #[test]
    fn test_adapt_rule_with_paths() {
        let spec = Spec::Rule(RuleSpec {
            path: "test.md".into(),
            frontmatter: RuleFrontmatter {
                id: "react-rule".to_string(),
                description: None,
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
            content.starts_with("---\n"),
            "rule with paths should start with frontmatter delimiter, got: {content}"
        );
        assert!(
            content.contains("paths:"),
            "frontmatter should contain paths key, got: {content}"
        );
        assert!(
            content.contains("src/components/**/*.tsx"),
            "frontmatter should contain first path, got: {content}"
        );
        assert!(
            content.contains("src/hooks/**/*.ts"),
            "frontmatter should contain second path, got: {content}"
        );
        assert!(
            content.contains("Rule body."),
            "body should follow frontmatter, got: {content}"
        );
    }

    #[test]
    fn test_adapt_rule_with_paths_and_prefix() {
        let cfg = AdapterConfig {
            prefix: Some("tw".to_string()),
            ..AdapterConfig::default()
        };
        let spec = Spec::Rule(RuleSpec {
            path: "test.md".into(),
            frontmatter: RuleFrontmatter {
                id: "react-rule".to_string(),
                description: None,
                tags: None,
                paths: Some(vec!["src/**/*.tsx".to_string()]),
            },
            body: "Rule body.".to_string(),
        });

        let files = compile_one(spec, Some(&cfg));
        assert_eq!(
            files[0].path.to_str(),
            Some("rules/tw-react-rule.md"),
            "prefix should be applied to file path"
        );
        let content = String::from_utf8(files[0].content.clone()).expect("utf8");
        assert!(
            content.contains("src/**/*.tsx"),
            "path glob should appear in frontmatter, got: {content}"
        );
    }
}
