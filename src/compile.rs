use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::adapters::{CompileCtx, SyncDestinationMode};
use crate::plan::{ConfigPatch, FileKind};
use crate::presets::ProviderPresetsMap;
use crate::provider::Provider;
use crate::spec::{HookEvent, Spec};
use crate::specs::ValidatedSpecs;
use crate::templating::{TemplateContext, TemplatingResources, resolve_fragments};

/// Per-provider configuration passed to adapters at compile time.
///
/// Adapters use this to apply prefix/strip transforms to output paths and
/// frontmatter fields. When `None` is passed, adapters produce canonical
/// (unprefixed, unstripped) output.
#[derive(Clone, Debug, Default)]
pub struct AdapterConfig {
    /// Namespace prefix for file paths and frontmatter names.
    pub prefix: Option<String>,
    /// Literal prefix for content/model-facing names (e.g., `"tw:"` → `"tw:{id}"`).
    /// When `None`, `content_prefix()` falls back to `"{prefix}-"`.
    pub content_prefix: Option<String>,
    /// Plugin manifest fields supplied by the binary's `SyncTargetConfig`.
    ///
    /// `Some(_)` only when the user configured `mode = "plugin"` with at least
    /// `plugin-name` set. The Claude adapter emits `.claude-plugin/plugin.json`
    /// from this struct when `ctx.mode == SyncDestinationMode::Plugin` and this
    /// is `Some`; Cursor's `.cursor-plugin/plugin.json` is conditional on the
    /// same predicate, with absence meaning "no manifest file emitted".
    pub plugin_manifest: Option<PluginManifest>,
}

/// Plugin manifest fields shared across providers.
///
/// `name` is required (validated at config-resolve time when `mode = "plugin"`);
/// the other fields are passthrough text the adapter serializes into the
/// provider-appropriate `plugin.json` shape.
#[derive(Clone, Debug)]
pub struct PluginManifest {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub author: Option<PluginAuthor>,
}

/// Author sub-record for plugin manifests.
///
/// Both Claude and Cursor accept an object shape (`{ name, email? }`); v1
/// emits the name-only shape. Email support is deferred (see TODO #17 in
/// `agentspec/TODO.md`).
#[derive(Clone, Debug)]
pub struct PluginAuthor {
    pub name: String,
}

/// How a provider's hook entries should reach disk.
///
/// `Bundled` means agentspec writes a self-contained `hooks.json` (Path mode
/// or `compile`-only). `MergedUser` and `MergedProject` mean entries are
/// merged into a host config file (`settings.json` for Claude, `hooks.json`
/// for Cursor) via a post-write patcher.
///
/// User vs. Project is split because the script command path-anchoring
/// differs: User mode emits `$HOME/.claude/hooks/scripts/<f>`, Project mode
/// emits `${CLAUDE_PROJECT_DIR}/.claude/hooks/scripts/<f>`. Per-mode anchoring
/// isn't derivable from a single Merged variant, so each mode gets its own
/// enum variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HookEmitMode {
    Bundled,
    MergedUser,
    MergedProject,
}

impl HookEmitMode {
    /// Whether this mode requires the post-write merge patcher.
    pub fn is_merged(self) -> bool {
        matches!(self, Self::MergedUser | Self::MergedProject)
    }
}

/// A single canonical hook entry, computed once per provider during compile.
///
/// Both the bundled-mode `synthesize_hooks` path (which writes a
/// self-contained `hooks.json`) and the merged-mode CST-aware merge layer
/// consume the same shape.
/// Provider-specific JSON wrapping (Claude grouping by matcher vs. Cursor's
/// per-entry matcher) is applied at JSON-emission time, not stored here.
#[derive(Clone, Debug)]
pub struct EmittedHookEntry {
    pub event: HookEvent,
    pub matcher: Option<String>,
    pub command: String,
    pub timeout: Option<u32>,
    /// Stable identifier emitted as `_agentspec_id` in the JSON output —
    /// the sentinel the merged-mode merge layer uses to identify owned entries.
    pub agentspec_id: String,
}

