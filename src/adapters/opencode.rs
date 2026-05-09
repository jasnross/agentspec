use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use jsonc_parser::ParseOptions;
use jsonc_parser::cst::{CstInputValue, CstObject, CstRootNode};
use serde::Serialize;
use strum::VariantArray as _;
use walkdir::WalkDir;

use super::{Adapter, AdapterOutput, CompileCtx, ProviderAdapter, RemoveCtx, SyncDestinationMode};
use crate::compile::{AdapterConfig, EmittedHookEntry, GeneratedFile, HookEmitMode};
use crate::plan::{
    ConfigPatch, FileKind, PatchBridge, PostWriteHook, RemovePatchReport, expand_tilde,
};
use crate::presets::ProviderPresetsMap;
use crate::provider::Provider;
use crate::spec::{AgentSpec, RuleSpec, SkillSpec, Spec, ToolFrontmatter};

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

// See: https://opencode.ai/docs/commands/#markdown
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
struct OpenCodeCommandFrontmatter {
    description: String,
    model: Option<String>,
}

// See: https://opencode.ai/docs/skills/#write-frontmatter
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
struct OpenCodeSkillFrontmatter {
    name: String,
    description: String,
    model: Option<String>,
    variant: Option<String>,
    tools: IndexMap<String, bool>,
}

/// Filename of `OpenCode`'s host config under each provider's config dir.
/// Single source of truth shared by the bridge `compile`/`removal_patches`
/// constructors and by `patch_opencode_instructions` /
/// `remove_opencode_instructions` at run time.
const HOST_FILENAME: &str = "opencode.json";

/// Zero-sized adapter for the `OpenCode` provider.
#[derive(Debug)]
pub struct OpenCodeAdapter;

impl Adapter for OpenCodeAdapter {
    fn compile(&self, specs: &[Spec], ctx: &CompileCtx<'_>) -> Result<AdapterOutput> {
        let mut files = Vec::new();
        for spec in specs {
            let mut adapted =
                ProviderAdapter::adapt(self, spec.clone(), ctx.presets, ctx.adapter_config)?;
            files.append(&mut adapted);
        }

        let owned_entries: Vec<EmittedHookEntry> = Vec::new();
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
                let host_path = dest_root.join(HOST_FILENAME);
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
                let host_path = dest_root.join(HOST_FILENAME);
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

    fn file_kinds(&self) -> &'static [FileKind] {
        ProviderAdapter::file_kinds(self)
    }

    fn remove_dest_root(&self, ctx: &RemoveCtx<'_>) -> PathBuf {
        ProviderAdapter::config_dir(
            self,
            ctx.mode,
            ctx.target_dir.and_then(Path::to_str),
            ctx.home,
            ctx.cwd,
        )
    }
}

