use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use jsonc_parser::ParseOptions;
use jsonc_parser::cst::{CstInputValue, CstObject, CstRootNode};
use serde::Serialize;

use super::hooks_helpers::has_agentspec_entries;
use super::{
    Adapter, AdapterOutput, CompileCtx, Degradation, DegradationKind, Delivery, RemovalOutput,
    RemoveCtx, SyncDestinationMode,
};
use crate::compile::{AdapterConfig, GeneratedFile};
use crate::declarations::Declarations;
use crate::mcp::{McpServer, McpServers, OpenCodeMcpServer, is_mcp_name};
use crate::plan::{FileKind, ForwardPatch, RemovePatchReport, ReversePatch};
use crate::presets::{ProviderPresets, ProviderPresetsMap};
use crate::provider::Provider;
use crate::setting::{Carries, SettingKey, SettingKind};
use crate::spec::{
    AgentSpec, HookEvent, McpGrant, McpTools, RuleSpec, SkillSpec, Spec, ToolFrontmatter,
};

// See: https://opencode.ai/docs/agents/#markdown
// See: https://opencode.ai/docs/agents/#permissions
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
struct OpenCodeAgentFrontmatter {
    description: String,
    mode: &'static str,
    model: Option<String>,
    variant: Option<String>,
    permission: Option<IndexMap<String, OpenCodePermissionRule>>,
    /// The logical servers whose grants `permission` carries. Server names
    /// cannot be read back from the map's keys without parsing them, so the
    /// expression that writes the keys records them here.
    #[serde(skip)]
    mcp_carried: Vec<String>,
}

/// One `permission` map value: an action for every pattern of the permission,
/// or a map of patterns to actions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
enum OpenCodePermissionRule {
    Action(OpenCodePermission),
    Patterns(IndexMap<String, OpenCodePermission>),
}

/// An `OpenCode` permission action. Serialized lowercase, as `OpenCode`'s
/// `permission` field spells it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum OpenCodePermission {
    Allow,
    Ask,
    Deny,
}