/// What an adapter's `synthesize_hooks` step returns.
///
/// `entries` is always populated (so the merged-mode patcher can consume
/// them regardless of emit mode). `files` carries the full bundle for
/// `HookEmitMode::Bundled` — both `hooks/hooks.json` AND every file under
/// `hooks/scripts/`, since helpers (e.g., `_common.sh`) that an entry script
/// `source`s also need to land at the destination. Merged-mode emission goes
/// through the post-write patcher; `files` is empty in that mode.
#[derive(Debug, Default)]
pub struct HookSynthesis {
    pub entries: Vec<EmittedHookEntry>,
    pub files: Vec<GeneratedFile>,
}

impl AdapterConfig {
    /// Returns the file path prefix string (e.g., `"tw-"`), if a prefix is configured.
    /// Only used for filesystem paths — content references use `content_prefix()`.
    pub fn file_prefix(&self) -> Option<String> {
        self.prefix.as_ref().map(|p| format!("{p}-"))
    }

    /// Returns the content-reference prefix string, if any prefix is configured.
    ///
    /// When an explicit `content_prefix` is set (e.g., `"tw:"`), returns it directly.
    /// Otherwise falls back to `"{prefix}-"` (matching the file prefix format).
    /// Returns `None` when neither field is set.
    pub fn content_prefix(&self) -> Option<String> {
        self.content_prefix
            .clone()
            .or_else(|| self.prefix.as_ref().map(|p| format!("{p}-")))
    }
}

/// A single file produced by a provider adapter.
///
/// `kind` is set explicitly by each adapter's per-spec helpers — Agents for
/// `adapt_agent_spec`, Skills for `adapt_skill_spec`, Rules for
/// `adapt_rule_spec`, Hooks for the bundled `hooks/hooks.json` and every
/// supporting script, and Commands for `OpenCode`'s user-invocable skill
/// outputs. `sync_plan` partitions writes by `file.kind` directly rather
/// than re-deriving the kind from the leading path component.
#[derive(Clone, Debug)]
pub struct GeneratedFile {
    /// Provider that produced this file
    pub provider: Provider,
    /// Output kind (drives manifest-tracked write partitioning).
    pub kind: FileKind,
    /// Relative path from the provider root (e.g., "agents/foo.md", "skills/commit/SKILL.md")
    pub path: PathBuf,
    /// File content
    pub content: Vec<u8>,
    /// Optional file mode (e.g., 0o755 for executable scripts)
    pub mode: Option<u32>,
}

impl GeneratedFile {
    /// Create a text file with no special permissions.
    pub fn text(
        provider: Provider,
        kind: FileKind,
        path: impl AsRef<Path>,
        content: String,
    ) -> Self {
        Self {
            provider,
            kind,
            path: path.as_ref().to_path_buf(),
            content: content.into_bytes(),
            mode: None,
        }
    }

    /// Create a binary file, optionally with a mode.
    pub fn binary(
        provider: Provider,
        kind: FileKind,
        path: impl AsRef<Path>,
        content: Vec<u8>,
        mode: Option<u32>,
    ) -> Self {
        Self {
            provider,
            kind,
            path: path.as_ref().to_path_buf(),
            content,
            mode,
        }
    }
}

/// Per-provider compile-time context the binary supplies to the orchestrator.
///
/// Each `Adapter::compile` call needs to know its `mode`, optional `target_dir`,
/// and `--force`/`overwrite` flag in order to compute output paths and
/// construct post-write patches. These values originate in the binary's
/// `SyncTargetConfig`; this owned struct mirrors just the fields the library
/// needs, preserving the binary/library boundary established in `CLAUDE.md`'s
/// "Use config structs at module boundaries" guidance.
///
/// Providers absent from the orchestrator's target map default to
/// `SyncDestinationMode::Compile` with `target_dir: None` and
/// `overwrite: false` — appropriate for the `compile` command path which has
/// no sync destination.
#[derive(Clone, Debug)]
pub struct ProviderCompileTarget {
    pub mode: SyncDestinationMode,
    pub target_dir: Option<PathBuf>,
    pub overwrite: bool,
}