impl ProviderAdapter for OpenCodeAdapter {
    fn adapt(
        &self,
        spec: Spec,
        presets: &ProviderPresetsMap,
        cfg: Option<&AdapterConfig>,
    ) -> Result<Vec<GeneratedFile>> {
        match spec {
            Spec::Agent(s) => adapt_agent_spec(s, presets, cfg),
            Spec::Skill(s) => adapt_skill_spec(s, presets, cfg),
            Spec::Rule(s) => Ok(adapt_rule_spec(&s, cfg)),
            // hooks are not emitted for OpenCode in v1; the per-provider warning
            // is surfaced from `run_compile` via `CompileDiagnostics::skipped_hooks`.
            Spec::Hook(_) => Ok(Vec::new()),
        }
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
            ToolFrontmatter::Bash => "bash",
            ToolFrontmatter::WebFetch => "webfetch",
            ToolFrontmatter::WebSearch => "websearch",
            ToolFrontmatter::Question => "question",
            ToolFrontmatter::Tasks => "todowrite",
            ToolFrontmatter::Subagent => "task",
            ToolFrontmatter::Skill => "skill",
        }
    }

    /// Returns the name the AI model uses to reference this spec.
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
    fn model_facing_name(&self, spec: &Spec, cfg: Option<&AdapterConfig>) -> String {
        let id = spec.id();
        match spec {
            Spec::Agent(_) => match cfg.and_then(AdapterConfig::content_prefix) {
                Some(prefix) => format!("{prefix}{id}"),
                None => id.to_owned(),
            },
            Spec::Skill(_) | Spec::Rule(_) | Spec::Hook(_) => id.to_owned(),
        }
    }

    /// `emit_mode`, `owned_entries`, and `_overwrite` are accepted for
    /// signature symmetry with the Claude/Cursor factories — `OpenCode` does
    /// not emit hooks in v1, so they are unused here. Keeping the signatures
    /// aligned lets the trait dispatch uniformly per provider.
    fn post_write_hook(
        &self,
        kind: FileKind,
        dest: &Path,
        config_dir: &Path,
        _emit_mode: HookEmitMode,
        _owned_entries: &[EmittedHookEntry],
        _overwrite: bool,
    ) -> Option<Box<dyn PostWriteHook>> {
        if kind != FileKind::Rules {
            return None;
        }
        Some(Box::new(OpenCodeInstructionsPatch {
            rules_dest_dir: dest.to_path_buf(),
            config_dir: config_dir.to_path_buf(),
        }))
    }

    /// Factory for `OpenCode`'s remove post-write hook.
    ///
    /// Signature mirrors Claude/Cursor's `remove_post_write_hook`. `_emit_mode`
    /// is accepted for symmetry — `OpenCode` doesn't have a merged-vs-bundled
    /// split for `instructions[]`; `opencode.json` is always the host file.
    /// Returns `Some` for `FileKind::Rules`, `None` otherwise.
    fn remove_post_write_hook(
        &self,
        kind: FileKind,
        dest: &Path,
        config_dir: &Path,
        _emit_mode: HookEmitMode,
    ) -> Option<Box<dyn PostWriteHook>> {
        if kind != FileKind::Rules {
            return None;
        }
        Some(Box::new(OpenCodeRemoveInstructionsPatch {
            rules_dest_dir: dest.to_path_buf(),
            config_dir: config_dir.to_path_buf(),
        }))
    }

    fn file_kinds(&self) -> &'static [FileKind] {
        &[
            FileKind::Agents,
            FileKind::Commands,
            FileKind::Rules,
            FileKind::Skills,
        ]
    }

    fn user_dest_dir(&self, home: &Path, kind: FileKind) -> PathBuf {
        home.join(".config").join("opencode").join(kind.dir_name())
    }

    fn project_dest_dir(&self, cwd: &Path, kind: FileKind) -> PathBuf {
        cwd.join(".opencode").join(kind.dir_name())
    }

    fn config_dir(
        &self,
        mode: SyncDestinationMode,
        dir: Option<&str>,
        home: &Path,
        cwd: &Path,
    ) -> PathBuf {
        match mode {
            SyncDestinationMode::User => home.join(".config").join("opencode"),
            SyncDestinationMode::Project => cwd.join(".opencode"),
            SyncDestinationMode::Path => dir.map_or_else(
                || home.join(".config").join("opencode"),
                |d| expand_tilde(d, home),
            ),
        }
    }
}