impl Carries for OpenCodeAgentFrontmatter {
    fn carried(&self) -> Vec<SettingKey> {
        // The `permission` map is present exactly when the spec declares
        // `capabilities.tools`.
        [
            self.model.as_ref().map(|_| SettingKey::Model),
            self.variant.as_ref().map(|_| SettingKey::Variant),
            self.permission.as_ref().map(|_| SettingKey::Tools),
        ]
        .into_iter()
        .flatten()
        .chain(
            self.mcp_carried
                .iter()
                .map(|server| SettingKey::Mcp(server.clone())),
        )
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
        let mut deliveries = Vec::new();
        let mut degradations = Vec::new();
        for spec in specs {
            match spec {
                Spec::Agent(s) => {
                    let tools = s
                        .frontmatter
                        .capabilities
                        .as_ref()
                        .and_then(|c| c.tools.as_deref());
                    if tools.is_some_and(splits_edit_write) {
                        degradations.push(Degradation::provider_wide(
                            Provider::OpenCode,
                            DegradationKind::EditWriteCoupled,
                        ));
                    }
                    if tools.is_some_and(|t| {
                        t.iter().any(|tool| permission_key(tool) == READ_PERMISSION)
                    }) {
                        degradations.push(Degradation::provider_wide(
                            Provider::OpenCode,
                            DegradationKind::McpResourceToolsOffered,
                        ));
                    }
                    if spec
                        .mcp_grants()
                        .is_some_and(|g| g.values().any(|grant| grant.tools == McpTools::All))
                    {
                        degradations.push(Degradation::provider_wide(
                            Provider::OpenCode,
                            DegradationKind::McpServerGlobOverMatch,
                        ));
                    }
                    let (f, d) = adapt_agent_spec(
                        s.clone(),
                        ctx.presets,
                        ctx.mcp_servers,
                        ctx.adapter_config,
                    )?;
                    files.extend(f);
                    deliveries.extend(d);
                }
                Spec::Skill(s) => {
                    let (f, d) = adapt_skill_spec(s.clone(), ctx.presets, ctx.adapter_config)?;
                    files.extend(f);
                    deliveries.extend(d);
                }
                Spec::Rule(s) => {
                    let (f, d) = adapt_rule_spec(s, ctx.adapter_config);
                    files.extend(f);
                    deliveries.extend(d);
                }
                // Hooks are not emitted for OpenCode in v1, and nothing is
                // recorded for one. The spec therefore holds a `Body` intent
                // no delivery satisfies, and the orchestrator's subtraction
                // reports the loss — naming the spec, which the old
                // provider-wide degradation could not.
                Spec::Hook(_) => {}
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
            // `OpenCode`'s runtime claims: it grants `edit` and `write` together,
            // it still offers its MCP resource tools to an agent whose `read`
            // pattern map refuses every call to them, and its whole-server key
            // matches by name prefix.
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

    /// Resolve a canonical tool to the name an `OpenCode` spec body should
    /// reference. Body content only: agent frontmatter names permissions,
    /// which `permission_key` resolves.
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

    fn body_mcp_tool_name(&self, server: &McpServer, logical: &str, tool: &str) -> String {
        compose_mcp_tool(server, logical, tool)
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
                SettingKind::Mcp,
            ],
            FileKind::Commands => &[SettingKind::Body, SettingKind::Model, SettingKind::Variant],
            FileKind::Skills | FileKind::Rules => &[SettingKind::Body],
            FileKind::Hooks | FileKind::PluginManifest => &[],
        }
    }

    fn validate_declarations(&self, declarations: &Declarations) -> Vec<String> {
        let Declarations { presets, mcp } = declarations;
        // An `opencode` preset block has no cross-field constraint: a
        // `variant` with no `model` is accepted and inert. The binding is what
        // makes a new provider block a compile error here.
        for preset in presets.values() {
            let ProviderPresets {
                claude: _,
                cursor: _,
                opencode: _,
            } = preset;
        }

        let mut errors = Vec::new();
        for (name, server) in mcp {
            let McpServer {
                claude: _,
                cursor: _,
                opencode,
            } = server;
            if let Some(OpenCodeMcpServer {
                server: Some(override_name),
            }) = opencode
                && !is_mcp_name(override_name)
            {
                errors.push(format!(
                    "[mcp.{name}.opencode] `server` must match [A-Za-z0-9_-]+ \
                     (got {override_name:?})"
                ));
            }
        }
        errors.extend(mcp_name_overlaps(mcp));
        errors
    }

    /// Unreachable rather than meaningful: `OpenCode` emits no hooks — its
    /// `carriable(FileKind::Hooks)` is empty — and the only caller
    /// (`hook test`) dispatches through `ProviderName`, which has no
    /// `OpenCode` variant.
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
    mcp_servers: &McpServers,
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

    let (builtins, grants) = spec
        .frontmatter
        .capabilities
        .map(|c| (c.tools, c.mcp))
        .unwrap_or_default();
    debug_assert!(
        grants.is_none() || builtins.is_some(),
        "an agent granting MCP tools without capabilities.tools should have been rejected by \
         validate_semantics"
    );
    let (permission, mcp_carried) = builtins
        .map(|tools| build_permission_map(&tools, grants.as_ref(), mcp_servers))
        .map_or((None, Vec::new()), |(map, carried)| (Some(map), carried));

    let frontmatter = OpenCodeAgentFrontmatter {
        description,
        mode: "subagent",
        model,
        variant,
        permission,
        mcp_carried,
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

/// Build the `permission` map for an `OpenCode` agent whose spec declares
/// `capabilities.tools`.
///
/// The map is an allowlist. `OpenCode` resolves these keys as rules, last match
/// wins in authored order, so `"*": "deny"` leads — the reverse order denies the
/// declared tools too (`experiments/opencode-agent-permission-deny-all/`). `*`
/// matches every permission, not only tools, so `external_directory` and
/// `doom_loop` are restated after the allows. Without them, any access outside
/// the project fails with no prompt
/// (`experiments/opencode-agent-permission-external-read/`).
///
/// `read` is written as a pattern map rather than `allow`, because `OpenCode`'s
/// `read` permission also governs MCP resources (`mcp:<server>:<uri>` and
/// `mcp:<server>:*` patterns). `{"*": "allow", "mcp:*": "deny"}` keeps file
/// reads and refuses every resource; rules resolve last-match-wins, so
/// `mcp:*` follows `*` (`experiments/opencode-agent-mcp-resource-read/`).
///
/// MCP grants follow the built-in allows, one `allow` per composed key sorted
/// by key, and precede the restatements. Returns the map with the logical
/// servers it wrote a key for.
fn build_permission_map(
    tools: &[ToolFrontmatter],
    grants: Option<&BTreeMap<String, McpGrant>>,
    servers: &McpServers,
) -> (IndexMap<String, OpenCodePermissionRule>, Vec<String>) {
    let mut allowed: Vec<&'static str> = tools.iter().map(permission_key).collect();
    allowed.sort_unstable();
    allowed.dedup();

    let mut mcp_keys: Vec<String> = Vec::new();
    let mut carried: Vec<String> = Vec::new();
    for (logical, McpGrant { tools: granted }) in grants.into_iter().flatten() {
        debug_assert!(
            servers.contains_key(logical),
            "undeclared MCP server '{logical}' should have been rejected by validate_semantics"
        );
        let Some(server) = servers.get(logical) else {
            continue;
        };
        match granted {
            McpTools::All => mcp_keys.push(compose_mcp_server_glob(server, logical)),
            McpTools::Named(named) => mcp_keys.extend(
                named
                    .iter()
                    .map(|tool| compose_mcp_tool(server, logical, tool)),
            ),
        }
        carried.push(logical.clone());
    }
    mcp_keys.sort_unstable();
    let expected_len = 1 + allowed.len() + mcp_keys.len() + RESTATED_PERMISSIONS.len();

    let allow = |key: &'static str| {
        let rule = if key == READ_PERMISSION {
            OpenCodePermissionRule::Patterns(IndexMap::from([
                ("*".to_owned(), OpenCodePermission::Allow),
                ("mcp:*".to_owned(), OpenCodePermission::Deny),
            ]))
        } else {
            OpenCodePermissionRule::Action(OpenCodePermission::Allow)
        };
        (key.to_owned(), rule)
    };

    let map: IndexMap<String, OpenCodePermissionRule> = std::iter::once((
        "*".to_owned(),
        OpenCodePermissionRule::Action(OpenCodePermission::Deny),
    ))
    .chain(allowed.into_iter().map(allow))
    .chain(mcp_keys.into_iter().map(|key| {
        (
            key,
            OpenCodePermissionRule::Action(OpenCodePermission::Allow),
        )
    }))
    .chain(RESTATED_PERMISSIONS.into_iter().map(|key| {
        (
            key.to_owned(),
            OpenCodePermissionRule::Action(OpenCodePermission::Ask),
        )
    }))
    .collect();
    // Collecting a repeated key would overwrite an earlier rule in place.
    // Validation keeps every key distinct (`mcp_name_overlaps`).
    debug_assert_eq!(
        map.len(),
        expected_len,
        "a permission key repeated: {map:?}"
    );
    (map, carried)
}

/// The permission that governs file reads and, through `mcp:` patterns, MCP
/// resources. Both the `read` pattern map and the warning that accompanies it
/// key on it, so they cannot disagree about which tools it covers.
const READ_PERMISSION: &str = "read";

/// The permissions every agent `permission` map restates as `ask` after its
/// allows, because the leading `"*": "deny"` matches them too
/// (`experiments/opencode-agent-permission-external-read/`).
const RESTATED_PERMISSIONS: [&str; 2] = ["external_directory", "doom_loop"];

/// `OpenCode`'s built-in permissions, other than the restated ones, whose
/// names contain `_` — so a server name they extend would grant them through
/// a `<server>_*` or `<server>_<tool>` key. Both default to `deny`, which such
/// a grant would silently lift. Listed as `OpenCode` 1.18.34 resolves them in
/// `opencode debug agent`.
const UNDERSCORED_PERMISSIONS: [&str; 2] = ["plan_enter", "plan_exit"];

/// The name `OpenCode` registers a declared MCP server under: the
/// `[mcp.<name>.opencode] server` override, or the logical name.
fn resolved_server<'a>(logical: &'a str, server: &'a McpServer) -> &'a str {
    let McpServer {
        claude: _,
        cursor: _,
        opencode,
    } = server;
    opencode
        .as_ref()
        .and_then(|o| o.server.as_deref())
        .unwrap_or(logical)
}

/// `OpenCode`'s id for one tool of a declared server, `<server>_<tool>`: the
/// one place the named-tool spelling is composed.
fn compose_mcp_tool(server: &McpServer, logical: &str, tool: &str) -> String {
    format!("{}_{tool}", resolved_server(logical, server))
}

/// `OpenCode`'s key for every tool of a declared server, `<server>_*`. It
/// matches any tool whose name begins `<server>_`, whichever server it comes
/// from (`experiments/opencode-agent-mcp-server-glob/`).
fn compose_mcp_server_glob(server: &McpServer, logical: &str) -> String {
    format!("{}_*", resolved_server(logical, server))
}

/// Reject declared servers whose `OpenCode` names overlap each other or a
/// reserved permission — a restated one, or one of
/// [`UNDERSCORED_PERMISSIONS`].
///
/// `OpenCode` names an MCP tool `<server>_<tool>`, so a grant's permission key
/// for one server also matches tools of any server whose name extends it past
/// an underscore: `fx_*` matches `fx_extra`'s tools, `external_*` would
/// match `external_directory`, and `plan_*` would match `plan_enter`. Each unordered pair is checked once, against
/// both prefix directions, so declaration order cannot decide whether a
/// collision is found.
fn mcp_name_overlaps(mcp: &McpServers) -> Vec<String> {
    // A name that breaks the neutral rule is already reported, by `validate.rs`
    // or by the override check above; checking it here would report it twice.
    let servers: Vec<(String, &str)> = mcp
        .iter()
        .map(|(name, server)| (format!("[mcp.{name}]"), resolved_server(name, server)))
        .filter(|(_, resolved)| is_mcp_name(resolved))
        .collect();
    let server_count = servers.len();
    let entries: Vec<(String, &str)> = servers
        .into_iter()
        .chain(
            RESTATED_PERMISSIONS
                .iter()
                .map(|p| (format!("the `{p}` permission agentspec restates"), *p)),
        )
        .chain(
            UNDERSCORED_PERMISSIONS
                .iter()
                .map(|p| (format!("OpenCode's built-in `{p}` permission"), *p)),
        )
        .collect();

    // A reserved permission is an exact key, never a `_*` glob or a tool's
    // prefix, so it collides only with a server name it extends.
    let overlaps = |x: &str, y: &str, y_is_reserved: bool| {
        if y_is_reserved {
            extends_past_underscore(y, x)
        } else {
            x == y || extends_past_underscore(y, x) || extends_past_underscore(x, y)
        }
    };

    let mut errors = Vec::new();
    for i in 0..server_count {
        for j in (i + 1)..entries.len() {
            let (first, x) = &entries[i];
            let (second, y) = &entries[j];
            if overlaps(x, y, j >= server_count) {
                errors.push(format!(
                    "{first} and {second} overlap as OpenCode names (`{x}` and `{y}`): \
                     OpenCode names an MCP tool `<server>_<tool>`, so a permission key \
                     for one would also match the other; set a distinct `server` under \
                     [mcp.<name>.opencode]"
                ));
            }
        }
    }
    errors
}

/// Whether `name` is `prefix` followed by `_`.
fn extends_past_underscore(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|rest| rest.starts_with('_'))
}

/// The `OpenCode` permission that governs a canonical tool. Permission names
/// match tool ids except that `edit`, `write`, and `apply_patch` share `edit`,
/// so `Write` maps to `edit` — never to a `write` or `apply_patch` key, which
/// `OpenCode` would accept and ignore (TODO #14).
fn permission_key(tool: &ToolFrontmatter) -> &'static str {
    match tool {
        ToolFrontmatter::Read => "read",
        ToolFrontmatter::Write | ToolFrontmatter::Edit => "edit",
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

/// Whether a declared tool list names exactly one of `edit` and `write`, which
/// `OpenCode` grants together.
fn splits_edit_write(tools: &[ToolFrontmatter]) -> bool {
    let edit = tools.iter().any(|t| matches!(t, ToolFrontmatter::Edit));
    let write = tools.iter().any(|t| matches!(t, ToolFrontmatter::Write));
    edit != write
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

    /// An agent whose spec declares no tools gets no `permission` map, so the
    /// file carries no tool setting at all.
    #[test]
    fn test_agent_frontmatter_carries_no_tools_when_none_declared() {
        let frontmatter = OpenCodeAgentFrontmatter {
            description: "d".to_owned(),
            mode: "subagent",
            model: None,
            variant: None,
            permission: None,
            mcp_carried: Vec::new(),
        };
        assert!(frontmatter.carried().is_empty());
    }

    #[test]
    fn test_agent_frontmatter_carries_model_and_variant_when_set() {
        let frontmatter = OpenCodeAgentFrontmatter {
            description: "d".to_owned(),
            mode: "subagent",
            model: Some("anthropic/claude-opus-5".to_owned()),
            variant: Some("thinking".to_owned()),
            permission: None,
            mcp_carried: Vec::new(),
        };
        assert_eq!(
            frontmatter.carried(),
            vec![SettingKey::Model, SettingKey::Variant]
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
            mcp_servers: &McpServers::new(),
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

    /// An agent spec with no preset, declaring `capabilities.tools` when
    /// `tools` is `Some` and no `capabilities` block otherwise.
    fn agent_spec(id: &str, tools: Option<Vec<ToolFrontmatter>>) -> Spec {
        Spec::Agent(AgentSpec {
            path: format!("{id}.md").into(),
            frontmatter: AgentFrontmatter {
                id: id.to_string(),
                description: "An agent".to_string(),
                tags: None,
                execution: None,
                capabilities: tools.map(|tools| CapabilitiesFrontmatter {
                    tools: Some(tools),
                    mcp: None,
                }),
            },
            body: "Body.".to_string(),
        })
    }

    /// The frontmatter block of a generated file, parsed as a YAML mapping.
    fn frontmatter_of(file: &GeneratedFile) -> serde_yml::Mapping {
        let content = String::from_utf8(file.content.clone()).expect("utf8");
        let yaml = content
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("---\n"))
            .map(|(yaml, _)| yaml)
            .expect("frontmatter block");
        serde_yml::from_str(yaml).expect("frontmatter parses")
    }

    #[test]
    fn test_build_permission_map_leads_with_deny_all_then_allows_then_restatements() {
        let map = build_permission_map(
            &[
                ToolFrontmatter::Write,
                ToolFrontmatter::Read,
                ToolFrontmatter::Edit,
                ToolFrontmatter::Write,
            ],
            None,
            &McpServers::new(),
        )
        .0;
        let keys: Vec<&str> = map.keys().map(String::as_str).collect();
        let values: Vec<OpenCodePermissionRule> = map.values().cloned().collect();
        assert_eq!(
            keys,
            ["*", "edit", "read", "external_directory", "doom_loop"]
        );
        assert_eq!(
            values,
            [
                OpenCodePermissionRule::Action(OpenCodePermission::Deny),
                OpenCodePermissionRule::Action(OpenCodePermission::Allow),
                OpenCodePermissionRule::Patterns(IndexMap::from([
                    ("*".to_owned(), OpenCodePermission::Allow),
                    ("mcp:*".to_owned(), OpenCodePermission::Deny),
                ])),
                OpenCodePermissionRule::Action(OpenCodePermission::Ask),
                OpenCodePermissionRule::Action(OpenCodePermission::Ask),
            ]
        );
    }

    /// `read` serializes as a pattern map with `mcp:*` after `*`: rules
    /// resolve last-match-wins, so the reverse order would allow resources.
    #[test]
    fn test_build_permission_map_read_withholds_mcp_resources() {
        let map = build_permission_map(&[ToolFrontmatter::Read], None, &McpServers::new()).0;
        let json = serde_json::to_string(&map["read"]).expect("serializes");
        assert_eq!(json, r#"{"*":"allow","mcp:*":"deny"}"#);
    }

    #[test]
    fn test_build_permission_map_empty_list_denies_all() {
        let map = build_permission_map(&[], None, &McpServers::new()).0;
        let keys: Vec<&str> = map.keys().map(String::as_str).collect();
        assert_eq!(keys, ["*", "external_directory", "doom_loop"]);
    }

    #[test]
    fn test_permission_key_shares_edit_for_write_and_edit() {
        assert_eq!(permission_key(&ToolFrontmatter::Write), "edit");
        assert_eq!(permission_key(&ToolFrontmatter::Edit), "edit");
    }

    #[test]
    fn test_adapt_agent_without_tools_emits_no_permission() {
        let files = compile_one(agent_spec("plain-agent", None), None);
        let content = String::from_utf8(files[0].content.clone()).expect("utf8");
        assert!(
            !content.contains("permission:") && !content.contains("tools:"),
            "an agent declaring no tools must carry no tool map, got:\n{content}"
        );
    }

    #[test]
    fn test_adapt_agent_with_tools_emits_deny_all_first() {
        let spec = agent_spec(
            "restricted-agent",
            Some(vec![ToolFrontmatter::Shell, ToolFrontmatter::Read]),
        );
        let files = compile_one(spec, None);
        let frontmatter = frontmatter_of(&files[0]);

        assert!(frontmatter.get("tools").is_none());
        let permission = frontmatter
            .get("permission")
            .and_then(serde_yml::Value::as_mapping)
            .expect("permission mapping");
        // A pattern map renders as JSON, so its order is part of the comparison.
        let entries: Vec<(&str, String)> = permission
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().expect("string key"),
                    v.as_str().map_or_else(
                        || serde_json::to_string(v).expect("serializes"),
                        str::to_owned,
                    ),
                )
            })
            .collect();
        assert_eq!(
            entries,
            [
                ("*", "deny".to_owned()),
                ("bash", "allow".to_owned()),
                ("read", r#"{"*":"allow","mcp:*":"deny"}"#.to_owned()),
                ("external_directory", "ask".to_owned()),
                ("doom_loop", "ask".to_owned()),
            ]
        );
    }

    #[test]
    fn test_compile_pushes_each_degradation_only_when_its_tools_are_declared() {
        let presets = HashMap::new();
        let ctx = CompileCtx {
            mode: SyncDestinationMode::Compile,
            home: Path::new("/tmp/home"),
            cwd: Path::new("/tmp/cwd"),
            target_dir: None,
            presets: &presets,
            mcp_servers: &McpServers::new(),
            adapter_config: None,
            overwrite: false,
        };
        let kinds = |tools: Vec<ToolFrontmatter>| -> Vec<DegradationKind> {
            OpenCodeAdapter
                .compile(&[agent_spec("a", Some(tools))], &ctx)
                .expect("compile")
                .degradations
                .iter()
                .map(Degradation::kind)
                .collect()
        };

        assert_eq!(
            kinds(vec![ToolFrontmatter::Grep, ToolFrontmatter::Edit]),
            [DegradationKind::EditWriteCoupled]
        );
        assert!(kinds(vec![ToolFrontmatter::Edit, ToolFrontmatter::Write]).is_empty());
        assert!(kinds(vec![ToolFrontmatter::Grep]).is_empty());

        // `read` draws the MCP resource warning, after the edit/write one.
        assert_eq!(
            kinds(vec![ToolFrontmatter::Read]),
            [DegradationKind::McpResourceToolsOffered]
        );
        assert_eq!(
            kinds(vec![ToolFrontmatter::Read, ToolFrontmatter::Edit]),
            [
                DegradationKind::EditWriteCoupled,
                DegradationKind::McpResourceToolsOffered,
            ]
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
                    mcp: None,
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
            mcp_servers: &McpServers::new(),
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
            mcp_servers: &McpServers::new(),
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
            mcp_servers: &McpServers::new(),
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
            mcp_servers: &McpServers::new(),
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
            mcp_servers: &McpServers::new(),
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
        // OpenCode emits no hooks (`carriable(FileKind::Hooks)` is empty),
        // and the only caller of this method dispatches through `ProviderName`, which
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

#[cfg(test)]
mod mcp_declaration_tests {
    use super::*;

    fn opencode_server(name: &str) -> McpServer {
        McpServer {
            opencode: Some(OpenCodeMcpServer {
                server: Some(name.to_owned()),
            }),
            ..McpServer::default()
        }
    }

    fn messages(servers: &[(&str, McpServer)]) -> Vec<String> {
        let declarations = Declarations {
            mcp: servers
                .iter()
                .map(|(n, s)| ((*n).to_owned(), s.clone()))
                .collect(),
            ..Declarations::default()
        };
        OpenCodeAdapter.validate_declarations(&declarations)
    }

    /// The `BTreeMap` visits `fx` before `fx_extra`, so the shorter name is
    /// declared first here.
    #[test]
    fn test_mcp_overlap_rejects_prefix_declared_first() {
        let errors = messages(&[
            ("fx", McpServer::default()),
            ("fx_extra", McpServer::default()),
        ]);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("[mcp.fx] and [mcp.fx_extra]"),
            "{}",
            errors[0]
        );
    }

    /// `zz_a` resolves to `fy` and sorts after `fy_b`, so the shorter resolved
    /// name is visited second.
    #[test]
    fn test_mcp_overlap_rejects_prefix_declared_second() {
        let errors = messages(&[
            ("fy_b", McpServer::default()),
            ("zz_a", opencode_server("fy")),
        ]);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("[mcp.fy_b] and [mcp.zz_a]"),
            "{}",
            errors[0]
        );
        assert!(errors[0].contains("`fy_b` and `fy`"), "{}", errors[0]);
    }

    #[test]
    fn test_mcp_overlap_rejects_two_servers_resolving_to_one_name() {
        let errors = messages(&[
            ("atlassian", opencode_server("jira")),
            ("jira", McpServer::default()),
        ]);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("`jira` and `jira`"), "{}", errors[0]);
    }

    #[test]
    fn test_mcp_overlap_rejects_restated_permission() {
        let errors = messages(&[("external", McpServer::default())]);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("`external_directory` permission"),
            "{}",
            errors[0]
        );
    }

    #[test]
    fn test_mcp_overlap_rejects_underscored_builtin_permission() {
        let errors = messages(&[("plan", McpServer::default())]);
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(
            errors[0].contains("built-in `plan_enter` permission"),
            "{}",
            errors[0]
        );
        assert!(
            errors[1].contains("built-in `plan_exit` permission"),
            "{}",
            errors[1]
        );
    }

    /// A restated permission is an exact key, so a server named after it, or
    /// extending it, collides with nothing.
    #[test]
    fn test_mcp_overlap_accepts_names_equal_to_or_extending_restated_permissions() {
        assert_eq!(
            messages(&[
                ("doom_loop_x", McpServer::default()),
                ("external_directory", McpServer::default()),
            ]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_mcp_overlap_accepts_names_sharing_a_prefix_without_underscore() {
        assert_eq!(
            messages(&[("fx", McpServer::default()), ("fxa", McpServer::default())]),
            Vec::<String>::new()
        );
    }

    /// A name the neutral rule rejects is reported there, not again here.
    #[test]
    fn test_mcp_overlap_skips_names_failing_the_neutral_rule() {
        assert_eq!(
            messages(&[("doom_loop_.x", McpServer::default())]),
            Vec::<String>::new()
        );
        assert_eq!(
            messages(&[("q", opencode_server("")), ("r", opencode_server("_x"))]),
            ["[mcp.q.opencode] `server` must match [A-Za-z0-9_-]+ (got \"\")"]
        );
    }

    #[test]
    fn test_mcp_opencode_server_override_must_be_mcp_name() {
        assert_eq!(
            messages(&[("quip", opencode_server("a.b"))]),
            ["[mcp.quip.opencode] `server` must match [A-Za-z0-9_-]+ (got \"a.b\")"]
        );
    }
}

#[cfg(test)]
mod mcp_grant_tests {
    use std::collections::HashMap;

    use indexmap::IndexMap;

    use super::*;
    use crate::spec::{AgentFrontmatter, CapabilitiesFrontmatter, SkillFrontmatter};

    fn servers() -> McpServers {
        McpServers::from([
            ("quip".to_owned(), McpServer::default()),
            (
                "atlassian".to_owned(),
                McpServer {
                    opencode: Some(OpenCodeMcpServer {
                        server: Some("jira".to_owned()),
                    }),
                    ..McpServer::default()
                },
            ),
        ])
    }

    fn grants(entries: &[(&str, McpTools)]) -> BTreeMap<String, McpGrant> {
        entries
            .iter()
            .map(|(s, t)| ((*s).to_owned(), McpGrant { tools: t.clone() }))
            .collect()
    }

    fn named(tools: &[&str]) -> McpTools {
        McpTools::Named(tools.iter().map(|t| (*t).to_owned()).collect())
    }

    fn agent(tools: Vec<ToolFrontmatter>, mcp: &[(&str, McpTools)]) -> Spec {
        Spec::Agent(AgentSpec {
            path: PathBuf::from("a.md"),
            frontmatter: AgentFrontmatter {
                id: "a".to_owned(),
                description: "d".to_owned(),
                tags: None,
                execution: None,
                capabilities: Some(CapabilitiesFrontmatter {
                    tools: Some(tools),
                    mcp: Some(grants(mcp)),
                }),
            },
            body: "Body.".to_owned(),
        })
    }

    fn compile(spec: Spec) -> AdapterOutput {
        let servers = servers();
        let presets = HashMap::new();
        let ctx = CompileCtx {
            mode: SyncDestinationMode::Compile,
            home: Path::new("/tmp/home"),
            cwd: Path::new("/tmp/cwd"),
            target_dir: None,
            presets: &presets,
            mcp_servers: &servers,
            adapter_config: None,
            overwrite: false,
        };
        OpenCodeAdapter.compile(&[spec], &ctx).expect("compile")
    }

    fn keys(tools: &[ToolFrontmatter], mcp: &[(&str, McpTools)]) -> Vec<String> {
        let map = build_permission_map(tools, Some(&grants(mcp)), &servers()).0;
        map.keys().cloned().collect()
    }

    /// Named and whole-server grants, with and without an `opencode.server`
    /// override, land after the built-ins and before the restatements, sorted
    /// by key.
    #[test]
    fn test_mcp_keys_follow_builtins_and_precede_restatements() {
        assert_eq!(
            keys(
                &[ToolFrontmatter::Grep],
                &[
                    ("quip", named(&["search_documents", "get_document"])),
                    ("atlassian", McpTools::All),
                ],
            ),
            [
                "*",
                "grep",
                "jira_*",
                "quip_get_document",
                "quip_search_documents",
                "external_directory",
                "doom_loop",
            ]
        );
        assert_eq!(
            keys(
                &[],
                &[
                    ("atlassian", named(&["get_issue"])),
                    ("quip", McpTools::All)
                ]
            ),
            [
                "*",
                "jira_get_issue",
                "quip_*",
                "external_directory",
                "doom_loop"
            ]
        );
    }

    #[test]
    fn test_mcp_allows_are_bare_allow_actions() {
        let map =
            build_permission_map(&[], Some(&grants(&[("quip", McpTools::All)])), &servers()).0;
        assert_eq!(
            map["quip_*"],
            OpenCodePermissionRule::Action(OpenCodePermission::Allow)
        );
    }

    /// One `Mcp` delivery per server, recorded on the agent file.
    #[test]
    fn test_mcp_carried_records_one_delivery_per_server() {
        let output = compile(agent(
            vec![],
            &[("quip", named(&["a", "b"])), ("atlassian", McpTools::All)],
        ));
        let mcp: Vec<&SettingKey> = output
            .deliveries
            .iter()
            .map(Delivery::setting)
            .filter(|s| matches!(s, SettingKey::Mcp(_)))
            .collect();
        assert_eq!(
            mcp,
            [
                &SettingKey::Mcp("atlassian".to_owned()),
                &SettingKey::Mcp("quip".to_owned()),
            ]
        );
    }

    /// `OpenCode` reads no tool restriction on skills or commands, so a skill's
    /// grant records nothing there.
    #[test]
    fn test_mcp_skill_grant_records_nothing() {
        let output = compile(Spec::Skill(SkillSpec {
            path: PathBuf::from("s"),
            frontmatter: SkillFrontmatter {
                id: "s".to_owned(),
                description: Some("d".to_owned()),
                tags: None,
                user_invocable: true,
                agent_invocable: true,
                execution: None,
                capabilities: Some(CapabilitiesFrontmatter {
                    tools: None,
                    mcp: Some(grants(&[("quip", McpTools::All)])),
                }),
            },
            body: "Body.".to_owned(),
            supporting_files: IndexMap::new(),
        }));
        assert!(
            !output
                .deliveries
                .iter()
                .any(|d| matches!(d.setting(), SettingKey::Mcp(_))),
            "{:?}",
            output.deliveries
        );
    }

    #[test]
    fn test_mcp_server_glob_over_match_pushed_only_for_all_grant() {
        let pushed = |mcp: &[(&str, McpTools)]| {
            compile(agent(vec![], mcp))
                .degradations
                .iter()
                .any(|d| d.kind() == DegradationKind::McpServerGlobOverMatch)
        };
        assert!(pushed(&[("quip", McpTools::All)]));
        assert!(!pushed(&[("quip", named(&["a"]))]));
    }
}
