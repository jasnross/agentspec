use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::presets::ProviderPresetsMap;
use crate::provider::Provider;
use crate::spec::{HookEvent, NormalizedHookSpec, NormalizedSpec};
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

/// Build the per-provider `Vec<GeneratedFile>` for every file under
/// `spec/hooks/scripts/`, taken from the first hook spec.
///
/// `load_hook_specs` attaches the same `supporting_files` list to every hook
/// spec parsed from a single `hooks.toml`, so reading from `specs[0]` gives
/// the full set. Emitting once per provider here (rather than once per hook
/// in `adapt_hook_spec`) avoids duplicate file entries downstream.
pub fn build_hook_script_files(
    provider: Provider,
    specs: &[&NormalizedHookSpec],
) -> Vec<GeneratedFile> {
    let Some(first) = specs.first() else {
        return Vec::new();
    };
    first
        .supporting_files
        .iter()
        .map(|sf| {
            GeneratedFile::binary(
                provider,
                Path::new("hooks").join(&sf.relative_path),
                sf.content.clone(),
                Some(sf.mode),
            )
        })
        .collect()
}

/// Build canonical `EmittedHookEntry` rows from normalized hook specs.
///
/// The `command` field's anchor depends on `(provider, emit_mode)`:
/// - Bundled (Path mode): `${CLAUDE_PLUGIN_ROOT}/hooks/scripts/<f>` for both
///   providers (Cursor aliases `${CLAUDE_PLUGIN_ROOT}` at plugin scope).
/// - `MergedUser`: `$HOME/.<dotdir>/hooks/scripts/<f>` (`$HOME` not `~/...`
///   because Claude's hook-command runtime isn't documented to expand `~`).
/// - `MergedProject`: `${CLAUDE_PROJECT_DIR}/.<dotdir>/hooks/scripts/<f>`.
///   Cursor's behavior with `${CLAUDE_PROJECT_DIR}` outside plugin scope is
///   not documented — must be verified empirically against a real Cursor
///   build before 1.0.
pub fn build_emitted_hook_entries(
    specs: &[&NormalizedHookSpec],
    provider: Provider,
    emit_mode: HookEmitMode,
) -> Vec<EmittedHookEntry> {
    specs
        .iter()
        .map(|s| {
            // The frontmatter `script` is documented as relative to
            // `spec/hooks/` and validated to live under `scripts/` (see
            // `validate_hook_script_path` in specs.rs). Strip any leading
            // `./` and the required `scripts/` segment so the command
            // anchor — which already includes `hooks/scripts/` — preserves
            // any nested subdirectory layout (e.g., `git/pre-commit.sh`).
            // `Path::strip_prefix("scripts")` does not handle a leading
            // `./` component, so iterate components instead.
            use std::path::Component;
            let path_under_scripts: std::path::PathBuf = s
                .frontmatter
                .script
                .components()
                .skip_while(|c| matches!(c, Component::CurDir))
                .skip(1) // the "scripts" component, enforced by validation
                .collect();
            let filename = path_under_scripts.to_string_lossy().into_owned();
            EmittedHookEntry {
                event: s.frontmatter.event,
                matcher: s.frontmatter.matcher.clone(),
                command: hook_command_anchor(provider, emit_mode, &filename),
                timeout: s.frontmatter.timeout,
                agentspec_id: s.frontmatter.id.clone(),
            }
        })
        .collect()
}

/// Compute the `command` string for a hook entry given the provider and mode.
///
/// In Bundled (Path) mode, the host runtime sets `$CLAUDE_PLUGIN_ROOT`
/// (Cursor aliases it) to the plugin root, so we just reference the script
/// directly. In Merged (User/Project) modes, the host doesn't set that
/// variable — but hook scripts authored for the plugin distribution model
/// commonly reference `$CLAUDE_PLUGIN_ROOT/rules`, `$CLAUDE_PLUGIN_ROOT/skills`,
/// etc. to find sibling assets. We assign it inline (`FOO=bar cmd`, standard
/// POSIX) so plugin-shaped scripts keep working when synced project/user-wide.
/// The assigned value is the config dir (e.g., `$HOME/.claude` for User mode),
/// where agentspec also writes those sibling kinds.
///
/// `OpenCode` never reaches this helper — the per-provider dispatch in
/// `compile_specs` short-circuits to an empty `HookSynthesis` for it. The
/// `OpenCode` arm panics loudly via `unreachable!()` so a future refactor
/// that wires `OpenCode` through this path surfaces the mistake immediately
/// instead of silently emitting Claude-shaped paths under `.claude`.
fn hook_command_anchor(provider: Provider, emit_mode: HookEmitMode, filename: &str) -> String {
    if matches!(emit_mode, HookEmitMode::Bundled) {
        return format!("${{CLAUDE_PLUGIN_ROOT}}/hooks/scripts/{filename}");
    }
    let Some(hook_adapter) = provider.hook_adapter() else {
        unreachable!(
            "OpenCode short-circuits in compile_specs and never reaches hook_command_anchor"
        )
    };
    let dotdir = hook_adapter.hook_command_dotdir();
    let var_anchor = match emit_mode {
        HookEmitMode::Bundled => {
            unreachable!("Bundled returns early at the top of hook_command_anchor")
        }
        HookEmitMode::MergedUser => "$HOME",
        HookEmitMode::MergedProject => "${CLAUDE_PROJECT_DIR}",
    };
    let config_dir = format!("{var_anchor}/{dotdir}");
    format!("CLAUDE_PLUGIN_ROOT={config_dir} {config_dir}/hooks/scripts/{filename}")
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
    /// Per-provider hook entries, populated whenever any `NormalizedSpec::Hook`
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

/// Takes `&[NormalizedSpec]` (borrowed) even though `resolve_fragments` needs
/// ownership — the slice is cloned once per provider so that each provider gets
/// its own template-resolved copy with the correct prefix-aware names.
pub(crate) fn compile_specs(
    specs: &[NormalizedSpec],
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

        let mut hook_specs: Vec<&NormalizedHookSpec> = Vec::new();
        let provider_emits_hooks = provider.hook_adapter().is_some();
        for spec in &resolved {
            let mut adapter_files =
                provider
                    .adapter()
                    .adapt(spec.clone(), presets, adapter_config)?;
            files.append(&mut adapter_files);

            if let NormalizedSpec::Hook(h) = spec {
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
