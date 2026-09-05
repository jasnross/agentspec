use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use jsonc_parser::ParseOptions;
use jsonc_parser::cst::{CstInputValue, CstObject, CstRootNode};
use serde::Serialize;
use strum::VariantArray as _;

use super::hooks_helpers::has_agentspec_entries;
use super::{
    Adapter, AdapterOutput, CompileCtx, Degradation, DegradationKind, Delivery, RemovalOutput,
    RemoveCtx, SyncDestinationMode,
};
use crate::compile::{AdapterConfig, GeneratedFile};
use crate::plan::{FileKind, ForwardPatch, RemovePatchReport, ReversePatch};
use crate::presets::ProviderPresetsMap;
use crate::provider::Provider;
use crate::setting::{Carries, SettingKey, SettingKind};
use crate::spec::{AgentSpec, HookEvent, RuleSpec, SkillSpec, Spec, ToolFrontmatter};

// See: https://opencode.ai/docs/agents/#markdown
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
struct OpenCodeAgentFrontmatter {
    description: String,
    mode: &'static str,
    model: Option<String>,
    variant: Option<String>,
    tools: IndexMap<String, bool>,
}

impl Carries for OpenCodeAgentFrontmatter {
    fn carried(&self) -> Vec<SettingKey> {
        // `tools` is unconditional because it is a non-`Option` map that
        // `build_tool_map` populates on every agent file — every canonical
        // tool set to `false`, the declared ones flipped to `true` — so the
        // file always carries a tool map. A spec that declared no tools
        // raises no `Tools` intent, so the extra delivery is inert.
        [
            self.model.as_ref().map(|_| SettingKey::Model),
            self.variant.as_ref().map(|_| SettingKey::Variant),
        ]
        .into_iter()
        .flatten()
        .chain(std::iter::once(SettingKey::Tools))
        .collect()
    }
}

// See: https://opencode.ai/docs/commands/#markdown
// `OpenCode` surfaces a top-level `variant:` key on commands, sibling to `model:`.
// Measured by `experiments/opencode-command-variant/` at opencode 1.18.21.
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
struct OpenCodeCommandFrontmatter {
    description: String,
    model: Option<String>,
    variant: Option<String>,
}

impl Carries for OpenCodeCommandFrontmatter {
    fn carried(&self) -> Vec<SettingKey> {
        [
            self.model.as_ref().map(|_| SettingKey::Model),
            self.variant.as_ref().map(|_| SettingKey::Variant),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

// See: https://opencode.ai/docs/skills/#write-frontmatter
// `model`, `variant`, and `tools` are deliberately absent: `OpenCode` does not
// surface them in its resolved skill record, which resolves to `content`,
// `description`, `location`, and `name` alone.
// Measured by `experiments/opencode-skill-frontmatter-discard/` at opencode 1.18.21.
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
struct OpenCodeSkillFrontmatter {
    name: String,
    description: String,
}

impl Carries for OpenCodeSkillFrontmatter {
    /// Nothing. Per the comment on the struct above, `OpenCode`'s resolved
    /// skill record is `content`, `description`, `location`, and `name`
    /// alone — measured by `experiments/opencode-skill-frontmatter-discard/`
    /// — so this struct has no field a preset or a capability could reach.
    fn carried(&self) -> Vec<SettingKey> {
        Vec::new()
    }
}

/// Filename of `OpenCode`'s host config under each provider's config dir.
const HOST_FILENAME: &str = "opencode.json";

/// Zero-sized adapter for the `OpenCode` provider.
#[derive(Debug)]
pub struct OpenCodeAdapter;

impl Adapter for OpenCodeAdapter {
    fn compile(&self, specs: &[Spec], ctx: &CompileCtx<'_>) -> Result<AdapterOutput> {
        let mut files = Vec::new();
        let mut degradations = Vec::new();
        let mut deliveries = Vec::new();
        for spec in specs {
            match spec {
                Spec::Agent(s) => {
                    let (f, d) = adapt_agent_spec(s.clone(), ctx.presets, ctx.adapter_config)?;
                    files.extend(f);
                    deliveries.extend(d);
                }
                Spec::Skill(s) => {
                    let (f, d) = adapt_skill_spec(s.clone(), ctx.presets, ctx.adapter_config)?;
                    files.extend(f);
                    deliveries.extend(d);
                }
                Spec::Rule(s) => {
                    if s.frontmatter.paths.is_some() && !self.supports_path_scoped_rules() {
                        degradations.push(Degradation::provider_wide(
                            Provider::OpenCode,
                            DegradationKind::PathScopedRulesUnsupported,
                        ));
                    }
                    let (f, d) = adapt_rule_spec(s, ctx.adapter_config);
                    files.extend(f);
                    deliveries.extend(d);
                }
                // Hooks are not emitted for OpenCode in v1. The degradation is
                // pushed here — the arm that drops the spec — rather than
                // rediscovered by a post-loop scan in `compile_specs`.
                Spec::Hook(_) if !self.emits_hooks() => degradations.push(Degradation::for_spec(
                    Provider::OpenCode,
                    spec.id(),
                    DegradationKind::HooksUnsupported,
                )),
                // A guarded arm does not contribute to exhaustiveness, and
                // `wildcard_enum_match_arm` is denied — so the unguarded arm
                // stays. It is reachable only if `emits_hooks()` flips to
                // `true` while this adapter still emits no hook files, which
                // would drop the spec with neither output nor a degradation.
                // Defense-in-depth per `.claude/rules/validation-locality.md`.
                Spec::Hook(_) => debug_assert!(
                    !self.emits_hooks(),
                    "OpenCode reports emits_hooks() but has no hook emission path"
                ),
            }
        }

        let dest_root = config_dir(ctx.mode, ctx.target_dir, ctx.home, ctx.cwd);
        // `FileKind::Rules` is statically known here; `dir_for_kind` only
        // ever returns `None` for `PluginManifest` on providers without a
        // plugin concept. The `unwrap_or` is a lint-safe fallback that
        // matches the central registry — see `Adapter::dir_for_kind`.
        let rules_dest_dir = dest_root.join(self.dir_for_kind(FileKind::Rules).unwrap_or("rules"));

        // Eagerly compute the instructions[] entries from the freshly-emitted
        // rule files rather than deferring to a hook-run-time WalkDir. Path
        // shape: each rule lands at `<rules_dest_dir>/<id>/AGENTS.md` (set by
        // `adapt_rule_spec`); the absolute path is `<rules_dest_dir>/<rel>`.
        let mut instruction_paths: Vec<String> = files
            .iter()
            .filter(|f| {
                f.kind == FileKind::Rules && f.path.file_name() == Some(OsStr::new("AGENTS.md"))
            })
            .map(|f| {
                // `f.path` is relative; `f.path` already has the leading
                // `rules/<id>/AGENTS.md` shape, so anchor under `dest_root`.
                dest_root.join(&f.path).to_string_lossy().into_owned()
            })
            .collect();
        instruction_paths.sort();

        // Always construct the patch — even with zero rule instructions, its
        // `run` strips orphaned `_agentspec_id`-tagged entries left over from
        // a prior sync. Pre-branch behavior (`post_write_hook` called per
        // `(provider, FileKind::Rules)` regardless of file count) ran this
        // cleanup on every sync; the patch's `run` short-circuits at line 598
        // when both the host file is absent AND `new_paths` is empty.
        let patches: Vec<Box<dyn ForwardPatch>> = vec![Box::new(OpenCodeInstructionsPatch {
            rules_dest_dir,
            host_path: dest_root.join(HOST_FILENAME),
            instruction_paths,
        })];

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
        let rules_dest_dir = dest_root.join(self.dir_for_kind(FileKind::Rules).unwrap_or("rules"));
        let patches: Vec<Box<dyn ReversePatch>> = vec![Box::new(OpenCodeRemoveInstructionsPatch {
            rules_dest_dir,
            host_path: dest_root.join(HOST_FILENAME),
        })];
        RemovalOutput { patches, dest_root }
    }

    fn prune_patches(&self, home: &Path, cwd: &Path) -> Vec<Box<dyn ReversePatch>> {
        let rules_dir_name = self.dir_for_kind(FileKind::Rules).unwrap_or("rules");
        let candidates = [
            (
                home.join(".config/opencode"),
                home.join(".config/opencode").join(rules_dir_name),
            ),
            (
                cwd.join(".opencode"),
                cwd.join(".opencode").join(rules_dir_name),
            ),
        ];
        candidates
            .into_iter()
            .filter(|(dest_root, _)| has_agentspec_entries(&dest_root.join(HOST_FILENAME)))
            .map(|(dest_root, rules_dest_dir)| -> Box<dyn ReversePatch> {
                Box::new(OpenCodeRemoveInstructionsPatch {
                    rules_dest_dir,
                    host_path: dest_root.join(HOST_FILENAME),
                })
            })
            .collect()
    }

    /// Resolve a canonical tool to the name an `OpenCode` spec body (or
    /// frontmatter tool map) should reference.
    fn body_tool_name(&self, tool: &ToolFrontmatter) -> &'static str {
        match tool {
            ToolFrontmatter::Read => "read",
            ToolFrontmatter::Write => "write",
            ToolFrontmatter::Edit => "edit",
            ToolFrontmatter::Grep => "grep",
            ToolFrontmatter::Glob => "glob",
            ToolFrontmatter::Shell => "bash",
            ToolFrontmatter::WebFetch => "webfetch",
            ToolFrontmatter::WebSearch => "websearch",
            ToolFrontmatter::Question => "question",
            ToolFrontmatter::Tasks => "todowrite",
            ToolFrontmatter::Subagent => "task",
            ToolFrontmatter::Skill => "skill",
        }
    }

    /// Returns the name which should be used to refer to the spec in the generated body content.
    ///
    /// - **Agents**: the model-facing name is prefixed via `content_prefix()`,
    ///   which may differ from the file-path prefix.
    /// - **Skills**: the frontmatter `name` field uses the unprefixed canonical ID
    ///   (the prefix only appears in the directory path). User-invocable skills
    ///   (commands) are also derived from `Spec::Skill` — there is no
    ///   separate `Command` variant — and follow the same unprefixed convention.
    /// - **Rules**: have no model-facing name (auto-loaded content). Returns the
    ///   canonical ID as a best-effort fallback; spec authors should not typically
    ///   reference rules by name.
    fn body_spec_name(&self, spec: &Spec, cfg: Option<&AdapterConfig>) -> String {
        let id = spec.id();
        match spec {
            Spec::Agent(_) => match cfg.and_then(AdapterConfig::content_prefix) {
                Some(prefix) => format!("{prefix}{id}"),
                None => id.to_owned(),
            },
            Spec::Skill(_) | Spec::Rule(_) | Spec::Hook(_) => id.to_owned(),
        }
    }

    fn body_skill_root(&self) -> Option<&'static str> {
        None
    }

    fn carriable(&self, kind: FileKind) -> &'static [SettingKind] {
        match kind {
            FileKind::Agents => &[
                SettingKind::Body,
                SettingKind::Model,
                SettingKind::Variant,
                SettingKind::Tools,
            ],
            FileKind::Commands => &[SettingKind::Body, SettingKind::Model, SettingKind::Variant],
            FileKind::Skills | FileKind::Rules => &[SettingKind::Body],
            FileKind::Hooks | FileKind::PluginManifest => &[],
        }
    }

