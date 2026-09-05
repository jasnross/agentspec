use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::adapters::{CompileCtx, Degradation, Delivery, SyncDestinationMode};
use crate::plan::{FileKind, ForwardPatch};
use crate::presets::ProviderPresetsMap;
use crate::provider::Provider;
use crate::setting::SettingKey;
use crate::spec::{HookEvent, Spec};
use crate::specs::ValidatedSpecs;
use crate::templating::{TemplateContext, Templating, resolve_fragments};

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
/// the other fields (`version`, `description`, `author`, `repository`,
/// `license`) are passthrough text the adapter serializes into the
/// provider-appropriate `plugin.json` shape.
#[derive(Clone, Debug)]
pub struct PluginManifest {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub author: Option<PluginAuthor>,
    pub repository: Option<String>,
    pub license: Option<String>,
}

/// Author sub-record for plugin manifests.
///
/// Both Claude and Cursor accept an object shape (`{ name, email? }`).
#[derive(Clone, Debug)]
pub struct PluginAuthor {
    pub name: String,
    pub email: Option<String>,
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
    /// The spec this file was produced from, or `None` for files no single
    /// spec owns — plugin manifests, hook scripts, hook shims, and the bundled
    /// `hooks/hooks.json`.
    ///
    /// Together with [`crate::adapters::AdapterOutput::deliveries`] this is
    /// how a provider's per-spec output is identified: a markdown spec names
    /// itself here, while a hook spec never does — no hook file names a single
    /// spec in any emit mode — and is identified by its registration delivery
    /// instead. A spec-owned file left `None` therefore reads as a spec the
    /// provider emitted nothing for.
    pub spec_id: Option<String>,
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
            spec_id: None,
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
            spec_id: None,
        }
    }

    /// Record the spec this file was produced from.
    #[must_use]
    pub fn with_spec_id(mut self, spec_id: &str) -> Self {
        self.spec_id = Some(spec_id.to_owned());
        self
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
/// the per-provider forward-direction `ForwardPatch` instances that downstream
/// `sync_plan` drains; `dest_roots` records each provider's adapter-computed
/// sync destination root so `sync_plan` can anchor `ManifestTrackedWrite`
/// destinations without re-calling adapter path methods.
#[derive(Debug, Default)]
pub struct CompileResult {
    pub files: Vec<GeneratedFile>,
    pub patches: HashMap<Provider, Vec<Box<dyn ForwardPatch>>>,
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

/// An intent with no matching delivery: a value the author configured that
/// reached no file of the kind that could have carried it.
///
/// Derived by subtraction in [`derive_losses`], never pushed. The constructor
/// is private to this module, so no adapter can assert a loss — an adapter
/// records only what it carried, and what it dropped follows from the
/// arithmetic.
///
/// Field declaration order is the sort key. `(provider, setting, kind,
/// spec_id)` is unique per loss, so `spec_type` and `categorical` never decide
/// an ordering.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Loss {
    provider: Provider,
    setting: SettingKey,
    /// The file kind the setting failed to reach, and `None` for a `Body`
    /// loss.
    ///
    /// A non-`Body` intent is raised per kind the adapter actually emitted, so
    /// every such loss names one. A `Body` loss means the adapter emitted
    /// nothing for this spec, so there is no kind to name — the renderer reads
    /// `spec_type` instead.
    kind: Option<FileKind>,
    /// Sorted on ahead of `spec_id` so a `Body` group stays contiguous by spec
    /// type, which is the population that group is counted against.
    spec_type: &'static str,
    spec_id: String,
    /// True when every spec holding this intent lost it, so the renderer
    /// collapses the group to one counted line.
    ///
    /// The population is every spec holding the same `(provider, setting,
    /// kind)` intent — except for `Body`, which every spec holds and whose
    /// `kind` is always `None`. Comparing a `Body` loss against the whole
    /// library would mean three hooks out of eleven specs never collapse, so a
    /// `Body` loss is counted against specs of its own type instead: "every
    /// hook spec lost its body". That is also the population that shares its
    /// rendered explanation, which reads from `spec_type` rather than a file
    /// kind.
    ///
    /// Computed after the subtraction by comparing losers against intent
    /// holders; no adapter declares it, so a provider that gains a capability
    /// moves its diagnostics from categorical to per-spec with no code change.
    ///
    /// This is the one field on `Loss` that encodes a rendering decision on
    /// the collection side, and it is deliberate: `categorical` is derived
    /// from the intent set, which only `compile_specs` holds, so the renderer
    /// cannot compute it.
    categorical: bool,
}

impl Loss {
    /// Build a loss directly, for tests that exercise a renderer rather than
    /// the subtraction that produces one.
    ///
    /// Test support, not part of the supported surface — `derive_losses` is
    /// the only producer in the pipeline. It cannot be `#[cfg(test)]`: the
    /// renderer lives in the binary crate, which links the library built
    /// without that cfg. What the privacy of the fields protects is that
    /// nothing *derives* a loss except by subtraction, and that holds: there
    /// is no route from here into [`CompileDiagnostics`], whose fields are
    /// private and which only `compile_specs` populates.
    #[doc(hidden)]
    pub fn for_test(
        provider: Provider,
        setting: SettingKey,
        kind: Option<FileKind>,
        spec_type: &'static str,
        spec_id: &str,
        categorical: bool,
    ) -> Self {
        Self {
            provider,
            setting,
            kind,
            spec_type,
            spec_id: spec_id.to_owned(),
            categorical,
        }
    }

    pub fn provider(&self) -> Provider {
        self.provider
    }

    pub fn setting(&self) -> &SettingKey {
        &self.setting
    }

    pub fn kind(&self) -> Option<FileKind> {
        self.kind
    }

    pub fn spec_id(&self) -> &str {
        &self.spec_id
    }

    pub fn spec_type(&self) -> &'static str {
        self.spec_type
    }

    pub fn is_categorical(&self) -> bool {
        self.categorical
    }
}