impl Default for ProviderCompileTarget {
    fn default() -> Self {
        Self {
            mode: SyncDestinationMode::Compile,
            target_dir: None,
            overwrite: false,
        }
    }
}

/// Result of compiling all specs for all target providers.
///
/// `files` carries every emitted file across all providers; `patches` carries
/// the per-provider post-write `ConfigPatch` instances that downstream
/// `sync_plan` drains; `dest_roots` records each provider's adapter-computed
/// sync destination root so `sync_plan` can anchor `ManifestTrackedWrite`
/// destinations without re-calling adapter path methods.
#[derive(Debug, Default)]
pub struct CompileResult {
    pub files: Vec<GeneratedFile>,
    pub patches: HashMap<Provider, Vec<Box<dyn ConfigPatch>>>,
    pub dest_roots: HashMap<Provider, PathBuf>,
}

impl CompileResult {
    /// Iterate over generated files for a specific provider.
    pub fn files_for(&self, provider: Provider) -> impl Iterator<Item = &GeneratedFile> {
        self.files.iter().filter(move |f| f.provider == provider)
    }

    /// Returns the adapter-computed sync destination root for `provider`, if
    /// the orchestrator recorded one (always populated after a successful
    /// `compile_specs` run).
    pub fn dest_root_for(&self, provider: Provider) -> Option<&Path> {
        self.dest_roots.get(&provider).map(PathBuf::as_path)
    }
}

/// Diagnostics surfaced from the compile stage that depend on the active
/// provider list (and so can't be computed during load).
///
/// Today this only carries hook-skip notifications for `OpenCode`; the
/// per-provider summary and per-spec listing are formatted by the binary.
/// Mirrors `LoadReport` in shape: each pipeline stage that produces stage-only
/// diagnostics owns its own report struct and returns it alongside its result.
#[derive(Debug, Default)]
pub struct CompileDiagnostics {
    pub skipped_hooks: Vec<SkippedHook>,
}

#[derive(Debug)]
pub struct SkippedHook {
    pub provider: Provider,
    pub hook_id: String,
}

/// Compile validated specs into provider-specific generated files.
///
/// Template resolution is performed internally: the `MiniJinja` environment is
/// built from `templating`, the context is constructed from the specs, and each
/// spec body is rendered before being handed to the provider adapters.
///
/// `compile_targets` carries per-provider sync-destination context that the
/// orchestrator turns into a `CompileCtx` for each `Adapter::compile` call.
/// Providers absent from the map use [`ProviderCompileTarget::default`]
/// (Path mode, no `target_dir`, no overwrite) — appropriate for the `compile`
/// command path which has no sync destination.
///
/// Returns the generated files alongside a [`CompileDiagnostics`] capturing
/// any compile-time anomalies (today: hook specs skipped for `OpenCode`).
// Eight params is over the clippy default of 7, but each carries a distinct
// stage-input concern (specs, templating, presets, providers, adapter configs,
// per-provider sync targets, home, cwd). Bundling them into a context struct
// would just rename the noise — see CLAUDE.md "config structs at module
// boundaries"; the boundary here is the public library API surface.
#[allow(clippy::too_many_arguments)]
pub fn run(
    validated: &ValidatedSpecs,
    templating: &TemplatingResources,
    presets: &ProviderPresetsMap,
    providers: &[Provider],
    adapter_configs: &HashMap<Provider, AdapterConfig>,
    compile_targets: &HashMap<Provider, ProviderCompileTarget>,
    home: &Path,
    cwd: &Path,
) -> Result<(CompileResult, CompileDiagnostics)> {
    compile_specs(
        validated.specs(),
        templating,
        presets,
        providers,
        adapter_configs,
        compile_targets,
        home,
        cwd,
    )
}