    fn emits_hooks(&self) -> bool {
        false
    }

    /// Unreachable rather than meaningful: `emits_hooks` is `false` for
    /// `OpenCode`, and the only caller (`hook test`) dispatches through
    /// `ProviderName`, which has no `OpenCode` variant.
    fn hook_command_preview(
        &self,
        _event: HookEvent,
        _script: &Path,
        _hook_id: &str,
        _args: &[String],
    ) -> String {
        String::new()
    }

    fn plugin_manifest_dir(&self) -> Option<&'static str> {
        None
    }

    fn supports_path_scoped_rules(&self) -> bool {
        false
    }
}

fn config_dir(
    mode: SyncDestinationMode,
    target_dir: Option<&Path>,
    home: &Path,
    cwd: &Path,
) -> PathBuf {
    super::resolve_config_dir(
        mode,
        target_dir,
        home,
        cwd,
        Path::new(".config/opencode"),
        Path::new(".opencode"),
    )
}

fn adapt_agent_spec(
    spec: AgentSpec,
    presets: &ProviderPresetsMap,
    cfg: Option<&AdapterConfig>,
) -> Result<(Vec<GeneratedFile>, Vec<Delivery>)> {
    let id = spec.frontmatter.id;
    let description = spec.frontmatter.description;

    let preset = spec
        .frontmatter
        .execution
        .and_then(|x| x.preset)
        .and_then(|x| presets.get(&x))
        .and_then(|x| x.opencode.clone());
    let model = preset.as_ref().and_then(|x| x.model.clone());
    let variant = preset.as_ref().and_then(|x| x.variant.clone());

    let tools: Vec<ToolFrontmatter> = spec
        .frontmatter
        .capabilities
        .and_then(|x| x.tools)
        .into_iter()
        .flatten()
        .collect();

    let tools = build_tool_map(&tools);

    let frontmatter = OpenCodeAgentFrontmatter {
        description,
        mode: "subagent",
        model,
        variant,
        tools,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body;
    let content = format!("---\n{frontmatter_str}---\n\n{}", body.trim());

    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();

    let file = GeneratedFile::text(
        Provider::OpenCode,
        FileKind::Agents,
        Path::new("agents").join(format!("{file_prefix}{id}.md")),
        content,
    )
    .with_spec_id(&id);
    let deliveries = Delivery::from_file(&id, &file, frontmatter.carried());
    Ok((vec![file], deliveries))
}

fn adapt_skill_spec(
    spec: SkillSpec,
    presets: &ProviderPresetsMap,
    cfg: Option<&AdapterConfig>,
) -> Result<(Vec<GeneratedFile>, Vec<Delivery>)> {
    let id = spec.frontmatter.id;
    let description = spec.frontmatter.description.unwrap_or_default();
    let user_invocable = spec.frontmatter.user_invocable;
    let agent_invocable = spec.frontmatter.agent_invocable;

    let preset = spec
        .frontmatter
        .execution
        .and_then(|x| x.preset)
        .and_then(|x| presets.get(&x))
        .and_then(|x| x.opencode.clone());
    let model = preset.as_ref().and_then(|x| x.model.clone());
    let variant = preset.as_ref().and_then(|x| x.variant.clone());

    let body = spec.body;
    let supporting_files = spec.supporting_files;

    let mut files = Vec::new();
    // Deliveries are recorded only for the branches actually taken: the
    // command file's `model`/`variant` when `user_invocable`, and nothing
    // extra when `agent_invocable`, because the skill file carries neither.
    // A dual-invocable skill therefore reports a `Skills`-kind loss for both
    // while its `Commands` file satisfies its own intent — the shape the
    // per-emitted-kind subtraction exists to express. Do not flatten it.
    let mut deliveries = Vec::new();

    if user_invocable {
        // OpenCode commands: prefix becomes a subdirectory, not a file prefix
        let cmd_path = match cfg.and_then(|c| c.prefix.as_deref()) {
            Some(prefix) => Path::new("commands").join(prefix).join(format!("{id}.md")),
            None => Path::new("commands").join(format!("{id}.md")),
        };

        let frontmatter = OpenCodeCommandFrontmatter {
            description: description.clone(),
            model,
            variant,
        };
        let frontmatter_str = serde_yml::to_string(&frontmatter)?;
        let content = format!("---\n{frontmatter_str}---\n\n{}", body.trim());
        let file = GeneratedFile::text(Provider::OpenCode, FileKind::Commands, cmd_path, content)
            .with_spec_id(&id);
        deliveries.extend(Delivery::from_file(&id, &file, frontmatter.carried()));
        files.push(file);
    }

    if agent_invocable {
        let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();

        let frontmatter = OpenCodeSkillFrontmatter {
            name: id.clone(),
            description,
        };
        let frontmatter_str = serde_yml::to_string(&frontmatter)?;
        let content = format!("---\n{frontmatter_str}---\n\n{}", body.trim());

        let skill_dir = Path::new("skills").join(format!("{file_prefix}{id}"));

        let skill_file = GeneratedFile::text(
            Provider::OpenCode,
            FileKind::Skills,
            skill_dir.join("SKILL.md"),
            content,
        )
        .with_spec_id(&id);
        deliveries.extend(Delivery::from_file(&id, &skill_file, frontmatter.carried()));
        files.push(skill_file);

        // Supporting files carry no settings, but still name their spec:
        // `Body` membership is read off `GeneratedFile.spec_id`.
        for (rel_path, sf) in supporting_files {
            files.push(
                GeneratedFile::binary(
                    Provider::OpenCode,
                    FileKind::Skills,
                    skill_dir.join(&rel_path),
                    sf.content,
                    Some(sf.mode),
                )
                .with_spec_id(&id),
            );
        }
    }

    Ok((files, deliveries))
}

fn adapt_rule_spec(
    spec: &RuleSpec,
    cfg: Option<&AdapterConfig>,
) -> (Vec<GeneratedFile>, Vec<Delivery>) {
    let id = &spec.frontmatter.id;
    let content = format!("{}\n", spec.body.trim());
    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let path = Path::new("rules")
        .join(format!("{file_prefix}{id}"))
        .join("AGENTS.md");

    // `OpenCode` rule files carry no frontmatter at all — the whole file is
    // the body — so there is no struct to read a record off and nothing but
    // the body is delivered.
    let file =
        GeneratedFile::text(Provider::OpenCode, FileKind::Rules, path, content).with_spec_id(id);
    (vec![file], Vec::new())
}

/// Post-write patch that registers agentspec rule files in `opencode.json`'s
/// `instructions[]`.
///
/// The patch carries the eager-computed list of instruction paths
/// (constructed from the rule-spec `GeneratedFile`s during compile) — no
/// runtime `WalkDir`. This means user-authored `AGENTS.md` files placed
/// inside agentspec's rules dest dir are no longer picked up; manifest-only
/// ownership.
#[derive(Debug)]
pub(crate) struct OpenCodeInstructionsPatch {
    rules_dest_dir: PathBuf,
    host_path: PathBuf,
    instruction_paths: Vec<String>,
}

impl ForwardPatch for OpenCodeInstructionsPatch {
    fn run(&self, dry_run: bool) -> Result<()> {
        patch_opencode_instructions(
            &self.rules_dest_dir,
            &self.host_path,
            &self.instruction_paths,
            dry_run,
        )
    }
}

/// Reverse-direction `instructions[]` filter: strips entries whose path
/// starts with `rules_dest_dir`. If `instructions[]` becomes empty the key
/// is dropped; if the residual file is then `{}` AND tidy actually removed
/// at least one agentspec entry, the host file is deleted and its parent
/// directory best-effort `rmdir`'d.
#[derive(Debug)]
pub(crate) struct OpenCodeRemoveInstructionsPatch {
    rules_dest_dir: PathBuf,
    host_path: PathBuf,
}

impl ReversePatch for OpenCodeRemoveInstructionsPatch {
    fn run_remove(&self, dry_run: bool) -> Result<()> {
        let report = remove_opencode_instructions(&self.rules_dest_dir, &self.host_path, dry_run)?;
        report.print_summary(dry_run);
        Ok(())
    }
}

/// Reverses `patch_opencode_instructions`'s effect on
/// `<config_dir>/opencode.json`.
///
/// Drops every `instructions[]` entry whose path starts with
/// `rules_dest_dir`; if the array becomes empty, the `instructions` key is
/// removed entirely. Returns the count of surviving user-authored entries
/// for `RemovePatchReport::print_summary`.
///
/// The host file is **deleted** when (a) tidy actually removed at least one
/// agentspec instruction entry, and (b) no other top-level keys survive.
/// After a delete, the host file's parent directory is best-effort
/// `rmdir`'d. Any user-authored top-level keys (e.g. `model`) keep the
/// file alive. `OpenCode`'s `opencode.json` doesn't use a `version` key, so
/// there's no version carve-out — that's Cursor-specific.
///
/// Trivia preservation: parses, mutates, and writes via `jsonc-parser`'s CST
/// so user-authored comments, key ordering, trailing commas, and formatting
/// whitespace round-trip across remove cycles.
fn remove_opencode_instructions(
    rules_dest_dir: &Path,
    host_path: &Path,
    dry_run: bool,
) -> Result<RemovePatchReport> {
    if !host_path.exists() {
        return Ok(RemovePatchReport::default());
    }

    let content = crate::cst_io::read_or_empty_object(host_path)?;
    let root = CstRootNode::parse(&content, &ParseOptions::default())
        .with_context(|| format!("failed to parse {}", host_path.display()))?;

    let Some(top) = root.object_value_or_create() else {
        let prefix = if dry_run { "[dry-run] " } else { "" };
        eprintln!(
            "{prefix}warning: {} has a non-object root; skipping tidy",
            host_path.display()
        );
        return Ok(RemovePatchReport {
            host_path: host_path.to_path_buf(),
            user_entries_remaining: 0,
            host_file_deleted: false,
            parent_rmdir: false,
        });
    };

    let TidyResult {
        agentspec_removed,
        user_entries_remaining,
    } = tidy_instructions(&top, rules_dest_dir);

    // No-op short-circuit: if no agentspec-owned entries were removed, skip
    // the rewrite to avoid bumping mtime on what is functionally a read-only
    // cycle. This branch also doubles as the `removed_owned > 0` guard for
    // the delete-on-empty predicate below.
    if agentspec_removed == 0 {
        return Ok(RemovePatchReport {
            host_path: host_path.to_path_buf(),
            user_entries_remaining,
            host_file_deleted: false,
            parent_rmdir: false,
        });
    }

    if top.properties().is_empty() {
        let parent_rmdir = crate::plan::delete_host_file_and_rmdir_parent(host_path, dry_run)?;
        return Ok(RemovePatchReport {
            host_path: host_path.to_path_buf(),
            user_entries_remaining: 0,
            host_file_deleted: true,
            parent_rmdir,
        });
    }

    if dry_run {
        eprintln!(
            "[dry-run] would tidy {agentspec_removed} agentspec instruction(s) from {}",
            host_path.display()
        );
        return Ok(RemovePatchReport {
            host_path: host_path.to_path_buf(),
            user_entries_remaining,
            host_file_deleted: false,
            parent_rmdir: false,
        });
    }

    crate::cst_io::finish(&root, host_path)?;

    Ok(RemovePatchReport {
        host_path: host_path.to_path_buf(),
        user_entries_remaining,
        host_file_deleted: false,
        parent_rmdir: false,
    })
}

struct TidyResult {
    agentspec_removed: usize,
    user_entries_remaining: usize,
}

/// Drop agentspec-owned string entries from `instructions[]`, preserving
/// user-authored strings and any non-string elements verbatim. If the
/// resulting array is empty, drop the `instructions` key entirely.
fn tidy_instructions(top: &CstObject, rules_dest_dir: &Path) -> TidyResult {
    let Some(arr) = top.array_value("instructions") else {
        return TidyResult {
            agentspec_removed: 0,
            user_entries_remaining: 0,
        };
    };

    let mut agentspec_removed = 0usize;
    let mut user_entries_remaining = 0usize;
    for entry in arr.elements() {
        match entry.as_string_lit().and_then(|s| s.decoded_value().ok()) {
            Some(p) if is_agentspec_instruction(&p, rules_dest_dir) => {
                entry.remove();
                agentspec_removed += 1;
            }
            // Defensive: a non-string element (malformed user file) is kept
            // verbatim and counted as a surviving user entry.
            _ => user_entries_remaining += 1,
        }
    }

    if arr.elements().is_empty()
        && let Some(prop) = top.get("instructions")
    {
        prop.remove();
    }

    TidyResult {
        agentspec_removed,
        user_entries_remaining,
    }
}

/// Build the boolean tool map used by `OpenCode` agents and agent-invocable skills.
///
/// Initializes all `ToolFrontmatter`-expressible `OpenCode` tools to false, then enables
/// the ones listed in the spec. User-facing `OpenCode` tools outside this set
/// (`apply_patch`, `lsp`) are omitted and fall back to `OpenCode`'s default behavior
/// (enabled when not explicitly disabled).
fn build_tool_map(tools: &[ToolFrontmatter]) -> IndexMap<String, bool> {
    let mut map: IndexMap<String, bool> = ToolFrontmatter::VARIANTS
        .iter()
        .map(|t| (OpenCodeAdapter.body_tool_name(t).to_string(), false))
        .collect();

    for tool in tools {
        map.insert(OpenCodeAdapter.body_tool_name(tool).to_string(), true);
    }

    map.sort_keys();

    map
}

/// Shared ownership predicate: returns `true` if `entry_path` (a string from
/// `opencode.json`'s `instructions[]`) belongs to agentspec.
///
/// Used by both [`patch_opencode_instructions`] (sync, write side) and
/// [`remove_opencode_instructions`] (read side) so any future change to path
/// representation must update both call sites at once.
fn is_agentspec_instruction(entry_path: &str, rules_dest_dir: &Path) -> bool {
    Path::new(entry_path).starts_with(rules_dest_dir)
}

/// Patches the `instructions` array in `config_dir/opencode.json` from the
/// pre-computed `new_paths` list.
///
/// Ownership contract: agentspec owns any entry whose path falls under
/// `rules_dest_dir`. On each sync those entries are replaced wholesale; all
/// other entries are preserved.
///
/// If `opencode.json` does not exist, it is created with just the
/// `instructions` key.
///
/// When `dry_run` is true, prints the planned diff but does not write the
/// file.
///
/// Trivia preservation: parses, mutates, and writes via `jsonc-parser`'s CST
/// so user-authored comments, key ordering, trailing commas, and formatting
/// whitespace round-trip across sync cycles.
fn patch_opencode_instructions(
    rules_dest_dir: &Path,
    host_path: &Path,
    new_paths: &[String],
    dry_run: bool,
) -> Result<()> {
    // Skip writing entirely when the file doesn't exist yet and there's nothing
    // to record. Avoids creating a spurious `opencode.json` when no rules have
    // ever been synced.
    if !host_path.exists() && new_paths.is_empty() {
        return Ok(());
    }

    let content = crate::cst_io::read_or_empty_object(host_path)?;
    let root = CstRootNode::parse(&content, &ParseOptions::default())
        .with_context(|| format!("failed to parse {}", host_path.display()))?;

    let Some(top) = root.object_value_or_create() else {
        // Behavior change vs. the prior serde_json implementation: that one
        // silently wrote the unmodified non-object value back. Aligning with
        // the remove path's existing warn-and-no-op contract here.
        let prefix = if dry_run { "[dry-run] " } else { "" };
        eprintln!(
            "{prefix}warning: {} has a non-object root; skipping patch",
            host_path.display()
        );
        return Ok(());
    };

    if dry_run {
        eprintln!(
            "[dry-run] would write {} instructions to {}",
            new_paths.len(),
            host_path.display()
        );
        return Ok(());
    }

    rewrite_instructions(&top, rules_dest_dir, new_paths);

    crate::cst_io::finish(&root, host_path)
}

/// Drop agentspec-owned string entries from `instructions[]`, preserving
/// user-authored strings and any non-string elements verbatim. Append
/// `new_paths` as fresh string entries. If `instructions[]` is absent, insert
/// it as a new property when `new_paths` is non-empty.
fn rewrite_instructions(top: &CstObject, rules_dest_dir: &Path, new_paths: &[String]) {
    if let Some(arr) = top.array_value("instructions") {
        for entry in arr.elements() {
            let is_owned = entry
                .as_string_lit()
                .and_then(|s| s.decoded_value().ok())
                .is_some_and(|p| is_agentspec_instruction(&p, rules_dest_dir));
            if is_owned {
                entry.remove();
            }
        }
        for path in new_paths {
            arr.append(CstInputValue::String(path.clone()));
        }
        if arr.elements().is_empty()
            && let Some(prop) = top.get("instructions")
        {
            prop.remove();
        }
    } else if !new_paths.is_empty() {
        let arr = top.array_value_or_set("instructions");
        for path in new_paths {
            arr.append(CstInputValue::String(path.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use super::*;
    use crate::presets::{OpenCodePreset, ProviderPresets};
    use crate::spec::{
        AgentFrontmatter, AgentSpec, CapabilitiesFrontmatter, ExecutionFrontmatter,
        SkillFrontmatter, SkillSpec,
    };

    /// `tools` is a non-`Option` map that `build_tool_map` populates on every
    /// agent file, so the file carries a tool map whether or not the spec
    /// declared one. Pinned so the behavior is a decision rather than an
    /// accident of the field's type.
    #[test]
    fn test_agent_frontmatter_carries_tools_with_none_declared() {
        let frontmatter = OpenCodeAgentFrontmatter {
            description: "d".to_owned(),
            mode: "subagent",
            model: None,
            variant: None,
            tools: build_tool_map(&[]),
        };
        assert_eq!(frontmatter.carried(), vec![SettingKey::Tools]);
    }

    #[test]
    fn test_agent_frontmatter_carries_model_and_variant_when_set() {
        let frontmatter = OpenCodeAgentFrontmatter {
            description: "d".to_owned(),
            mode: "subagent",
            model: Some("anthropic/claude-opus-5".to_owned()),
            variant: Some("thinking".to_owned()),
            tools: build_tool_map(&[]),
        };
        assert_eq!(
            frontmatter.carried(),
            vec![SettingKey::Model, SettingKey::Variant, SettingKey::Tools]
        );
    }

    #[test]
    fn test_skill_frontmatter_carries_nothing() {
        let frontmatter = OpenCodeSkillFrontmatter {
            name: "s".to_owned(),
            description: "d".to_owned(),
        };
        assert!(frontmatter.carried().is_empty());
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
        OpenCodeAdapter
            .compile(&[spec], &ctx)
            .expect("compile")
            .files
    }

    fn compile_one(spec: Spec, cfg: Option<&AdapterConfig>) -> Vec<GeneratedFile> {
        compile_one_with_presets(spec, cfg, &HashMap::new())
    }

    /// A single-entry presets map whose `OpenCode` half sets both `model` and
    /// `variant`, so an emitted file proves where each key lands.
    fn presets_with_model_and_variant() -> ProviderPresetsMap {
        HashMap::from([(
            "default".to_string(),
            ProviderPresets {
                claude: None,
                cursor: None,
                opencode: Some(OpenCodePreset {
                    model: Some("anthropic/claude-sonnet-4-5".to_string()),
                    variant: Some("high".to_string()),
                }),
            },
        )])
    }

    #[test]
    fn test_body_tool_name_tasks_maps_to_todowrite() {
        assert_eq!(
            OpenCodeAdapter.body_tool_name(&ToolFrontmatter::Tasks),
            "todowrite"
        );
    }

    #[test]
    fn test_body_tool_name_subagent_maps_to_task() {
        assert_eq!(
            OpenCodeAdapter.body_tool_name(&ToolFrontmatter::Subagent),
            "task"
        );
    }

    #[test]
    fn test_body_tool_name_skill_identity() {
        assert_eq!(
            OpenCodeAdapter.body_tool_name(&ToolFrontmatter::Skill),
            "skill"
        );
    }

    #[test]
    fn test_build_tool_map_keys_are_sorted() {
        let tools = &[ToolFrontmatter::Write, ToolFrontmatter::Read];
        let map = build_tool_map(tools);
        let keys: Vec<&str> = map.keys().map(String::as_str).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(
            keys, sorted,
            "tool map keys should be in alphabetical order"
        );
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
            "description: Test agent\n",
            "mode: subagent\n",
            "tools:\n",
            "  bash: false\n",
            "  edit: false\n",
            "  glob: false\n",
            "  grep: false\n",
            "  question: false\n",
            "  read: false\n",
            "  skill: false\n",
            "  task: false\n",
            "  todowrite: false\n",
            "  webfetch: false\n",
            "  websearch: false\n",
            "  write: false\n",
            "---\n",
            "\n",
            "Body.",
        );
        assert_eq!(content, expected);
    }

    /// `OpenCode` surfaces `variant:` on agents as well as on commands. The
    /// preset-free `test_adapt_agent_output_format` above pins the key's
    /// *absence*, so without this test nothing here asserts the agent surface
    /// carries it at all.
    #[test]
    fn test_adapt_agent_output_format_includes_variant() {
        let spec = Spec::Agent(AgentSpec {
            path: "test.md".into(),
            frontmatter: AgentFrontmatter {
                id: "preset-agent".to_string(),
                description: "An agent with a preset".to_string(),
                tags: None,
                execution: Some(ExecutionFrontmatter {
                    preset: Some("default".to_string()),
                }),
                capabilities: None,
            },
            body: "Body.".to_string(),
        });

        let files = compile_one_with_presets(spec, None, &presets_with_model_and_variant());
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");

        let expected = concat!(
            "---\n",
            "description: An agent with a preset\n",
            "mode: subagent\n",
            "model: anthropic/claude-sonnet-4-5\n",
            "variant: high\n",
            "tools:\n",
            "  bash: false\n",
            "  edit: false\n",
            "  glob: false\n",
            "  grep: false\n",
            "  question: false\n",
            "  read: false\n",
            "  skill: false\n",
            "  task: false\n",
            "  todowrite: false\n",
            "  webfetch: false\n",
            "  websearch: false\n",
            "  write: false\n",
            "---\n",
            "\n",
            "Body.",
        );
        assert_eq!(content, expected);
    }

    /// Asserts the whole emitted block rather than substrings, so field order
    /// is pinned alongside presence.
    #[test]
    fn test_adapt_command_output_format_includes_variant() {
        let spec = Spec::Skill(SkillSpec {
            path: "test.md".into(),
            frontmatter: SkillFrontmatter {
                id: "preset-skill".to_string(),
                description: Some("A skill with a preset".to_string()),
                tags: None,
                execution: Some(ExecutionFrontmatter {
                    preset: Some("default".to_string()),
                }),
                capabilities: None,
                user_invocable: true,
                agent_invocable: false,
            },
            body: "Body.".to_string(),
            supporting_files: IndexMap::new(),
        });

        let files = compile_one_with_presets(spec, None, &presets_with_model_and_variant());
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");

        let expected = concat!(
            "---\n",
            "description: A skill with a preset\n",
            "model: anthropic/claude-sonnet-4-5\n",
            "variant: high\n",
            "---\n",
            "\n",
            "Body.",
        );
        assert_eq!(content, expected);
    }

    /// A spec naming no preset at all: `skip_serializing_none` must elide both
    /// optional keys rather than emitting them as null.
    ///
    /// The spec carries `execution: None` rather than a preset name absent from
    /// the map, because `validate_semantics` rejects the latter with `unknown
    /// preset` — that pairing never reaches an adapter in the real pipeline.
    #[test]
    fn test_adapt_command_output_omits_variant_without_preset() {
        let spec = Spec::Skill(SkillSpec {
            path: "test.md".into(),
            frontmatter: SkillFrontmatter {
                id: "presetless-skill".to_string(),
                description: Some("A skill with no preset".to_string()),
                tags: None,
                execution: None,
                capabilities: None,
                user_invocable: true,
                agent_invocable: false,
            },
            body: "Body.".to_string(),
            supporting_files: IndexMap::new(),
        });

        let files = compile_one(spec, None);
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");

        let expected = concat!(
            "---\n",
            "description: A skill with no preset\n",
            "---\n",
            "\n",
            "Body.",
        );
        assert_eq!(content, expected);
    }

    /// The closest neighbor to the drop this correction fixes: a preset that
    /// resolves, but whose `OpenCode` half sets `model` with no `variant`. The
    /// `model:` line proves the preset was found, so the missing `variant:` is
    /// elision rather than a lookup miss.
    #[test]
    fn test_adapt_command_output_omits_variant_when_preset_sets_only_model() {
        let presets = HashMap::from([(
            "default".to_string(),
            ProviderPresets {
                claude: None,
                cursor: None,
                opencode: Some(OpenCodePreset {
                    model: Some("anthropic/claude-sonnet-4-5".to_string()),
                    variant: None,
                }),
            },
        )]);

        let spec = Spec::Skill(SkillSpec {
            path: "test.md".into(),
            frontmatter: SkillFrontmatter {
                id: "preset-skill".to_string(),
                description: Some("A skill with a model-only preset".to_string()),
                tags: None,
                execution: Some(ExecutionFrontmatter {
                    preset: Some("default".to_string()),
                }),
                capabilities: None,
                user_invocable: true,
                agent_invocable: false,
            },
            body: "Body.".to_string(),
            supporting_files: IndexMap::new(),
        });

        let files = compile_one_with_presets(spec, None, &presets);
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");

        let expected = concat!(
            "---\n",
            "description: A skill with a model-only preset\n",
            "model: anthropic/claude-sonnet-4-5\n",
            "---\n",
            "\n",
            "Body.",
        );
        assert_eq!(content, expected);
    }

    /// The preset resolves with both `model` and `variant`, and the spec
    /// declares `capabilities.tools`, so all three discarded keys had values
    /// available to emit. The full-block assertion is what pins the field set
    /// down to `name` and `description`.
    #[test]
    fn test_adapt_skill_output_format_omits_discarded_keys() {
        let spec = Spec::Skill(SkillSpec {
            path: "test.md".into(),
            frontmatter: SkillFrontmatter {
                id: "preset-skill".to_string(),
                description: Some("A skill with a preset".to_string()),
                tags: None,
                execution: Some(ExecutionFrontmatter {
                    preset: Some("default".to_string()),
                }),
                capabilities: Some(CapabilitiesFrontmatter {
                    tools: Some(vec![ToolFrontmatter::Read, ToolFrontmatter::Grep]),
                }),
                user_invocable: false,
                agent_invocable: true,
            },
            body: "Body.".to_string(),
            supporting_files: IndexMap::new(),
        });

        let files = compile_one_with_presets(spec, None, &presets_with_model_and_variant());
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");

        let expected = concat!(
            "---\n",
            "name: preset-skill\n",
            "description: A skill with a preset\n",
            "---\n",
            "\n",
            "Body.",
        );
        assert_eq!(content, expected);
    }

    #[test]
    fn test_adapt_skill_command_with_prefix_uses_subdirectory() {
        let cfg = AdapterConfig {
            prefix: Some("tw".to_string()),
            ..AdapterConfig::default()
        };
        let spec = Spec::Skill(SkillSpec {
            path: "test.md".into(),
            frontmatter: SkillFrontmatter {
                id: "basic-skill".to_string(),
                description: Some("A basic skill".to_string()),
                tags: None,
                execution: None,
                capabilities: None,
                user_invocable: true,
                agent_invocable: false,
            },
            body: "Body.".to_string(),
            supporting_files: IndexMap::new(),
        });

        let files = compile_one(spec, Some(&cfg));
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].path.to_str(),
            Some("commands/tw/basic-skill.md"),
            "OpenCode commands should use prefix as subdirectory"
        );
    }

    // ── patch_opencode_instructions tests ───────────────────────────────────

    /// Test helper: discover existing AGENTS.md paths under `rules_dest_dir`.
    /// Used to drive `patch_opencode_instructions` directly, mirroring the
    /// path set the production `OpenCodeAdapter::compile` would have built
    /// from its `GeneratedFile`s.
    fn discover_rules(rules_dest_dir: &Path) -> Vec<String> {
        let mut paths: Vec<String> = if rules_dest_dir.is_dir() {
            walkdir::WalkDir::new(rules_dest_dir)
                .min_depth(1)
                .follow_links(true)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_file() && e.file_name() == "AGENTS.md")
                .map(|e| e.path().to_string_lossy().into_owned())
                .collect()
        } else {
            Vec::new()
        };
        paths.sort();
        paths
    }

    #[test]
    fn test_patch_no_prior_config_creates_file() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir_all(rules_dir.join("my-rule")).expect("expected value");
        fs::write(rules_dir.join("my-rule/AGENTS.md"), "rule").expect("expected value");

        let paths = discover_rules(&rules_dir);
        patch_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), &paths, false)
            .expect("expected value");

        let config_path = tmp.path().join("opencode.json");
        assert!(config_path.exists());
        let content = fs::read_to_string(&config_path).expect("expected value");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("expected value");
        let instructions = parsed["instructions"].as_array().expect("expected array");
        assert_eq!(instructions.len(), 1);
        assert!(
            instructions[0]
                .as_str()
                .expect("expected str")
                .contains("my-rule")
        );
    }

    #[test]
    fn test_patch_preserves_user_entries() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir_all(rules_dir.join("my-rule")).expect("expected value");
        fs::write(rules_dir.join("my-rule/AGENTS.md"), "rule").expect("expected value");

        let config_path = tmp.path().join("opencode.json");
        fs::write(
            &config_path,
            r#"{"instructions": ["/user/custom/AGENTS.md"]}"#,
        )
        .expect("expected value");

        let paths = discover_rules(&rules_dir);
        patch_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), &paths, false)
            .expect("expected value");

        let content = fs::read_to_string(&config_path).expect("expected value");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("expected value");
        let instructions = parsed["instructions"].as_array().expect("expected array");
        let paths: Vec<&str> = instructions
            .iter()
            .map(|v| v.as_str().expect("expected str"))
            .collect();
        assert!(
            paths.contains(&"/user/custom/AGENTS.md"),
            "user entry preserved"
        );
        assert!(
            paths.iter().any(|p| p.contains("my-rule")),
            "agentspec entry added"
        );
    }

    #[test]
    fn test_patch_replaces_stale_agentspec_entries() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir_all(rules_dir.join("new-rule")).expect("expected value");
        fs::write(rules_dir.join("new-rule/AGENTS.md"), "rule").expect("expected value");

        let config_path = tmp.path().join("opencode.json");
        let stale_path = rules_dir.join("old-rule/AGENTS.md");
        let existing = serde_json::json!({
            "instructions": [
                stale_path.to_string_lossy(),
                "/user/AGENTS.md"
            ]
        });
        fs::write(
            &config_path,
            serde_json::to_string(&existing).expect("expected value"),
        )
        .expect("expected value");

        let paths = discover_rules(&rules_dir);
        patch_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), &paths, false)
            .expect("expected value");

        let content = fs::read_to_string(&config_path).expect("expected value");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("expected value");
        let instructions = parsed["instructions"].as_array().expect("expected array");
        let paths: Vec<&str> = instructions
            .iter()
            .map(|v| v.as_str().expect("expected str"))
            .collect();
        assert!(
            !paths.iter().any(|p| p.contains("old-rule")),
            "stale entry removed"
        );
        assert!(
            paths.iter().any(|p| p.contains("new-rule")),
            "new entry present"
        );
        assert!(paths.contains(&"/user/AGENTS.md"), "user entry preserved");
    }

    #[test]
    fn test_patch_empty_rules_dir_removes_agentspec_entries() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir_all(&rules_dir).expect("expected value");

        let config_path = tmp.path().join("opencode.json");
        let stale_path = rules_dir.join("old-rule/AGENTS.md");
        let existing = serde_json::json!({
            "instructions": [stale_path.to_string_lossy(), "/user/AGENTS.md"]
        });
        fs::write(
            &config_path,
            serde_json::to_string(&existing).expect("expected value"),
        )
        .expect("expected value");

        let paths = discover_rules(&rules_dir);
        patch_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), &paths, false)
            .expect("expected value");

        let content = fs::read_to_string(&config_path).expect("expected value");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("expected value");
        let instructions = parsed["instructions"].as_array().expect("expected array");
        assert_eq!(instructions.len(), 1);
        assert_eq!(
            instructions[0].as_str().expect("expected str"),
            "/user/AGENTS.md"
        );
    }

    #[test]
    fn test_patch_dry_run_no_file_written() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");

        let paths = discover_rules(&rules_dir);
        patch_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), &paths, true)
            .expect("expected value");

        assert!(
            !tmp.path().join("opencode.json").exists(),
            "dry_run must not create file"
        );
    }

    // ── remove_opencode_instructions tests ──────────────────────────────────

    #[test]
    fn test_remove_opencode_missing_file_is_no_op() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        let report =
            remove_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), false)
                .expect("ok");
        assert_eq!(report.user_entries_remaining, 0);
        assert!(
            !tmp.path().join("opencode.json").exists(),
            "host file must not be created"
        );
    }

    #[test]
    fn test_remove_opencode_drops_only_agentspec_entries() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        let config_path = tmp.path().join("opencode.json");

        let initial = serde_json::json!({
            "instructions": [
                rules_dir.join("a/AGENTS.md").to_string_lossy(),
                "~/notes/personal.md",
                rules_dir.join("b/AGENTS.md").to_string_lossy(),
            ]
        });
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&initial).expect("ser"),
        )
        .expect("write");

        let report =
            remove_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), false)
                .expect("ok");
        assert_eq!(report.user_entries_remaining, 1);

        let content = std::fs::read_to_string(&config_path).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse");
        let arr = parsed
            .get("instructions")
            .and_then(|v| v.as_array())
            .expect("array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0].as_str(), Some("~/notes/personal.md"));
    }

    #[test]
    fn test_remove_opencode_deletes_file_when_only_agentspec_instructions_were_present() {
        let tmp = tempfile::tempdir().expect("tmp");
        let parent = tmp.path().join("opencode-config");
        std::fs::create_dir_all(&parent).expect("mkdir parent");
        let rules_dir = tmp.path().join("rules");
        let config_path = parent.join("opencode.json");

        let initial = serde_json::json!({
            "instructions": [rules_dir.join("a/AGENTS.md").to_string_lossy()]
        });
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&initial).expect("ser"),
        )
        .expect("write");

        let report = remove_opencode_instructions(&rules_dir, &parent.join(HOST_FILENAME), false)
            .expect("ok");

        assert!(!config_path.exists());
        assert!(!parent.exists());
        assert!(report.host_file_deleted);
        assert!(report.parent_rmdir);
        assert_eq!(report.user_entries_remaining, 0);
    }

    #[test]
    fn test_remove_opencode_keeps_file_when_user_top_level_keys_remain() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        let config_path = tmp.path().join("opencode.json");

        let initial = serde_json::json!({
            "model": "haiku",
            "instructions": [rules_dir.join("a/AGENTS.md").to_string_lossy()]
        });
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&initial).expect("ser"),
        )
        .expect("write");

        let report =
            remove_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), false)
                .expect("ok");

        assert!(config_path.exists());
        assert!(!report.host_file_deleted);
        assert!(!report.parent_rmdir);

        let content = std::fs::read_to_string(&config_path).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse");
        assert!(parsed.get("instructions").is_none());
        assert_eq!(parsed.get("model").and_then(|v| v.as_str()), Some("haiku"));
    }

    #[test]
    fn test_remove_opencode_dry_run_does_not_delete_or_rmdir() {
        let tmp = tempfile::tempdir().expect("tmp");
        let parent = tmp.path().join("opencode-config");
        std::fs::create_dir_all(&parent).expect("mkdir parent");
        let rules_dir = tmp.path().join("rules");
        let config_path = parent.join("opencode.json");

        let initial = serde_json::json!({
            "instructions": [rules_dir.join("a/AGENTS.md").to_string_lossy()]
        });
        let initial_serialized = serde_json::to_string_pretty(&initial).expect("ser");
        std::fs::write(&config_path, &initial_serialized).expect("write");

        let report = remove_opencode_instructions(&rules_dir, &parent.join(HOST_FILENAME), true)
            .expect("ok");

        assert!(config_path.exists());
        let post = std::fs::read_to_string(&config_path).expect("read");
        assert_eq!(post, initial_serialized);
        assert!(parent.exists());
        assert!(report.host_file_deleted);
        assert_eq!(report.user_entries_remaining, 0);
    }

    #[test]
    fn test_remove_opencode_no_op_when_no_agentspec_entries_present() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        let config_path = tmp.path().join("opencode.json");

        let initial = serde_json::json!({
            "model": "haiku",
            "instructions": ["~/notes/personal.md"]
        });
        let initial_serialized = serde_json::to_string_pretty(&initial).expect("ser");
        std::fs::write(&config_path, &initial_serialized).expect("write");

        let pre_mtime = std::fs::metadata(&config_path)
            .expect("meta")
            .modified()
            .expect("mtime");

        std::thread::sleep(std::time::Duration::from_millis(10));

        let report =
            remove_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), false)
                .expect("ok");
        assert_eq!(report.user_entries_remaining, 1);

        let post_mtime = std::fs::metadata(&config_path)
            .expect("meta")
            .modified()
            .expect("mtime");
        assert_eq!(pre_mtime, post_mtime, "no-op cycle must not bump mtime");
    }

    #[test]
    fn test_remove_opencode_preserves_other_top_level_keys() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        let config_path = tmp.path().join("opencode.json");

        let initial = serde_json::json!({
            "model": "haiku",
            "instructions": [rules_dir.join("a/AGENTS.md").to_string_lossy()]
        });
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&initial).expect("ser"),
        )
        .expect("write");

        remove_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), false)
            .expect("ok");

        let content = std::fs::read_to_string(&config_path).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse");
        assert_eq!(parsed.get("model").and_then(|v| v.as_str()), Some("haiku"));
    }

    #[test]
    fn test_user_dest_dir_is_xdg_style() {
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
        let output = OpenCodeAdapter.compile(&[], &ctx).expect("compile");
        assert_eq!(
            output.dest_root,
            PathBuf::from("/home/user/.config/opencode")
        );
    }

    #[test]
    fn test_project_dest_dir_is_flat() {
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
        let output = OpenCodeAdapter.compile(&[], &ctx).expect("compile");
        assert_eq!(output.dest_root, PathBuf::from("/work/project/.opencode"));
    }

    #[test]
    fn test_patch_preserves_comments_and_trivia() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir_all(rules_dir.join("my-rule")).expect("mkdir rule");
        fs::write(rules_dir.join("my-rule/AGENTS.md"), "rule").expect("write rule");

        let config_path = tmp.path().join("opencode.json");
        let initial = r#"{
  // user comment about the model
  "model": "haiku",
  "instructions": []
}
"#;
        fs::write(&config_path, initial).expect("write initial");

        let paths = discover_rules(&rules_dir);
        patch_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), &paths, false)
            .expect("patch");

        let after = fs::read_to_string(&config_path).expect("read");
        assert!(
            after.contains("// user comment about the model"),
            "comment must round-trip, got:\n{after}"
        );
        assert!(
            after.contains("\"model\": \"haiku\""),
            "model value must round-trip, got:\n{after}"
        );
        let model_pos = after.find("\"model\"").expect("model present");
        let instructions_pos = after
            .find("\"instructions\"")
            .expect("instructions present");
        assert!(model_pos < instructions_pos);
        assert!(after.contains("my-rule"));
    }

    #[test]
    fn test_patch_preserves_user_top_level_key_ordering() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir_all(rules_dir.join("r")).expect("mkdir rule");
        fs::write(rules_dir.join("r/AGENTS.md"), "rule").expect("write rule");

        let config_path = tmp.path().join("opencode.json");
        let initial = r#"{
  "model": "haiku",
  "permissions": ["read", "write"],
  "instructions": ["~/notes/personal.md"]
}
"#;
        fs::write(&config_path, initial).expect("write initial");

        let paths = discover_rules(&rules_dir);
        patch_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), &paths, false)
            .expect("patch");

        let after = fs::read_to_string(&config_path).expect("read");
        let model_pos = after.find("\"model\"").expect("model");
        let permissions_pos = after.find("\"permissions\"").expect("permissions");
        let instructions_pos = after.find("\"instructions\"").expect("instructions");
        assert!(model_pos < permissions_pos && permissions_pos < instructions_pos);
        assert!(
            after.contains("\"haiku\"")
                && after.contains("\"read\"")
                && after.contains("\"write\"")
        );
        assert!(after.contains("~/notes/personal.md"));
    }

    #[test]
    fn test_remove_preserves_comments_and_trivia() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        let config_path = tmp.path().join("opencode.json");

        let agentspec_path = rules_dir.join("a/AGENTS.md");
        let initial = format!(
            "{{\n  // user note\n  \"model\": \"haiku\",\n  \"instructions\": [\"{}\"]\n}}\n",
            agentspec_path.display()
        );
        fs::write(&config_path, &initial).expect("write initial");

        remove_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), false)
            .expect("remove");

        let after = fs::read_to_string(&config_path).expect("read");
        assert!(after.contains("// user note"));
        assert!(after.contains("\"model\": \"haiku\""));
        assert!(!after.contains("\"instructions\""));
    }

    #[test]
    fn test_patch_idempotent_round_trip() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir_all(rules_dir.join("r")).expect("mkdir rule");
        fs::write(rules_dir.join("r/AGENTS.md"), "rule").expect("write rule");

        let paths = discover_rules(&rules_dir);
        patch_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), &paths, false)
            .expect("patch 1");
        let after_1 = fs::read_to_string(tmp.path().join("opencode.json")).expect("read 1");

        patch_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), &paths, false)
            .expect("patch 2");
        let after_2 = fs::read_to_string(tmp.path().join("opencode.json")).expect("read 2");

        assert_eq!(after_1, after_2);
    }

    #[test]
    fn test_patch_handles_empty_file() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir_all(rules_dir.join("r")).expect("mkdir rule");
        fs::write(rules_dir.join("r/AGENTS.md"), "rule").expect("write rule");

        let config_path = tmp.path().join("opencode.json");
        fs::write(&config_path, "").expect("touch empty file");

        let paths = discover_rules(&rules_dir);
        patch_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), &paths, false)
            .expect("patch should succeed on empty file");

        let after = fs::read_to_string(&config_path).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&after).expect("must parse as JSON");
        let arr = parsed
            .get("instructions")
            .and_then(|v| v.as_array())
            .expect("instructions array");
        assert_eq!(arr.len(), 1);
        assert!(
            arr[0]
                .as_str()
                .expect("string entry")
                .contains("r/AGENTS.md")
        );
    }

    #[test]
    fn test_patch_warns_on_non_object_root() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir_all(rules_dir.join("r")).expect("mkdir rule");
        fs::write(rules_dir.join("r/AGENTS.md"), "rule").expect("write rule");

        let config_path = tmp.path().join("opencode.json");
        let initial = "[]";
        fs::write(&config_path, initial).expect("write initial");

        let paths = discover_rules(&rules_dir);
        patch_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), &paths, false)
            .expect("patch returns Ok on non-object root");

        let after = fs::read_to_string(&config_path).expect("read");
        assert_eq!(after, initial);
    }

    #[test]
    fn test_remove_warns_on_non_object_root() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        let config_path = tmp.path().join("opencode.json");
        let initial = "[]";
        fs::write(&config_path, initial).expect("write initial");

        let report =
            remove_opencode_instructions(&rules_dir, &tmp.path().join(HOST_FILENAME), false)
                .expect("remove returns Ok on non-object root");

        assert!(!report.host_file_deleted);
        assert_eq!(report.user_entries_remaining, 0);

        let after = fs::read_to_string(&config_path).expect("read");
        assert_eq!(after, initial);
    }

    #[test]
    fn test_compile_eager_instruction_paths() {
        // Regression for the `WalkDir` → eager-paths refactor: the
        // `OpenCodeInstructionsPatch` must carry an instruction list
        // derived from the rule-spec `GeneratedFile`s, anchored under
        // `dest_root`, and sorted alphabetically. We exercise this by
        // running the compiled patch against a real tempdir and inspecting
        // the resulting `opencode.json` — `instruction_paths` is private
        // to the patch struct, but the on-disk effect is the actual
        // contract we care about.
        let tmp = tempfile::tempdir().expect("tmp");
        let dest_root = tmp.path();
        let cfg = AdapterConfig::default();
        let presets = HashMap::new();
        let ctx = CompileCtx {
            mode: SyncDestinationMode::Compile,
            home: Path::new("/should-not-be-consulted"),
            cwd: Path::new("/should-not-be-consulted"),
            target_dir: Some(dest_root),
            presets: &presets,
            adapter_config: Some(&cfg),
            overwrite: false,
        };

        // Authored in non-alphabetical order to confirm the patch sorts
        // them before writing.
        let rule_zulu = Spec::Rule(crate::spec::RuleSpec {
            path: "rule-z.md".into(),
            frontmatter: crate::spec::RuleFrontmatter {
                id: "zulu".to_string(),
                description: None,
                tags: None,
                paths: None,
            },
            body: "zulu body".to_string(),
        });
        let rule_alpha = Spec::Rule(crate::spec::RuleSpec {
            path: "rule-a.md".into(),
            frontmatter: crate::spec::RuleFrontmatter {
                id: "alpha".to_string(),
                description: None,
                tags: None,
                paths: None,
            },
            body: "alpha body".to_string(),
        });

        let output = OpenCodeAdapter
            .compile(&[rule_zulu, rule_alpha], &ctx)
            .expect("compile");

        assert_eq!(output.dest_root, dest_root);
        assert_eq!(
            output
                .files
                .iter()
                .filter(|f| f.kind == FileKind::Rules)
                .count(),
            2
        );
        assert_eq!(output.patches.len(), 1);

        // Run the patch against the tempdir and inspect the resulting
        // `opencode.json`.
        let patch = &output.patches[0];
        let host_path = dest_root.join(HOST_FILENAME);
        patch.run(false).expect("run patch");

        let written = std::fs::read_to_string(&host_path).expect("read opencode.json");
        let parsed: serde_json::Value = serde_json::from_str(&written).expect("valid json");
        let instructions = parsed
            .get("instructions")
            .and_then(|v| v.as_array())
            .expect("instructions array");
        let entries: Vec<&str> = instructions
            .iter()
            .map(|v| v.as_str().expect("string entry"))
            .collect();

        // Two rules → two entries, anchored under `dest_root`, in
        // alphabetical order regardless of input order.
        assert_eq!(entries.len(), 2);
        let alpha_path = dest_root.join("rules/alpha/AGENTS.md");
        let zulu_path = dest_root.join("rules/zulu/AGENTS.md");
        assert_eq!(entries[0], alpha_path.to_string_lossy());
        assert_eq!(entries[1], zulu_path.to_string_lossy());
        assert!(
            entries[0] < entries[1],
            "instructions must be alphabetically sorted"
        );
    }

    #[test]
    fn test_compile_with_no_rules_still_emits_cleanup_patch() {
        // Regression: pre-branch `post_write_hook` was called for every
        // (provider, FileKind::Rules) pair regardless of file count, so
        // `OpenCodeInstructionsPatch` always ran and tidied orphaned
        // agentspec entries from `opencode.json`. The branch's refactor
        // accidentally gated patch construction on `!instruction_paths.is_empty()`,
        // breaking cleanup when a user removed all their rules. This test
        // pins the recovered behavior: compile must always produce the patch
        // for OpenCode.
        let tmp = tempfile::tempdir().expect("tmp");
        let dest_root = tmp.path();
        let cfg = AdapterConfig::default();
        let presets = HashMap::new();
        let ctx = CompileCtx {
            mode: SyncDestinationMode::Compile,
            home: Path::new("/should-not-be-consulted"),
            cwd: Path::new("/should-not-be-consulted"),
            target_dir: Some(dest_root),
            presets: &presets,
            adapter_config: Some(&cfg),
            overwrite: false,
        };

        // Compile with zero rule specs.
        let output = OpenCodeAdapter.compile(&[], &ctx).expect("compile");
        assert_eq!(
            output.patches.len(),
            1,
            "patch must be constructed even when there are no rules, so a sync \
             tidies orphans left by a prior sync"
        );

        // Pre-seed opencode.json with an orphaned agentspec entry, then run
        // the patch and confirm the orphan is stripped.
        let host_path = dest_root.join(HOST_FILENAME);
        let rules_dir = dest_root.join(
            OpenCodeAdapter
                .dir_for_kind(FileKind::Rules)
                .unwrap_or("rules"),
        );
        let stale_path = rules_dir.join("removed-rule/AGENTS.md");
        let existing = serde_json::json!({
            "instructions": [stale_path.to_string_lossy(), "/user/AGENTS.md"]
        });
        fs::write(
            &host_path,
            serde_json::to_string(&existing).expect("serialize"),
        )
        .expect("write");

        output.patches[0].run(false).expect("run patch");

        let written = fs::read_to_string(&host_path).expect("read opencode.json");
        let parsed: serde_json::Value = serde_json::from_str(&written).expect("valid json");
        let instructions = parsed
            .get("instructions")
            .and_then(|v| v.as_array())
            .expect("instructions array");
        assert_eq!(
            instructions.len(),
            1,
            "orphaned agentspec entry must be stripped"
        );
        assert_eq!(
            instructions[0].as_str().expect("string"),
            "/user/AGENTS.md",
            "user-authored entry must be preserved"
        );
    }

    #[test]
    fn test_adapt_rule_with_paths_ignored() {
        let presets = HashMap::new();
        let ctx = CompileCtx {
            mode: SyncDestinationMode::Compile,
            home: Path::new("/home"),
            cwd: Path::new("/work"),
            target_dir: None,
            presets: &presets,
            adapter_config: None,
            overwrite: false,
        };

        let rule_with_paths = Spec::Rule(crate::spec::RuleSpec {
            path: "react.md".into(),
            frontmatter: crate::spec::RuleFrontmatter {
                id: "react-rule".to_string(),
                description: None,
                tags: None,
                paths: Some(vec!["src/**/*.tsx".to_string()]),
            },
            body: "Rule body.".to_string(),
        });
        let rule_without_paths = Spec::Rule(crate::spec::RuleSpec {
            path: "react-plain.md".into(),
            frontmatter: crate::spec::RuleFrontmatter {
                id: "react-rule-plain".to_string(),
                description: None,
                tags: None,
                paths: None,
            },
            body: "Rule body.".to_string(),
        });

        let with_paths = OpenCodeAdapter
            .compile(&[rule_with_paths], &ctx)
            .expect("compile")
            .files;
        let without_paths = OpenCodeAdapter
            .compile(&[rule_without_paths], &ctx)
            .expect("compile")
            .files;

        let content_with = String::from_utf8(with_paths[0].content.clone()).expect("utf8");
        let content_without = String::from_utf8(without_paths[0].content.clone()).expect("utf8");

        assert!(
            !content_with.contains("paths:") && !content_with.contains("globs:"),
            "opencode rule should not contain paths or globs, got: {content_with}"
        );
        assert_eq!(
            content_with, content_without,
            "opencode should emit identical output regardless of paths field"
        );
    }

    #[test]
    fn test_hook_command_preview_returns_empty_string() {
        // OpenCode emits no hooks (`emits_hooks() == false`), and the only
        // caller of this method dispatches through `ProviderName`, which
        // has no `OpenCode` variant — this impl exists only to satisfy the
        // trait.
        let preview = OpenCodeAdapter.hook_command_preview(
            HookEvent::PreToolUse,
            std::path::Path::new("scripts/audit.sh"),
            "audit-bash",
            &["--strict".to_string()],
        );
        assert_eq!(preview, "");
    }
}