/// Diagnostics surfaced from the compile stage.
///
/// Three channels, split by who originates them. `losses` records values an
/// author configured that reached no generated file; each is derived here by
/// subtracting what the adapters recorded delivering from what the author
/// configured, so no adapter constructs one. `degradations` records provider
/// limitations — facts about a provider runtime acting on bytes agentspec
/// delivered successfully — each constructed by the adapter making the claim
/// and drained here. `parity` records disagreements across the active
/// provider set, which no single adapter can compute.
///
/// Every field is private, and neither `Loss` nor `Degradation` has a
/// constructor reachable from outside its own module, so no code path can
/// append to the wrong channel.
#[derive(Debug, Default)]
pub struct CompileDiagnostics {
    losses: Vec<Loss>,
    degradations: Vec<Degradation>,
    parity: Vec<ParityWarning>,
}

impl CompileDiagnostics {
    /// Ordered by `(provider, setting, kind, spec_type, spec_id)` and
    /// deduplicated at the derivation point. `spec_type` precedes `spec_id`
    /// so that a `Body` group — which the renderer splits by spec type, on the
    /// same population `categorical` was counted against — stays contiguous.
    pub fn losses(&self) -> &[Loss] {
        &self.losses
    }

    /// Ordered by `(provider, kind, subject)` and deduplicated at the drain
    /// point, so consecutive entries sharing a `(provider, kind)` form a
    /// renderable group.
    pub fn degradations(&self) -> &[Degradation] {
        &self.degradations
    }

    pub fn parity(&self) -> &[ParityWarning] {
        &self.parity
    }
}

/// A disagreement across the active provider set.
///
/// Constructed only by `compile_specs`, which is the only code with the active
/// provider list. `AdapterOutput` has no field for one, so an adapter cannot
/// emit a parity warning even by accident.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParityWarning {
    /// At least one hook spec targets `session_start` AND the active provider
    /// set spans both `session_start_fires_on_resume() == true` and `== false`
    /// hook-emitting adapters. Single-provider configurations never trip this.
    SessionStart,
}

impl ParityWarning {
    pub fn message(&self) -> String {
        match self {
            Self::SessionStart => String::from(
                "session_start asymmetry: at least one targeted provider does not fire `session_start` on conversation resume; the canonical `session_start` event reflects this. To trigger logic on resume firings, branch on `provider_raw.source == \"resume\"` (provider-specific). See docs/hooks-canonical.md#session-start-asymmetry.",
            ),
        }
    }
}

/// One thing the author configured, and the file kind it was expected to
/// reach. `None` for `Body`, which is matched per spec rather than per kind.
type Intent = (String, SettingKey, Option<FileKind>);