/// Takes `&[Spec]` (borrowed) even though `resolve_fragments` needs ownership —
/// the slice is cloned once per provider so that each provider gets its own
/// template-resolved copy with the correct prefix-aware names.
#[allow(clippy::too_many_arguments)] // mirrors `run` — see allow note there
pub(crate) fn compile_specs(
    specs: &[Spec],
    templating: &TemplatingResources,
    presets: &ProviderPresetsMap,
    providers: &[Provider],
    adapter_configs: &HashMap<Provider, AdapterConfig>,
    compile_targets: &HashMap<Provider, ProviderCompileTarget>,
    home: &Path,
    cwd: &Path,
) -> Result<(CompileResult, CompileDiagnostics)> {
    let mut files: Vec<GeneratedFile> = Vec::new();
    let mut patches: HashMap<Provider, Vec<Box<dyn ConfigPatch>>> = HashMap::new();
    let mut dest_roots: HashMap<Provider, PathBuf> = HashMap::new();
    let mut diagnostics = CompileDiagnostics::default();

    let default_target = ProviderCompileTarget::default();

    // `Provider` derives `Ord` and its variants are declared alphabetically
    // (Claude < Cursor < OpenCode), so `sort()` matches the lowercase-string
    // sort order without per-provider `String` allocations.
    let mut sorted_providers: Vec<Provider> = providers.to_vec();
    sorted_providers.sort();

    for &provider in &sorted_providers {
        // Per-provider template resolution stays in the orchestrator —
        // templating is provider-agnostic plumbing that should not be
        // duplicated across adapters.
        let env = templating.build_environment(Some(provider))?;
        let adapter_config = adapter_configs.get(&provider);
        let context = TemplateContext::from_specs_for_provider(specs, provider, adapter_config);
        let resolved = resolve_fragments(specs.to_vec(), &env, &context)?;

        let target = compile_targets.get(&provider).unwrap_or(&default_target);
        let ctx = CompileCtx {
            mode: target.mode,
            home,
            cwd,
            target_dir: target.target_dir.as_deref(),
            presets,
            adapter_config,
            overwrite: target.overwrite,
        };

        let output = provider.adapter().compile(&resolved, &ctx)?;
        files.extend(output.files);
        if !output.patches.is_empty() {
            patches.entry(provider).or_default().extend(output.patches);
        }
        dest_roots.insert(provider, output.dest_root);

        // Diagnostic post-pass: any `Spec::Hook` whose provider can't emit
        // hooks is recorded as a skip. Today only `OpenCode` falls through
        // here. The capability accessor lives on `Adapter` itself so the
        // orchestrator never branches on `Provider`.
        if !provider.adapter().emits_hooks() {
            for spec in &resolved {
                if matches!(spec, Spec::Hook(_)) {
                    diagnostics.skipped_hooks.push(SkippedHook {
                        provider,
                        hook_id: spec.id().to_string(),
                    });
                }
            }
        }
    }

    // Sort output files by path for deterministic ordering
    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok((
        CompileResult {
            files,
            patches,
            dest_roots,
        },
        diagnostics,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_prefix_returns_explicit_value() {
        let cfg = AdapterConfig {
            prefix: Some("tw".to_owned()),
            content_prefix: Some("tw:".to_owned()),
            ..AdapterConfig::default()
        };
        assert_eq!(cfg.content_prefix(), Some("tw:".to_owned()));
    }

    #[test]
    fn test_content_prefix_falls_back_to_prefix_with_hyphen() {
        let cfg = AdapterConfig {
            prefix: Some("tw".to_owned()),
            ..AdapterConfig::default()
        };
        assert_eq!(cfg.content_prefix(), Some("tw-".to_owned()));
    }

    #[test]
    fn test_content_prefix_returns_none_when_both_none() {
        let cfg = AdapterConfig::default();
        assert_eq!(cfg.content_prefix(), None);
    }

    #[test]
    fn test_file_prefix_unaffected_by_content_prefix() {
        let cfg = AdapterConfig {
            prefix: Some("tw".to_owned()),
            content_prefix: Some("tw:".to_owned()),
            ..AdapterConfig::default()
        };
        assert_eq!(cfg.file_prefix(), Some("tw-".to_owned()));
    }
}