fn adapt_agent_spec(
    spec: AgentSpec,
    presets: &ProviderPresetsMap,
    cfg: Option<&AdapterConfig>,
) -> Result<Vec<GeneratedFile>> {
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

    Ok(vec![GeneratedFile::text(
        Provider::OpenCode,
        Path::new("agents").join(format!("{file_prefix}{id}.md")),
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

    let tools: Vec<ToolFrontmatter> = spec
        .frontmatter
        .capabilities
        .and_then(|x| x.tools)
        .into_iter()
        .flatten()
        .collect();

    let tools = build_tool_map(&tools);

    let body = spec.body;
    let supporting_files = spec.supporting_files;

    let mut files = Vec::new();

    if user_invocable {
        // OpenCode commands: prefix becomes a subdirectory, not a file prefix
        let cmd_path = match cfg.and_then(|c| c.prefix.as_deref()) {
            Some(prefix) => Path::new("commands").join(prefix).join(format!("{id}.md")),
            None => Path::new("commands").join(format!("{id}.md")),
        };

        let frontmatter = OpenCodeCommandFrontmatter {
            description: description.clone(),
            model: model.clone(),
        };
        let frontmatter_str = serde_yml::to_string(&frontmatter)?;
        let content = format!("---\n{frontmatter_str}---\n\n{}", body.trim());
        files.push(GeneratedFile::text(Provider::OpenCode, cmd_path, content));
    }

    if agent_invocable {
        let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();

        let frontmatter = OpenCodeSkillFrontmatter {
            name: id.clone(),
            description,
            model,
            variant,
            tools,
        };
        let frontmatter_str = serde_yml::to_string(&frontmatter)?;
        let content = format!("---\n{frontmatter_str}---\n\n{}", body.trim());

        let skill_dir = Path::new("skills").join(format!("{file_prefix}{id}"));

        files.push(GeneratedFile::text(
            Provider::OpenCode,
            skill_dir.join("SKILL.md"),
            content,
        ));

        for sf in supporting_files {
            files.push(GeneratedFile::binary(
                Provider::OpenCode,
                skill_dir.join(&sf.relative_path),
                sf.content,
                Some(sf.mode),
            ));
        }
    }

    Ok(files)
}

fn adapt_rule_spec(spec: &RuleSpec, cfg: Option<&AdapterConfig>) -> Vec<GeneratedFile> {
    let content = format!("{}\n", spec.body.trim());
    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let path = Path::new("rules")
        .join(format!("{file_prefix}{}", spec.frontmatter.id))
        .join("AGENTS.md");

    vec![GeneratedFile::text(Provider::OpenCode, path, content)]
}

/// Post-write hook that patches `opencode.json` instructions with rule file paths.
#[derive(Debug)]
pub struct OpenCodeInstructionsPatch {
    rules_dest_dir: PathBuf,
    config_dir: PathBuf,
}

impl PostWriteHook for OpenCodeInstructionsPatch {
    fn run(&self, dry_run: bool) -> Result<()> {
        patch_opencode_instructions(&self.rules_dest_dir, &self.config_dir, dry_run)
    }
}

/// Post-write hook that strips `instructions[]` entries pointing into
/// agentspec's rules dest dir.
///
/// Mirrors [`OpenCodeInstructionsPatch`] but inverted: instead of appending
/// the current set of rules, it filters out everything whose path starts
/// with `rules_dest_dir`. If `instructions[]` becomes `[]`, the key is
/// dropped; if the residual file is then `{}` AND tidy actually removed
/// at least one agentspec entry, the host file (`opencode.json`) is
/// deleted and its parent directory best-effort `rmdir`'d. User-authored
/// top-level keys (e.g. `model`) keep the file alive.
///
/// Trivia preservation: parses, mutates, and writes via `jsonc-parser`'s CST
/// so user-authored comments, key ordering, trailing commas, and formatting
/// whitespace round-trip across remove cycles.
#[derive(Debug)]
pub struct OpenCodeRemoveInstructionsPatch {
    rules_dest_dir: PathBuf,
    config_dir: PathBuf,
}

impl PostWriteHook for OpenCodeRemoveInstructionsPatch {
    fn run(&self, dry_run: bool) -> Result<()> {
        let report = remove_opencode_instructions(&self.rules_dest_dir, &self.config_dir, dry_run)?;
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
/// Short-circuits when no agentspec entries are present (so a no-op cycle
/// doesn't bump the file's mtime). Emits a "would tidy …" line under
/// `dry_run` for parity with the sync-side patcher and the Claude/Cursor
/// remove patches.
///
/// Trivia preservation: parses, mutates, and writes via `jsonc-parser`'s CST
/// so user-authored comments, key ordering, trailing commas, and formatting
/// whitespace round-trip across remove cycles.
fn remove_opencode_instructions(
    rules_dest_dir: &Path,
    config_dir: &Path,
    dry_run: bool,
) -> Result<RemovePatchReport> {
    let config_path = config_dir.join(HOST_FILENAME);

    if !config_path.exists() {
        return Ok(RemovePatchReport::default());
    }

    let content = crate::cst_io::read_or_empty_object(&config_path)?;
    let root = CstRootNode::parse(&content, &ParseOptions::default())
        .with_context(|| format!("failed to parse {}", config_path.display()))?;

    let Some(top) = root.object_value_or_create() else {
        let prefix = if dry_run { "[dry-run] " } else { "" };
        eprintln!(
            "{prefix}warning: {} has a non-object root; skipping tidy",
            config_path.display()
        );
        return Ok(RemovePatchReport {
            host_path: config_path,
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
    // the delete-on-empty predicate below — anything that reaches the
    // `top.properties().is_empty()` check is guaranteed to have removed at
    // least one agentspec entry, mirroring the Claude/Cursor `removed_owned > 0` gate.
    if agentspec_removed == 0 {
        return Ok(RemovePatchReport {
            host_path: config_path,
            user_entries_remaining,
            host_file_deleted: false,
            parent_rmdir: false,
        });
    }

    // Delete-on-empty: at least one agentspec entry was removed AND no other
    // top-level keys survive. `delete_host_file_and_rmdir_parent` respects
    // `dry_run` internally, so this branch covers both dry and live runs.
    if top.properties().is_empty() {
        let parent_rmdir = crate::plan::delete_host_file_and_rmdir_parent(&config_path, dry_run)?;
        return Ok(RemovePatchReport {
            host_path: config_path,
            user_entries_remaining: 0,
            host_file_deleted: true,
            parent_rmdir,
        });
    }

    if dry_run {
        eprintln!(
            "[dry-run] would tidy {agentspec_removed} agentspec instruction(s) from {}",
            config_path.display()
        );
        return Ok(RemovePatchReport {
            host_path: config_path,
            user_entries_remaining,
            host_file_deleted: false,
            parent_rmdir: false,
        });
    }

    crate::cst_io::finish(&root, &config_path)?;

    Ok(RemovePatchReport {
        host_path: config_path,
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
///
/// Returns the count of removed agentspec entries (for the no-op
/// short-circuit) and the count of user-authored entries that survived (for
/// the report).
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
///
/// Note: `OpenCode` is transitioning from the per-agent `tools:` map to a `permissions`
/// system; `task` (subagent dispatch) appears there rather than in the tools docs.
/// See <https://opencode.ai/docs/permissions/>.
fn build_tool_map(tools: &[ToolFrontmatter]) -> IndexMap<String, bool> {
    let mut map: IndexMap<String, bool> = ToolFrontmatter::VARIANTS
        .iter()
        .map(|t| {
            (
                ProviderAdapter::body_tool_name(&OpenCodeAdapter, t).to_string(),
                false,
            )
        })
        .collect();

    for tool in tools {
        map.insert(
            ProviderAdapter::body_tool_name(&OpenCodeAdapter, tool).to_string(),
            true,
        );
    }

    map.sort_keys();

    map
}

/// Shared ownership predicate: returns `true` if `entry_path` (a string from
/// `opencode.json`'s `instructions[]`) belongs to agentspec.
///
/// Used by both [`patch_opencode_instructions`] (sync, write side) and
/// [`remove_opencode_instructions`] (read side) so any future change to path
/// representation must update both call sites at once. Without this seam, a
/// switch to absolute / canonicalized / tilde-prefixed paths on the write side
/// would silently leave entries behind on remove.
fn is_agentspec_instruction(entry_path: &str, rules_dest_dir: &Path) -> bool {
    Path::new(entry_path).starts_with(rules_dest_dir)
}

/// Patches the `instructions` array in `config_dir/opencode.json`.
///
/// Ownership contract: agentspec owns any entry whose path falls under `rules_dest_dir`.
/// On each sync those entries are replaced wholesale; all other entries are preserved.
///
/// If `opencode.json` does not exist, it is created with just the `instructions` key.
///
/// When `dry_run` is true, prints the planned diff but does not write the file.
///
/// Trivia preservation: parses, mutates, and writes via `jsonc-parser`'s CST so
/// user-authored comments, key ordering, trailing commas, and formatting
/// whitespace round-trip across sync cycles.
fn patch_opencode_instructions(
    rules_dest_dir: &Path,
    config_dir: &Path,
    dry_run: bool,
) -> Result<()> {
    let config_path = config_dir.join(HOST_FILENAME);

    let mut new_rule_paths: Vec<String> = if rules_dest_dir.is_dir() {
        WalkDir::new(rules_dest_dir)
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
    new_rule_paths.sort();

    // Skip writing entirely when the file doesn't exist yet and there's nothing
    // to record. Avoids creating a spurious `opencode.json` when no rules have
    // ever been synced.
    if !config_path.exists() && new_rule_paths.is_empty() {
        return Ok(());
    }

    let content = crate::cst_io::read_or_empty_object(&config_path)?;
    let root = CstRootNode::parse(&content, &ParseOptions::default())
        .with_context(|| format!("failed to parse {}", config_path.display()))?;

    let Some(top) = root.object_value_or_create() else {
        // Behavior change vs. the prior serde_json implementation: that one
        // silently wrote the unmodified non-object value back. Aligning with
        // the remove path's existing warn-and-no-op contract here.
        let prefix = if dry_run { "[dry-run] " } else { "" };
        eprintln!(
            "{prefix}warning: {} has a non-object root; skipping patch",
            config_path.display()
        );
        return Ok(());
    };

    if dry_run {
        eprintln!(
            "[dry-run] would write {} instructions to {}",
            new_rule_paths.len(),
            config_path.display()
        );
        return Ok(());
    }

    rewrite_instructions(&top, rules_dest_dir, &new_rule_paths);

    crate::cst_io::finish(&root, &config_path)
}

/// Drop agentspec-owned string entries from `instructions[]`, preserving
/// user-authored strings and any non-string elements verbatim. Append
/// `new_paths` as fresh string entries. If `instructions[]` is absent, insert
/// it as a new property when `new_paths` is non-empty.
///
/// Non-array existing values (e.g., `instructions: null`) are replaced with a
/// fresh array via `array_value_or_set` when `new_paths` is non-empty;
/// otherwise the value is left alone (a small refinement over the prior
/// `serde_json` behavior, which would write `[]` over a `null`).
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
    use crate::spec::{AgentFrontmatter, AgentSpec, SkillFrontmatter, SkillSpec};

    #[test]
    fn test_body_tool_name_tasks_maps_to_todowrite() {
        assert_eq!(
            ProviderAdapter::body_tool_name(&OpenCodeAdapter, &ToolFrontmatter::Tasks),
            "todowrite"
        );
    }

    #[test]
    fn test_body_tool_name_subagent_maps_to_task() {
        assert_eq!(
            ProviderAdapter::body_tool_name(&OpenCodeAdapter, &ToolFrontmatter::Subagent),
            "task"
        );
    }

    #[test]
    fn test_body_tool_name_skill_identity() {
        assert_eq!(
            ProviderAdapter::body_tool_name(&OpenCodeAdapter, &ToolFrontmatter::Skill),
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

        let files = OpenCodeAdapter
            .adapt(spec, &HashMap::new(), None)
            .expect("expected value");
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

    #[test]
    fn test_adapt_skill_command_with_prefix_uses_subdirectory() {
        let cfg = AdapterConfig {
            prefix: Some("tw".to_string()),
            content_prefix: None,
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
            supporting_files: vec![],
        });

        let files = OpenCodeAdapter
            .adapt(spec, &HashMap::new(), Some(&cfg))
            .expect("expected value");
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].path.to_str(),
            Some("commands/tw/basic-skill.md"),
            "OpenCode commands should use prefix as subdirectory"
        );
    }

    // patch_opencode_instructions tests

    #[test]
    fn test_patch_no_prior_config_creates_file() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir_all(rules_dir.join("my-rule")).expect("expected value");
        fs::write(rules_dir.join("my-rule/AGENTS.md"), "rule").expect("expected value");

        patch_opencode_instructions(&rules_dir, tmp.path(), false).expect("expected value");

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

        patch_opencode_instructions(&rules_dir, tmp.path(), false).expect("expected value");

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

        patch_opencode_instructions(&rules_dir, tmp.path(), false).expect("expected value");

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

        patch_opencode_instructions(&rules_dir, tmp.path(), false).expect("expected value");

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

        patch_opencode_instructions(&rules_dir, tmp.path(), true).expect("expected value");

        assert!(
            !tmp.path().join("opencode.json").exists(),
            "dry_run must not create file"
        );
    }

    // -----------------------------------------------------------------------
    // remove_opencode_instructions tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_remove_opencode_missing_file_is_no_op() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        let report = remove_opencode_instructions(&rules_dir, tmp.path(), false).expect("ok");
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

        let report = remove_opencode_instructions(&rules_dir, tmp.path(), false).expect("ok");
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
        // The host file is deleted when (a) tidy actually removed at least one
        // agentspec entry, and (b) no other top-level keys survive. The
        // parent directory is best-effort rmdir'd as well.
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

        let report = remove_opencode_instructions(&rules_dir, &parent, false).expect("ok");

        assert!(
            !config_path.exists(),
            "host file should be deleted when only agentspec instructions were present"
        );
        assert!(
            !parent.exists(),
            "parent dir should be rmdir'd when it becomes empty after host-file delete"
        );
        assert!(report.host_file_deleted);
        assert!(report.parent_rmdir);
        assert_eq!(report.user_entries_remaining, 0);
    }

    #[test]
    fn test_remove_opencode_keeps_file_when_user_top_level_keys_remain() {
        // A user-authored top-level key (e.g. `model`) keeps the host file
        // alive — only the `instructions` key is dropped.
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

        let report = remove_opencode_instructions(&rules_dir, tmp.path(), false).expect("ok");

        assert!(
            config_path.exists(),
            "host file must survive when user-authored top-level keys remain"
        );
        assert!(!report.host_file_deleted);
        assert!(!report.parent_rmdir);

        let content = std::fs::read_to_string(&config_path).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse");
        assert!(
            parsed.get("instructions").is_none(),
            "instructions key should be dropped when array empties"
        );
        assert_eq!(
            parsed.get("model").and_then(|v| v.as_str()),
            Some("haiku"),
            "user-authored top-level key must round-trip"
        );
    }

    #[test]
    fn test_remove_opencode_dry_run_does_not_delete_or_rmdir() {
        // Dry-run must not touch the filesystem, but the returned report
        // should still carry `host_file_deleted: true` so the implementer can
        // see what the live run would do.
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

        let report = remove_opencode_instructions(&rules_dir, &parent, true).expect("ok");

        // File still exists, content unchanged.
        assert!(
            config_path.exists(),
            "dry-run must not delete the host file"
        );
        let post = std::fs::read_to_string(&config_path).expect("read");
        assert_eq!(post, initial_serialized, "dry-run must not write");
        // Parent untouched.
        assert!(parent.exists(), "dry-run must not rmdir the parent");
        // Report reflects the would-be outcome.
        assert!(
            report.host_file_deleted,
            "dry-run report should still carry host_file_deleted: true"
        );
        assert_eq!(report.user_entries_remaining, 0);
    }

    #[test]
    fn test_remove_opencode_no_op_when_no_agentspec_entries_present() {
        // Pre-existing config with only user entries should not be rewritten —
        // mtime stays unchanged.
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

        // Tiny delay so a stray rewrite would produce a detectably-newer mtime.
        std::thread::sleep(std::time::Duration::from_millis(10));

        let report = remove_opencode_instructions(&rules_dir, tmp.path(), false).expect("ok");
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

        remove_opencode_instructions(&rules_dir, tmp.path(), false).expect("ok");

        let content = std::fs::read_to_string(&config_path).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse");
        assert_eq!(
            parsed.get("model").and_then(|v| v.as_str()),
            Some("haiku"),
            "top-level keys other than `instructions` must round-trip"
        );
    }

    #[test]
    fn test_file_kinds_includes_commands() {
        assert!(Adapter::file_kinds(&OpenCodeAdapter).contains(&FileKind::Commands));
    }

    #[test]
    fn test_user_dest_dir_is_xdg_style() {
        let result = OpenCodeAdapter.user_dest_dir(Path::new("/home/user"), FileKind::Skills);
        assert_eq!(result, PathBuf::from("/home/user/.config/opencode/skills"));
    }

    #[test]
    fn test_project_dest_dir_is_flat() {
        let result = OpenCodeAdapter.project_dest_dir(Path::new("/work/project"), FileKind::Agents);
        assert_eq!(result, PathBuf::from("/work/project/.opencode/agents"));
    }

    #[test]
    fn test_patch_preserves_comments_and_trivia() {
        // JSONC `opencode.json` with a comment and a `model` key authored before
        // `instructions`. After patching, the comment, the `model` value, and
        // the original key order must round-trip.
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

        patch_opencode_instructions(&rules_dir, tmp.path(), false).expect("patch");

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
        assert!(
            model_pos < instructions_pos,
            "model must precede instructions; got:\n{after}"
        );
        assert!(
            after.contains("my-rule"),
            "agentspec rule must be appended, got:\n{after}"
        );
    }

    #[test]
    fn test_patch_preserves_user_top_level_key_ordering() {
        // User authored `model` and `permissions` before `instructions`. After
        // sync, both keys round-trip in their original order with original
        // formatting.
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

        patch_opencode_instructions(&rules_dir, tmp.path(), false).expect("patch");

        let after = fs::read_to_string(&config_path).expect("read");
        let model_pos = after.find("\"model\"").expect("model");
        let permissions_pos = after.find("\"permissions\"").expect("permissions");
        let instructions_pos = after.find("\"instructions\"").expect("instructions");
        assert!(
            model_pos < permissions_pos && permissions_pos < instructions_pos,
            "key ordering must round-trip; got:\n{after}"
        );
        assert!(
            after.contains("\"haiku\"")
                && after.contains("\"read\"")
                && after.contains("\"write\""),
            "user values must round-trip, got:\n{after}"
        );
        assert!(
            after.contains("~/notes/personal.md"),
            "user instruction entry must round-trip, got:\n{after}"
        );
    }

    #[test]
    fn test_remove_preserves_comments_and_trivia() {
        // Seed with a comment, a user-authored `model` key, and a single
        // agentspec entry under `instructions`. After remove, the comment,
        // `model`, and the original formatting must survive; `instructions`
        // is dropped because the array empties.
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        let config_path = tmp.path().join("opencode.json");

        let agentspec_path = rules_dir.join("a/AGENTS.md");
        let initial = format!(
            "{{\n  // user note\n  \"model\": \"haiku\",\n  \"instructions\": [\"{}\"]\n}}\n",
            agentspec_path.display()
        );
        fs::write(&config_path, &initial).expect("write initial");

        remove_opencode_instructions(&rules_dir, tmp.path(), false).expect("remove");

        let after = fs::read_to_string(&config_path).expect("read");
        assert!(
            after.contains("// user note"),
            "comment must round-trip, got:\n{after}"
        );
        assert!(
            after.contains("\"model\": \"haiku\""),
            "model value must round-trip with original formatting, got:\n{after}"
        );
        assert!(
            !after.contains("\"instructions\""),
            "instructions key should be dropped when array empties, got:\n{after}"
        );
    }

    #[test]
    fn test_patch_idempotent_round_trip() {
        // Calling patch twice with the same inputs must produce byte-identical
        // output. Mirrors hooks_merge::test_merge_idempotent_round_trip.
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir_all(rules_dir.join("r")).expect("mkdir rule");
        fs::write(rules_dir.join("r/AGENTS.md"), "rule").expect("write rule");

        patch_opencode_instructions(&rules_dir, tmp.path(), false).expect("patch 1");
        let after_1 = fs::read_to_string(tmp.path().join("opencode.json")).expect("read 1");

        patch_opencode_instructions(&rules_dir, tmp.path(), false).expect("patch 2");
        let after_2 = fs::read_to_string(tmp.path().join("opencode.json")).expect("read 2");

        assert_eq!(after_1, after_2, "patch must be idempotent");
    }

    #[test]
    fn test_patch_handles_empty_file() {
        // A zero-byte `opencode.json` must be treated as `{}` rather than
        // failing the parser. With at least one rule under `rules_dest_dir`,
        // the function proceeds past the short-circuit and creates a valid
        // JSON object with the agentspec entry in `instructions[]`.
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir_all(rules_dir.join("r")).expect("mkdir rule");
        fs::write(rules_dir.join("r/AGENTS.md"), "rule").expect("write rule");

        let config_path = tmp.path().join("opencode.json");
        fs::write(&config_path, "").expect("touch empty file");

        patch_opencode_instructions(&rules_dir, tmp.path(), false)
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
                .contains("r/AGENTS.md"),
            "agentspec rule path expected, got: {arr:?}"
        );
    }

    #[test]
    fn test_patch_warns_on_non_object_root() {
        // A top-level array root should trigger the warn-and-no-op path:
        // returns Ok, prints a warning, and leaves the file byte-identical.
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir_all(rules_dir.join("r")).expect("mkdir rule");
        fs::write(rules_dir.join("r/AGENTS.md"), "rule").expect("write rule");

        let config_path = tmp.path().join("opencode.json");
        let initial = "[]";
        fs::write(&config_path, initial).expect("write initial");

        patch_opencode_instructions(&rules_dir, tmp.path(), false)
            .expect("patch returns Ok on non-object root");

        let after = fs::read_to_string(&config_path).expect("read");
        assert_eq!(
            after, initial,
            "non-object root must round-trip byte-identical (no rewrite to {{}})"
        );
    }

    #[test]
    fn test_remove_warns_on_non_object_root() {
        // Symmetric for the remove path: non-object root → warn, return empty
        // report, leave file byte-identical.
        let tmp = tempfile::tempdir().expect("tmp");
        let rules_dir = tmp.path().join("rules");
        let config_path = tmp.path().join("opencode.json");
        let initial = "[]";
        fs::write(&config_path, initial).expect("write initial");

        let report = remove_opencode_instructions(&rules_dir, tmp.path(), false)
            .expect("remove returns Ok on non-object root");

        assert!(!report.host_file_deleted);
        assert_eq!(report.user_entries_remaining, 0);

        let after = fs::read_to_string(&config_path).expect("read");
        assert_eq!(
            after, initial,
            "non-object root must round-trip byte-identical"
        );
    }
}