/// The settings the author configured for one provider, keyed for subtraction.
///
/// `Body` takes one intent per spec. Every other setting takes one intent per
/// kind the adapter actually emitted for that spec, so an `OpenCode` skill
/// with both invocation flags holds a `Model` intent against `Commands` and
/// another against `Skills` — and the command file satisfying the first does
/// not mask the skill file losing the second.
///
/// A spec the provider emitted nothing for yields only its `Body` intent.
/// Reporting a lost `model` beside "no file at all" is noise, and a per-kind
/// rule over an empty set would make the total drop unreportable.
fn intents(
    specs: &[Spec],
    provider: Provider,
    presets: &ProviderPresetsMap,
    files: &[GeneratedFile],
) -> Vec<Intent> {
    let mut out = Vec::new();
    for spec in specs {
        let id = spec.id();
        out.push((id.to_owned(), SettingKey::Body, None));

        let mut emitted: Vec<FileKind> = Vec::new();
        for file in files {
            if file.provider == provider
                && file.spec_id.as_deref() == Some(id)
                && !emitted.contains(&file.kind)
            {
                emitted.push(file.kind);
            }
        }
        if emitted.is_empty() {
            continue;
        }

        let mut configured: Vec<SettingKey> = spec
            .execution_preset()
            .and_then(|name| presets.get(name))
            .map(|preset| preset.configured(provider))
            .unwrap_or_default();
        if spec.declares_tools() {
            configured.push(SettingKey::Tools);
        }
        if spec.declares_paths() {
            configured.push(SettingKey::Paths);
        }

        for key in configured {
            for &kind in &emitted {
                out.push((id.to_owned(), key.clone(), Some(kind)));
            }
        }
    }
    out
}

