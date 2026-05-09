use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::presets::ProviderPresetsMap;
use crate::provider::Provider;
use crate::spec::{HookEvent, HookSpec, Spec};
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
    /// How hook entries should be emitted for this provider.
    ///
    /// `None` means "use canonical (Bundled) defaults" — appropriate for the
    /// `compile` command path when no `[sync.<provider>]` is configured. The
    /// binary crate translates `SyncMode → HookEmitMode` at the boundary so
    /// the library has no dependency on the binary's `SyncMode` type.
    pub hook_emit_mode: Option<HookEmitMode>,
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
#[derive(Clone, Debug)]
pub struct GeneratedFile {
    /// Provider that produced this file
    pub provider: Provider,
    /// Relative path from the provider root (e.g., "agents/foo.md", "skills/commit/SKILL.md")
    pub path: PathBuf,
    /// File content
    pub content: Vec<u8>,
    /// Optional file mode (e.g., 0o755 for executable scripts)
    pub mode: Option<u32>,
}

impl GeneratedFile {
    /// Create a text file with no special permissions.
    pub fn text(provider: Provider, path: impl AsRef<Path>, content: String) -> Self {
        Self {
            provider,
            path: path.as_ref().to_path_buf(),
            content: content.into_bytes(),
            mode: None,
        }
    }

    /// Create a binary file, optionally with a mode.
    pub fn binary(
        provider: Provider,
        path: impl AsRef<Path>,
        content: Vec<u8>,
        mode: Option<u32>,
    ) -> Self {
        Self {
            provider,
            path: path.as_ref().to_path_buf(),
            content,
            mode,
        }
    }
}

/// Result of compiling all specs for all target providers.
#[derive(Debug, Default)]
pub struct CompileResult {
    pub files: Vec<GeneratedFile>,
    /// Per-provider hook entries, populated whenever any `Spec::Hook`
    /// is in the input. Empty for providers that don't emit hooks (`OpenCode`)
    /// or when no hook specs exist. The merged-mode merge layer consumes this
    /// map directly so it doesn't have to re-parse the emitted JSON.
    pub hooks: HashMap<Provider, Vec<EmittedHookEntry>>,
}

impl CompileResult {
    /// Iterate over generated files for a specific provider.
    pub fn files_for(&self, provider: Provider) -> impl Iterator<Item = &GeneratedFile> {
        self.files.iter().filter(move |f| f.provider == provider)
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
/// Returns the generated files alongside a [`CompileDiagnostics`] capturing
/// any compile-time anomalies (today: hook specs skipped for `OpenCode`).
pub fn run(
    validated: &ValidatedSpecs,
    templating: &TemplatingResources,
    presets: &ProviderPresetsMap,
    providers: &[Provider],
    adapter_configs: &HashMap<Provider, AdapterConfig>,
) -> Result<(CompileResult, CompileDiagnostics)> {
    compile_specs(
        validated.specs(),
        templating,
        presets,
        providers,
        adapter_configs,
    )
}

/// Takes `&[Spec]` (borrowed) even though `resolve_fragments` needs ownership —
/// the slice is cloned once per provider so that each provider gets its own
/// template-resolved copy with the correct prefix-aware names.
pub(crate) fn compile_specs(
    specs: &[Spec],
    templating: &TemplatingResources,
    presets: &ProviderPresetsMap,
    providers: &[Provider],
    adapter_configs: &HashMap<Provider, AdapterConfig>,
) -> Result<(CompileResult, CompileDiagnostics)> {
    let mut files: Vec<GeneratedFile> = Vec::new();
    let mut hooks_map: HashMap<Provider, Vec<EmittedHookEntry>> = HashMap::new();
    let mut diagnostics = CompileDiagnostics::default();

    // `Provider` derives `Ord` and its variants are declared alphabetically
    // (Claude < Cursor < OpenCode), so `sort()` matches the lowercase-string
    // sort order without per-provider `String` allocations.
    let mut sorted_providers: Vec<Provider> = providers.to_vec();
    sorted_providers.sort();

    for &provider in &sorted_providers {
        let env = templating.build_environment(Some(provider))?;
        let adapter_config = adapter_configs.get(&provider);
        let context = TemplateContext::from_specs_for_provider(specs, provider, adapter_config);
        let resolved = resolve_fragments(specs.to_vec(), &env, &context)?;

        let mut hook_specs: Vec<&HookSpec> = Vec::new();
        let provider_emits_hooks = provider.hook_adapter().is_some();
        for spec in &resolved {
            let mut adapter_files =
                provider
                    .adapter()
                    .adapt(spec.clone(), presets, adapter_config)?;
            files.append(&mut adapter_files);

            if let Spec::Hook(h) = spec {
                hook_specs.push(h);
                if !provider_emits_hooks {
                    diagnostics.skipped_hooks.push(SkippedHook {
                        provider,
                        hook_id: spec.id().to_string(),
                    });
                }
            }
        }

        // Per-provider hook synthesis — runs once per provider with the full
        // hook list because `hooks.json` is one shared file and the `scripts/`
        // tree is shared across all hooks in the spec set. Providers without a
        // hook adapter (`OpenCode`) short-circuit to an empty `HookSynthesis`;
        // their per-spec skips are recorded in `diagnostics.skipped_hooks` above.
        let synthesis = provider.hook_adapter().map_or_else(
            || Ok(HookSynthesis::default()),
            |h| h.synthesize_hooks(&hook_specs, adapter_config),
        )?;
        files.extend(synthesis.files);
        if !synthesis.entries.is_empty() {
            hooks_map.insert(provider, synthesis.entries);
        }
    }

    // Sort output files by path for deterministic ordering
    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok((
        CompileResult {
            files,
            hooks: hooks_map,
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