/// Subtract what each adapter recorded delivering from what the author
/// configured. The remainder is the loss set.
fn derive_losses(
    specs: &[Spec],
    presets: &ProviderPresetsMap,
    providers: &[Provider],
    files: &[GeneratedFile],
    deliveries: &HashMap<Provider, Vec<Delivery>>,
) -> BTreeSet<Loss> {
    let spec_types: HashMap<&str, &'static str> =
        specs.iter().map(|s| (s.id(), s.spec_type())).collect();
    let mut losses: BTreeSet<Loss> = BTreeSet::new();

    for &provider in providers {
        // A `Body` delivery is reduced to `(spec_id, Body, None)`, discarding
        // the kind it carries. A spec's emitted kind need not match its spec
        // type — `OpenCode` emits a `Commands` file and no `Skills` file for a
        // `user_invocable`-only skill — so keying `Body` on kind would report
        // those specs as unemitted.
        let delivered: HashSet<Intent> = deliveries
            .get(&provider)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|d| {
                let kind = if *d.setting() == SettingKey::Body {
                    None
                } else {
                    Some(d.kind())
                };
                (d.spec_id().to_owned(), d.setting().clone(), kind)
            })
            .collect();

        // A markdown spec's `Body` delivery is the emitted file itself, read
        // straight off `files` rather than through a `Delivery` — which is
        // what keeps `Delivery`'s constructor out of the orchestrator's reach
        // while the derivation stays structural.
        let emitted_bodies: HashSet<&str> = files
            .iter()
            .filter(|f| f.provider == provider)
            .filter_map(|f| f.spec_id.as_deref())
            .collect();

        let raised = intents(specs, provider, presets, files);
        let mut lost: Vec<Intent> = Vec::new();
        for intent in &raised {
            let satisfied = if intent.1 == SettingKey::Body {
                emitted_bodies.contains(intent.0.as_str()) || delivered.contains(intent)
            } else {
                delivered.contains(intent)
            };
            if !satisfied {
                lost.push(intent.clone());
            }
        }

        let adapter = provider.adapter();
        for (spec_id, setting, kind) in &lost {
            // A loss whose kind the adapter's own table says carries the
            // setting means the adapter emitted such a file and left the field
            // unset. Per `.claude/rules/validation-locality.md` that belongs
            // here as a debug-only check rather than as a report line, so the
            // report carries no schema-limit-versus-adapter-bug verdict.
            //
            // What holds it: no adapter conditionally omits a field its
            // `carriable` table declares for a kind it emitted. Each `carried()`
            // reads `Some`-ness off the struct just built, and every declared
            // field is populated unconditionally for that kind — Cursor's
            // bracket keys come from the composing expression, OpenCode's
            // `tools` map is non-`Option`. `test_carriable_agrees_with_carried`
            // checks the weaker existential claim (some spec delivered each
            // declared setting), so it narrows this rather than proving it; a
            // future adapter that gates a declared field on a spec predicate
            // would trip this assert, which is the intended signal.
            //
            // A `Body` loss carries `kind: None` and is exempt by construction.
            debug_assert!(
                kind.is_none_or(|k| !adapter.carriable(k).contains(&setting.kind())),
                "{provider} lost {setting:?} on a {kind:?} file its own carriable table \
                 declares — test_carriable_agrees_with_carried should have excluded this"
            );

            let spec_type = spec_types.get(spec_id.as_str()).copied().unwrap_or("spec");

            // Categorical when every spec holding this intent lost it, derived
            // from the intent set rather than declared. A `Body` intent is
            // held by every spec, so it is counted against specs of its own
            // type — see the field's doc comment.
            let same_population = |(other_id, s, k): &Intent| {
                s == setting
                    && k == kind
                    && (kind.is_some()
                        || spec_types.get(other_id.as_str()).copied() == Some(spec_type))
            };
            let holders = raised.iter().filter(|i| same_population(i)).count();
            let losers = lost.iter().filter(|i| same_population(i)).count();

            losses.insert(Loss {
                provider,
                setting: setting.clone(),
                kind: *kind,
                spec_type,
                spec_id: spec_id.clone(),
                categorical: holders == losers,
            });
        }
    }
    losses
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
/// Returns the generated files alongside a [`CompileDiagnostics`] carrying two
/// channels: the `Degradation` values each adapter reported for inputs it
/// could not honor, and the cross-provider `ParityWarning` gates that only the
/// full active provider set can evaluate.
///
/// Presets come from `validated`, not a parameter, so the map compiled against
/// is the one [`Specs::validate`](crate::specs::Specs::validate) checked — a
/// caller cannot validate one map and compile with another. Calling an adapter
/// directly through [`Provider::adapter`](crate::provider::Provider::adapter)
/// bypasses that entirely and supplies its own presets, guarded only by
/// `debug_assert!`s that compile out in release.
pub fn run(
    validated: &ValidatedSpecs,
    templating: &Templating,
    providers: &[Provider],
    adapter_configs: &HashMap<Provider, AdapterConfig>,
    compile_targets: &HashMap<Provider, ProviderCompileTarget>,
    home: &Path,
    cwd: &Path,
) -> Result<(CompileResult, CompileDiagnostics)> {
    compile_specs(
        validated.specs(),
        templating,
        validated.presets(),
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
// Eight params, one over the clippy default, and each carries a distinct
// stage-input concern (specs, templating, presets, providers, adapter configs,
// per-provider compile targets, home, cwd). Bundling them into a context struct
// would just rename the noise — see CLAUDE.md "config structs at module
// boundaries".
#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_specs(
    specs: &[Spec],
    templating: &Templating,
    presets: &ProviderPresetsMap,
    providers: &[Provider],
    adapter_configs: &HashMap<Provider, AdapterConfig>,
    compile_targets: &HashMap<Provider, ProviderCompileTarget>,
    home: &Path,
    cwd: &Path,
) -> Result<(CompileResult, CompileDiagnostics)> {
    let mut files: Vec<GeneratedFile> = Vec::new();
    let mut patches: HashMap<Provider, Vec<Box<dyn ForwardPatch>>> = HashMap::new();
    let mut dest_roots: HashMap<Provider, PathBuf> = HashMap::new();
    let mut degradations: BTreeSet<Degradation> = BTreeSet::new();
    let mut deliveries: HashMap<Provider, Vec<Delivery>> = HashMap::new();

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
        let adapter_config = adapter_configs.get(&provider);
        let context = TemplateContext::from_specs_for_provider(specs, provider, adapter_config);
        let resolved = resolve_fragments(specs.to_vec(), templating, Some(provider), &context)?;

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
        degradations.extend(output.degradations);
        deliveries
            .entry(provider)
            .or_default()
            .extend(output.deliveries);
    }

    // Sort output files by path for deterministic ordering
    files.sort_by(|a, b| a.path.cmp(&b.path));

    // Losses are derived here, after every adapter has reported what it
    // carried, by subtracting those records from what the author configured.
    // No adapter can construct a `Loss`, and this function cannot construct a
    // `Delivery` — the arithmetic is the only route between the two.
    //
    // Reads the pre-resolution `specs` while the deliveries came from each
    // provider's template-resolved copy. That is sound because
    // `resolve_fragments` rewrites `body` alone: ids, execution presets,
    // capabilities, and paths pass through untouched, so both sides agree on
    // spec identity and on what the author configured. A resolution step that
    // ever rewrote frontmatter would make every setting read as lost.
    let losses = derive_losses(specs, presets, &sorted_providers, &files, &deliveries);

    // Cross-provider parity warning. Evaluated after the per-provider compile
    // loop so the active provider set is fully known — this is the one
    // diagnostic no single adapter can compute, because it compares adapters
    // against each other rather than describing any one of them.
    //
    // The other two channels arrive differently: a `Degradation` is pushed by
    // the adapter making the claim, and a `Loss` is derived by subtraction
    // above. `compile_specs` has no reachable constructor for a `Degradation`,
    // so it cannot rediscover one by re-scanning specs here.
    //
    // `SessionStart` fires once globally when the active set spans adapters
    // that disagree on resume-firing. Single-provider configurations can't
    // trip this gate by construction.
    let has_session_start_hook = specs.iter().any(|spec| {
        matches!(spec, Spec::Hook(h) if h.frontmatter.events.contains(&HookEvent::SessionStart))
    });

    let mut parity: Vec<ParityWarning> = Vec::new();
    if has_session_start_hook {
        let mut any_fires_on_resume = false;
        let mut any_does_not_fire_on_resume = false;
        for &provider in &sorted_providers {
            let adapter = provider.adapter();
            // A provider that declares no carriable setting on hook files
            // emits no hooks, so it has no resume behavior to compare.
            if adapter.carriable(FileKind::Hooks).is_empty() {
                continue;
            }
            if adapter.session_start_fires_on_resume() {
                any_fires_on_resume = true;
            } else {
                any_does_not_fire_on_resume = true;
            }
        }
        if any_fires_on_resume && any_does_not_fire_on_resume {
            parity.push(ParityWarning::SessionStart);
        }
    }

    Ok((
        CompileResult {
            files,
            patches,
            dest_roots,
        },
        CompileDiagnostics {
            // `BTreeSet` ordering is what makes both drained orders
            // independent of adapter iteration order and of the order in which
            // an adapter happens to walk specs — by
            // `(provider, setting, kind, spec_type, spec_id)` for losses and
            // `(provider, kind, subject)` for degradations.
            losses: losses.into_iter().collect(),
            degradations: degradations.into_iter().collect(),
            parity,
        },
    ))
}

#[cfg(test)]
mod loss_tests {
    use indexmap::IndexMap;

    use super::{GeneratedFile, Loss, derive_losses};
    use crate::adapters::Delivery;
    use crate::plan::FileKind;
    use crate::presets::{OpenCodePreset, ProviderPresets, ProviderPresetsMap};
    use crate::provider::Provider;
    use crate::setting::SettingKey;
    use crate::spec::{
        CapabilitiesFrontmatter, ExecutionFrontmatter, SkillFrontmatter, SkillSpec, Spec,
        ToolFrontmatter,
    };

    const PROVIDER: Provider = Provider::OpenCode;
    const PRESET: &str = "default";

    fn presets() -> ProviderPresetsMap {
        std::collections::HashMap::from([(
            PRESET.to_owned(),
            ProviderPresets {
                opencode: Some(OpenCodePreset {
                    model: Some("anthropic/claude-opus-5".to_owned()),
                    variant: Some("thinking".to_owned()),
                }),
                ..ProviderPresets::default()
            },
        )])
    }

    fn skill(id: &str, with_preset: bool, with_tools: bool) -> Spec {
        Spec::Skill(SkillSpec {
            path: "test.md".into(),
            frontmatter: SkillFrontmatter {
                id: id.to_owned(),
                description: Some("d".to_owned()),
                tags: None,
                user_invocable: true,
                agent_invocable: true,
                execution: with_preset.then(|| ExecutionFrontmatter {
                    preset: Some(PRESET.to_owned()),
                }),
                capabilities: with_tools.then(|| CapabilitiesFrontmatter {
                    tools: Some(vec![ToolFrontmatter::Read]),
                }),
            },
            body: "Body.".to_owned(),
            supporting_files: IndexMap::new(),
        })
    }

    fn file(spec_id: &str, kind: FileKind) -> GeneratedFile {
        GeneratedFile::text(PROVIDER, kind, "out.md", String::new()).with_spec_id(spec_id)
    }

    fn delivery(spec_id: &str, setting: &SettingKey, kind: FileKind) -> Delivery {
        Delivery::for_test(spec_id, setting.clone(), kind)
    }

    fn losses_for(specs: &[Spec], files: &[GeneratedFile], deliveries: Vec<Delivery>) -> Vec<Loss> {
        let map = std::collections::HashMap::from([(PROVIDER, deliveries)]);
        derive_losses(specs, &presets(), &[PROVIDER], files, &map)
            .into_iter()
            .collect()
    }

    #[test]
    fn test_subtraction_reports_configured_value_with_no_delivery() {
        // On `Skills`, where `OpenCode` carries neither setting. Asserting
        // this against `Commands` instead would trip the `debug_assert!` in
        // `derive_losses`, because that kind's table declares both — which is
        // the check working, not a test to route around.
        let specs = [skill("s", true, false)];
        let files = [file("s", FileKind::Skills)];
        let losses = losses_for(
            &specs,
            &files,
            vec![delivery("s", &SettingKey::Model, FileKind::Skills)],
        );
        assert_eq!(losses.len(), 1, "{losses:#?}");
        assert_eq!(*losses[0].setting(), SettingKey::Variant);
        assert_eq!(losses[0].kind(), Some(FileKind::Skills));
    }

    #[test]
    fn test_intent_is_per_emitted_kind() {
        // The masking the per-kind rule exists to close: a command file
        // carrying `model` must not satisfy the skill file's intent. This is
        // the assertion that fails if anyone collapses non-`Body` matching
        // back to `(spec, setting)`.
        let specs = [skill("s", true, false)];
        let files = [file("s", FileKind::Commands), file("s", FileKind::Skills)];
        let losses = losses_for(
            &specs,
            &files,
            vec![
                delivery("s", &SettingKey::Model, FileKind::Commands),
                delivery("s", &SettingKey::Variant, FileKind::Commands),
            ],
        );
        let skills_losses: Vec<&Loss> = losses
            .iter()
            .filter(|l| l.kind() == Some(FileKind::Skills))
            .collect();
        assert_eq!(skills_losses.len(), 2, "{losses:#?}");
        assert!(
            losses.iter().all(|l| l.kind() != Some(FileKind::Commands)),
            "the command file carried both settings, so it loses nothing: {losses:#?}"
        );
    }

    #[test]
    fn test_spec_with_no_emitted_files_loses_only_body() {
        // Reporting a lost `model` beside "no file at all" is noise, and a
        // per-kind rule over an empty set would make the total drop
        // unreportable.
        let specs = [skill("s", true, true)];
        let losses = losses_for(&specs, &[], Vec::new());
        assert_eq!(losses.len(), 1, "{losses:#?}");
        assert_eq!(*losses[0].setting(), SettingKey::Body);
        assert_eq!(losses[0].kind(), None);
    }

    #[test]
    fn test_body_is_delivered_by_any_emitted_kind() {
        // Regression guard for keying `Body` on the delivery's kind, which
        // would fabricate a body loss for every spec whose emitted kind
        // differs from its spec type — `basic-skill` and `scripted-skill` in
        // the fixture are both `Commands`-only — and then trip the
        // `debug_assert!` in `derive_losses`.
        let specs = [skill("s", false, false)];
        let files = [file("s", FileKind::Commands)];
        let losses = losses_for(&specs, &files, Vec::new());
        assert!(losses.is_empty(), "{losses:#?}");
    }

    #[test]
    fn test_loss_is_categorical_when_every_intent_holder_loses() {
        let specs = [skill("a", true, false), skill("b", true, false)];
        let files = [file("a", FileKind::Skills), file("b", FileKind::Skills)];

        let both_lose = losses_for(&specs, &files, Vec::new());
        assert!(
            both_lose
                .iter()
                .filter(|l| *l.setting() == SettingKey::Model)
                .all(Loss::is_categorical),
            "{both_lose:#?}"
        );

        let one_delivers = losses_for(
            &specs,
            &files,
            vec![delivery("a", &SettingKey::Model, FileKind::Skills)],
        );
        let model_losses: Vec<&Loss> = one_delivers
            .iter()
            .filter(|l| *l.setting() == SettingKey::Model)
            .collect();
        assert_eq!(model_losses.len(), 1, "{one_delivers:#?}");
        assert!(
            !model_losses[0].is_categorical(),
            "one holder delivered, so the group is per-spec: {model_losses:#?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parity_warning_session_start_message() {
        // The text was transplanted verbatim from the removed
        // `CompileWarning::SessionStartAsymmetry` arm; assert it directly so
        // the string is not pinned only through integration-level stderr.
        assert!(
            ParityWarning::SessionStart
                .message()
                .contains("session_start asymmetry")
        );
    }

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
