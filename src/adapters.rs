//! Provider adapter trait.
//!
//! Every provider-specific decision (file paths, frontmatter shapes, hook JSON
//! layout, post-write patchers) lives behind the [`Adapter`] trait. Non-adapter
//! modules dispatch through `Provider::adapter()` exclusively — see
//! `.claude/rules/provider-logic-in-adapters.md`.

mod claude;
mod cursor;
mod hook_compile;
mod hooks_helpers;
mod opencode;

use std::path::{Path, PathBuf};

use anyhow::Result;
pub use claude::ClaudeAdapter;
pub use cursor::CursorAdapter;
pub use opencode::OpenCodeAdapter;

use crate::compile::{AdapterConfig, GeneratedFile, HookEmitMode};
use crate::plan::{FileKind, ForwardPatch, ReversePatch, expand_tilde};
use crate::presets::ProviderPresetsMap;
use crate::provider::Provider;
use crate::setting::{SettingKey, SettingKind};
use crate::spec::{HookEvent, Spec, ToolFrontmatter};

/// Library-side mirror of the binary's `SyncMode`.
///
/// Defined here (with no clap or serde derives) so trait methods can stay in
/// the library while the binary owns the CLI/config-loading parts of
/// `SyncMode`. The binary translates at the boundary, paralleling the existing
/// `SyncMode → HookEmitMode` translation in `src/config.rs`.
///
/// Carries one more variant than the binary's `SyncMode` by design: `Compile`
/// is internal-only (the canonical mode for the `agentspec compile` command)
/// and is never reachable from TOML. `Plugin` is the public sync-target mode
/// that drives plugin-tier emission (provider plugin manifest +
/// provider-anchored hook commands).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncDestinationMode {
    User,
    Project,
    /// Plugin distribution mode — emits a self-contained tree with a
    /// provider-appropriate manifest under `<dest>/<provider-plugin-dir>/plugin.json`.
    Plugin,
    /// Compile-command default — internal-only, used by `agentspec compile`
    /// to produce canonical provider-config-dir-agnostic output under
    /// `generated/<provider>/`. Not reachable from TOML.
    Compile,
}

pub(crate) fn resolve_config_dir(
    mode: SyncDestinationMode,
    target_dir: Option<&Path>,
    home: &Path,
    cwd: &Path,
    home_dotdir: &Path,
    project_dotdir: &Path,
) -> PathBuf {
    match mode {
        SyncDestinationMode::User => home.join(home_dotdir),
        SyncDestinationMode::Project => target_dir.map_or_else(
            || cwd.join(project_dotdir),
            |d| {
                let base = d
                    .to_str()
                    .map_or_else(|| d.to_path_buf(), |s| expand_tilde(s, home));
                base.join(project_dotdir)
            },
        ),
        SyncDestinationMode::Plugin | SyncDestinationMode::Compile => target_dir.map_or_else(
            || home.join(home_dotdir),
            |d| {
                d.to_str()
                    .map_or_else(|| d.to_path_buf(), |s| expand_tilde(s, home))
            },
        ),
    }
}

impl SyncDestinationMode {
    /// Map to the `HookEmitMode` that controls per-provider hook emission
    /// shape. User/Project map to merged-mode patches; Plugin and Compile
    /// both map to a self-contained `hooks/hooks.json` bundle.
    ///
    /// This collapses the previous binary-side `SyncMode → HookEmitMode`
    /// translation into a library-side method so adapters can derive their
    /// emit mode directly from `CompileCtx.mode` without threading
    /// `AdapterConfig.hook_emit_mode` through.
    pub fn to_hook_emit_mode(self) -> HookEmitMode {
        match self {
            Self::User => HookEmitMode::MergedUser,
            Self::Project => HookEmitMode::MergedProject,
            Self::Plugin | Self::Compile => HookEmitMode::Bundled,
        }
    }
}

/// Outcome of a per-provider tidy. The provider's tidy closure mutates the
/// supplied top-level `CstObject` in place via `jsonc-parser`'s
/// interior-mutability API and reports (a) how many user-authored entries
/// survived (for the existing summary line) and (b) whether the file is
/// effectively empty per the provider's predicate, which drives the
/// delete-on-empty branch in the generic shell.
#[derive(Debug)]
pub struct TidyOutcome {
    pub user_entries_remaining: usize,
    pub file_should_be_deleted: bool,
}

/// Per-provider context passed to `Adapter::compile`.
///
/// Carries everything `compile` needs that isn't the spec list itself. The
/// orchestrator constructs one `CompileCtx` per `(provider, target)` and hands
/// it to the adapter. Adapters use it to compute output paths, apply prefix
/// transforms, and resolve presets — without each piece of state needing a
/// dedicated trait method.
#[derive(Debug)]
pub struct CompileCtx<'a> {
    /// Library-side mirror of the binary's `SyncMode`. For the `compile`
    /// command path (no sync target), the binary supplies a default of
    /// `SyncDestinationMode::Compile` with `target_dir: None`.
    pub mode: SyncDestinationMode,
    /// Effective home directory (for User-mode dest resolution).
    pub home: &'a Path,
    /// Effective current working directory (for Project-mode dest resolution).
    pub cwd: &'a Path,
    /// Explicit destination directory. Set when `mode` is `Plugin`, `Compile`,
    /// or `Project` with a custom `dir`; `None` for `User` mode and default
    /// `Project` mode.
    pub target_dir: Option<&'a Path>,
    /// Preset library — adapters consume per-provider presets when applying
    /// frontmatter transforms.
    pub presets: &'a ProviderPresetsMap,
    /// Per-provider `AdapterConfig` for prefix/strip transforms. `None` means
    /// "use canonical (unprefixed) defaults" — the same convention as today's
    /// `AdapterConfig` parameter.
    pub adapter_config: Option<&'a AdapterConfig>,
    /// `--force` flag from the sync target; controls whether forward patches
    /// may replace user-authored non-object/non-array values rather than
    /// erroring. `false` for the `compile` command path.
    pub overwrite: bool,
}

/// Per-provider context for `Adapter::removal_patches`.
///
/// Narrower than `CompileCtx` because the remove path identifies owned
/// entries via on-disk `_agentspec_id` sentinels (no spec input → no preset
/// or prefix transforms apply).
#[derive(Debug)]
pub struct RemoveCtx<'a> {
    pub mode: SyncDestinationMode,
    pub home: &'a Path,
    pub cwd: &'a Path,
    pub target_dir: Option<&'a Path>,
}

/// What an adapter's `compile` step returns.
///
/// `files` carries every file the adapter produces (markdown specs plus any
/// hook scripts and bundled `hooks.json` in Bundled mode). `patches` carries
/// the forward-direction post-write patches to apply after files land —
/// Claude/Cursor settings merges, `OpenCode` instructions registration, etc.
/// `dest_root` is the adapter-computed sync-mode destination root that
/// downstream `sync_plan` uses to anchor each `(provider, kind)`
/// `ManifestTrackedWrite`. `degradations` carries the values this adapter was
/// handed and could not honor.
#[derive(Debug)]
pub struct AdapterOutput {
    pub files: Vec<GeneratedFile>,
    pub patches: Vec<Box<dyn ForwardPatch>>,
    pub dest_root: PathBuf,
    /// Values this adapter was handed and could not honor, discovered during
    /// its own walk. `compile_specs` drains these; it cannot construct one.
    pub degradations: Vec<Degradation>,
    /// The record of what this adapter carried: one entry per setting that
    /// landed in an emitted file. An adapter asserts nothing here about what
    /// it dropped — only about what it delivered.
    ///
    /// `compile_specs` collects these per provider. Nothing subtracts them
    /// from an author-intent set yet, so the record is currently written and
    /// not read.
    pub deliveries: Vec<Delivery>,
}

/// One adapter carried one setting for one spec into one emitted file.
///
/// Constructed only by adapter implementations and the hook synthesis helper,
/// from the point where the value is handed to a `GeneratedFile`. The
/// orchestrator subtracts these; it cannot build one.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Delivery {
    spec_id: String,
    setting: SettingKey,
    /// The kind of the file the setting landed in. Part of the subtraction
    /// key, so a `model` delivered into an `OpenCode` command file does not
    /// satisfy the same spec's skill-file intent. Also what lets
    /// `test_carriable_agrees_with_carried` check a declaration against a
    /// recording per file kind rather than per provider.
    kind: FileKind,
    /// Relative to the provider's dest root. For a delivery recorded off a
    /// `GeneratedFile`, this is that file's path. For a hook registration
    /// under merged emit mode it is `hooks/hooks.json`, which agentspec does
    /// not write in that mode — the registration lands in the provider's host
    /// file instead. The report never renders this field, so the merged-mode
    /// value is a stable identity for the subtraction rather than a path the
    /// author can open.
    file: PathBuf,
}

impl Delivery {
    /// One delivery per setting `carried` landed in `file`.
    ///
    /// Called immediately after the adapter builds the `GeneratedFile`, from
    /// the one point where a frontmatter struct becomes bytes.
    fn from_file(spec_id: &str, file: &GeneratedFile, carried: Vec<SettingKey>) -> Vec<Self> {
        carried
            .into_iter()
            .map(|setting| Self {
                spec_id: spec_id.to_owned(),
                setting,
                kind: file.kind,
                file: file.path.clone(),
            })
            .collect()
    }

    /// A hook spec's registration reached `file`. Used only by
    /// `synthesize_hooks`, which has entries rather than per-spec files, so
    /// this takes the path directly rather than a `GeneratedFile`.
    fn registration(spec_id: &str, kind: FileKind, file: PathBuf) -> Self {
        Self {
            spec_id: spec_id.to_owned(),
            setting: SettingKey::Body,
            kind,
            file,
        }
    }

    pub fn spec_id(&self) -> &str {
        &self.spec_id
    }

    pub fn setting(&self) -> &SettingKey {
        &self.setting
    }

    pub fn kind(&self) -> FileKind {
        self.kind
    }

    pub fn file(&self) -> &Path {
        &self.file
    }
}

/// A value the spec author supplied that one provider could not honor.
///
/// Constructed only by adapter implementations, from inside the walk where the
/// drop happens. The orchestrator drains and renders these; it cannot build
/// one, which is the direction that eroded when `SkippedHook` was populated by
/// a post-loop re-scan in `compile_specs`.
///
/// Field declaration order is the sort key: derived `Ord` compares
/// `provider`, then `kind`, then `subject`, which is exactly the tuple
/// `compile_specs` collects into a `BTreeSet` to dedup and order in one step.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Degradation {
    provider: Provider,
    kind: DegradationKind,
    /// The spec whose value was dropped, when the same provider honors the
    /// value for other specs. `None` when the limitation is provider-global
    /// and enumerating specs adds nothing.
    subject: Option<String>,
}

impl Degradation {
    /// The provider cannot honor this kind of value for any spec. Pushing the
    /// same `(provider, kind)` more than once is harmless — the drain point's
    /// `BTreeSet` collapses it.
    fn provider_wide(provider: Provider, kind: DegradationKind) -> Self {
        Self {
            provider,
            kind,
            subject: None,
        }
    }

    /// The provider honors this kind of value for other specs but dropped it
    /// for `subject`. One rendered line per subject survives the drain point.
    fn for_spec(provider: Provider, subject: &str, kind: DegradationKind) -> Self {
        Self {
            provider,
            kind,
            subject: Some(subject.to_owned()),
        }
    }

    pub fn provider(&self) -> Provider {
        self.provider
    }

    pub fn kind(&self) -> DegradationKind {
        self.kind
    }

    pub fn subject(&self) -> Option<&str> {
        self.subject.as_deref()
    }

    /// Human-readable diagnostic text.
    ///
    /// Rendered only for kinds whose `presentation()` is
    /// `Presentation::Warning`; `CountedSubjects` kinds are rendered from
    /// their subjects instead. The `HooksUnsupported` arm therefore has no
    /// caller in the binary today, but is reachable through this public
    /// accessor and is not dead.
    pub fn message(&self) -> String {
        let name = self.provider.display_name();
        match self.kind {
            DegradationKind::PathScopedRulesUnsupported => format!(
                "{name} does not support path-scoped rules; rules with `paths` will be emitted as always-on for {name}."
            ),
            // URL anchors at the parent `## Documented limitations` section
            // rather than templating a per-provider subsection name — the
            // per-provider subsections may not exist for every adapter that
            // ever returns `fully_implements_canonical_output() == false`.
            DegradationKind::PartialOutputImpl => format!(
                "{name} has partial implementation of `user_message`/`agent_message`/`additional_context` hook output fields; canonical `user_facing_message`, `decision_reason`, and `additional_context` values may not surface in the {name} UI/agent context. See docs/hooks-canonical.md#documented-limitations. (Suppression via config flag is on the roadmap.)"
            ),
            DegradationKind::HooksUnsupported => {
                format!("{name} does not emit hooks; hook specs are skipped.")
            }
        }
    }
}

/// What kind of value was dropped.
///
/// Carries no payload, and that is a commitment rather than an omission: the
/// drain point identifies a degradation by `(provider, kind, subject)`, so a
/// payload here would redefine what "the same degradation" means and break the
/// `BTreeSet` collapse.
///
/// Declaration order is the secondary sort key, and so is user-visible:
/// reordering these variants reorders the stderr lines within a provider's
/// group. `test_compile_diagnostic_block_order_and_cardinality` is what
/// catches such a reorder.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DegradationKind {
    /// A rule spec has `paths` set and the provider has no native path
    /// scoping, so the rule is emitted as always-on. Pushed by the adapter
    /// whose `supports_path_scoped_rules()` is `false`.
    PathScopedRulesUnsupported,
    /// At least one hook spec targets a provider whose hook host runtime only
    /// partially implements the canonical output schema's UI/agent/context
    /// fields. Pushed by the adapter whose
    /// `fully_implements_canonical_output()` is `false`.
    PartialOutputImpl,
    /// A hook spec targets a provider that emits no hooks at all, so the spec
    /// produces nothing. Pushed by the adapter whose `emits_hooks()` is
    /// `false`.
    HooksUnsupported,
}

impl DegradationKind {
    /// How `surface_compile_diagnostics` renders a `(provider, kind)` group.
    pub fn presentation(self) -> Presentation {
        match self {
            Self::PathScopedRulesUnsupported | Self::PartialOutputImpl => Presentation::Warning,
            Self::HooksUnsupported => Presentation::CountedSubjects {
                singular: "hook",
                plural: "hooks",
            },
        }
    }
}

/// Preserves the two stderr shapes the compile stage already emits rather than
/// unifying them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Presentation {
    /// One `agentspec warning: {message}` line per group. Subjects are not
    /// listed, and `--verbose` changes nothing.
    Warning,
    /// A `{provider}: skipped {n} {singular|plural}` count line, plus one
    /// `{provider}: skipped {singular} {subject}` line per subject under
    /// `--verbose`. The noun travels with the kind so the renderer stays
    /// kind-agnostic.
    CountedSubjects {
        singular: &'static str,
        plural: &'static str,
    },
}

/// What an adapter's `removal_patches` step returns.
///
/// Mirrors [`AdapterOutput`] but for the reverse-direction (`remove`)
/// pipeline. `dest_root` is the adapter-computed sync-mode destination root
/// — same shape `compile` returns — so `remove_plan` can compute per-kind
/// `RemoveWrite` destinations without re-deriving paths. `patches` carries
/// the reverse-direction `ReversePatch` impls; their `run_remove` is called
/// after manifest-tracked file deletions.
#[derive(Debug)]
pub struct RemovalOutput {
    pub patches: Vec<Box<dyn ReversePatch>>,
    pub dest_root: PathBuf,
}

/// Provider-neutral adapter contract.
///
/// One primary entry point (`compile`) absorbs today's two-phase per-spec /
/// per-provider ordering. A second method (`removal_patches`) constructs the
/// reverse-direction patches for the `remove` pipeline (reverse direction has
/// no spec input — patches identify owned entries by on-disk sentinels). Two
/// accessor methods (`body_tool_name`, `model_facing_name`) survive because
/// templating needs them at spec-resolution time, before `compile` runs. The
/// capability accessors are each adapter's own claim about its runtime: an
/// adapter reads its own accessor at the point it drops a value, and pushes a
/// [`Degradation`] from there. `compile_specs` retains a single gate of its
/// own — the cross-provider parity gate no individual adapter has the input to
/// compute. That gate reads two accessors: `emits_hooks` to exclude providers
/// with no hooks to compare, then `session_start_fires_on_resume` on those
/// that remain.
///
/// Object-safe by design — `&dyn Adapter` is the dispatch shape used by
/// `Provider::adapter()`. No associated types.
///
/// `Send + Sync` mirrors today's adapter shape — every adapter is a
/// zero-sized unit struct and trivially `Send + Sync` — and the bound
/// future-proofs sub-paths where adapter-constructed `Box<dyn ForwardPatch>`
/// / `Box<dyn ReversePatch>` instances may directly hold `&'static dyn
/// Adapter` references (the patch traits' `Send + Sync` bounds cascade).
pub trait Adapter: std::fmt::Debug + Send + Sync {
    /// Compile every spec for one provider into output files and post-write
    /// patches.
    ///
    /// The adapter sequences per-spec adaptation and cross-spec aggregation
    /// (e.g., per-provider `hooks.json` synthesis or `OpenCode` `instructions[]`
    /// list construction) in whatever order makes sense internally. Today's
    /// implicit two-phase ordering in `compile_specs` collapses into the
    /// adapter's own implementation.
    fn compile(&self, specs: &[Spec], ctx: &CompileCtx<'_>) -> Result<AdapterOutput>;

    /// Construct the reverse-direction patches that the `remove` pipeline runs
    /// after manifest-tracked file deletions, plus the per-provider sync
    /// destination root used to compute per-kind `RemoveWrite` destinations.
    ///
    /// No spec input — owned entries are identified via on-disk
    /// `_agentspec_id` sentinels. Adapters that have no removal-side cleanup
    /// return an empty `patches` vector but always supply `dest_root`.
    fn removal_patches(&self, ctx: &RemoveCtx<'_>) -> RemovalOutput;

    /// Resolve a canonical tool to the body-level name this provider expects
    /// in spec content (e.g. Claude's `"Read"`, Cursor's `"Read files"`,
    /// `OpenCode`'s `"read"`).
    fn body_tool_name(&self, tool: &ToolFrontmatter) -> &'static str;

    /// Resolve a canonical tool to the name this provider expects in hook
    /// matcher values for tool-execute events (`PreToolUse`, `PostToolUse`,
    /// `PostToolUseFailure`).
    ///
    /// Returns `Some(name)` for tools the provider supports, `None` for
    /// tools with no provider equivalent. `None` causes the canonical
    /// token to pass through unchanged in the translated matcher string.
    ///
    /// Defaults to `Some(body_tool_name)`, which is correct for providers
    /// where body references and matcher identifiers coincide (Claude).
    /// Providers where they differ (Cursor: display labels vs.
    /// function-call identifiers, plus absent tools) must override.
    fn matcher_tool_name(&self, tool: &ToolFrontmatter) -> Option<&'static str> {
        Some(self.body_tool_name(tool))
    }

    /// Resolve a canonical subagent-type name to the string this provider
    /// expects in hook matcher values for subagent events (`SubagentStart`,
    /// `SubagentStop`).
    ///
    /// Canonical types: `general`, `explore`, `plan`. Unrecognized names
    /// pass through unchanged (handles provider-specific and custom types).
    ///
    /// Default: returns the input unchanged. Providers with non-canonical
    /// subagent-type naming (Claude, Cursor) override.
    fn matcher_subagent_type<'a>(&self, canonical: &'a str) -> &'a str {
        canonical
    }

    /// Returns the name which should be used to refer to the spec in the generated body content.
    fn body_spec_name(&self, spec: &Spec, cfg: Option<&AdapterConfig>) -> String;

    /// The root directory marker for skills to reference their path. Used for including
    /// references to scripts in skill content. Implementations must not include a trailing slash.
    fn body_skill_root(&self) -> Option<&'static str>;

    /// The settings this adapter can express on `kind`.
    ///
    /// A claim about the provider's schema, not about any spec.
    ///
    /// `test_carriable_agrees_with_carried` is the sole consumer of the claim:
    /// it compares this table against what the frontmatter struct for `kind`
    /// actually records, in both directions. A declared setting with no field
    /// to hold it and no composer writing it is a bug in this table; a
    /// recorded delivery whose setting this table omits is a bug the other
    /// way.
    ///
    /// The table is also what `README.md`'s prose about which spec values
    /// reach which provider is written against — the prose is hand-maintained,
    /// and this is the declaration a reader checks it against.
    ///
    /// Returns an empty slice for a [`FileKind`] this adapter never emits.
    fn carriable(&self, kind: FileKind) -> &'static [SettingKind];

    /// Whether this provider emits hook entries.
    ///
    /// Today: `true` for Claude / Cursor, `false` for `OpenCode` (which has
    /// no `hooks.json` analog and silently drops `Spec::Hook` inputs). The
    /// adapter consults this at its own `Spec::Hook` arm and pushes a
    /// [`DegradationKind::HooksUnsupported`] for each spec it drops.
    /// `compile_specs` retains one read, to exclude hookless providers from
    /// the session-start parity gate before comparing resume behavior.
    ///
    /// Capability accessor (not provider-knowledge leakage): adapters expose
    /// what kinds of output they support, callers iterate without branching
    /// on `Provider`.
    fn emits_hooks(&self) -> bool;

    /// Directory name under the sync destination root where this provider's
    /// plugin manifest file lives (e.g. `".claude-plugin"` for Claude,
    /// `".cursor-plugin"` for Cursor). Returns `None` for providers without
    /// a plugin concept (e.g. `OpenCode`), in which case the orchestrator
    /// skips `FileKind::PluginManifest` writes and removals for that provider.
    ///
    /// Capability accessor — parallel to [`Adapter::emits_hooks`]. The
    /// `Option` shape collapses "does this provider support plugin manifests"
    /// and "what directory does its manifest live in" into a single method.
    fn plugin_manifest_dir(&self) -> Option<&'static str>;

    /// Whether this provider's hook host runtime fully implements the
    /// canonical output schema's UI-facing / agent-facing / context-injection
    /// fields. Defaults to `true`; providers with documented partial
    /// implementations override to `false` and push a
    /// [`DegradationKind::PartialOutputImpl`] from their own compile walk when
    /// they are handed a hook spec they won't fully honour for
    /// `user_facing_message` / `decision_reason` / `additional_context`.
    ///
    /// Capability accessor — the adapter reads its own claim at the point it
    /// drops the value, so no `match provider { ... }` is needed in
    /// `compile_specs`.
    ///
    /// This value is a claim about a provider's runtime, so it is probe-backed
    /// rather than inferred. Cursor's `false` is measured by
    /// `experiments/cursor-gate-19-output-json` (what a deny JSON surfaces
    /// alongside `exit 2`) and `experiments/cursor-gate-21-plain-stdout`
    /// (whether plain stdout injects as context). Before changing this for any
    /// provider, run or write the probe — and see
    /// `experiments/README.md#the-life-of-a-refutation` for keeping the
    /// manifests and this accessor from drifting apart.
    fn fully_implements_canonical_output(&self) -> bool {
        true
    }

    /// Whether this provider's `session_start` hook fires on conversation
    /// resume (as well as on initial conversation creation). Defaults to
    /// `true`; providers that only fire on creation override to `false`.
    /// The compile stage surfaces a cross-provider asymmetry warning when
    /// hook specs target multiple providers that disagree on this value
    /// (e.g. Claude + Cursor for the same `session_start` hook).
    ///
    /// Capability accessor — keeps the cross-provider parity gate from
    /// naming any specific provider in `compile_specs`.
    ///
    /// Both sides of the asymmetry are probe-backed: the `true` default is
    /// measured for Claude by `experiments/claude-session-start`, and Cursor's
    /// `false` override by `experiments/cursor-session-start`. Note that the
    /// default is *inherited* by providers with no probe of their own —
    /// `OpenCode`'s `true` is untested, not measured.
    fn session_start_fires_on_resume(&self) -> bool {
        true
    }

    /// Whether this provider supports path-scoped rules (rules that
    /// activate only when files matching a glob pattern are in context).
    /// Defaults to `true`; providers without native path scoping override to
    /// `false` and push a [`DegradationKind::PathScopedRulesUnsupported`] from
    /// their own `Spec::Rule` arm for each path-scoped rule they flatten to
    /// always-on. The push is naive — once per offending rule — and the drain
    /// point's `BTreeSet` collapses it to one rendered warning.
    fn supports_path_scoped_rules(&self) -> bool {
        true
    }

    /// Render the command a provider would receive for this hook, using the
    /// `Bundled` anchor. `Bundled` is the one mode whose anchor is a literal
    /// `${<PLUGIN_ROOT>}` rather than a resolved path, so the preview needs
    /// no sync configuration; argument quoting is identical in all modes.
    ///
    /// `script` is the raw `hooks.toml` `script` path (still carrying its
    /// `scripts/` prefix) — each implementation normalizes it via
    /// `hook_compile::script_filename` internally, the same way
    /// `build_emitted_hook_entries` does. Callers never derive the filename
    /// themselves, which is what keeps `scripts/scripts/` from ever
    /// appearing in a preview.
    fn hook_command_preview(
        &self,
        event: HookEvent,
        script: &Path,
        hook_id: &str,
        args: &[String],
    ) -> String;

    /// Construct reverse patches for all discoverable host-config paths.
    ///
    /// Used by the `prune` command to strip orphaned `_agentspec_id`-tagged
    /// entries without requiring sync configuration. Scans deterministic
    /// user-mode and project-mode paths; skips any path that doesn't exist
    /// on disk.
    fn prune_patches(&self, home: &Path, cwd: &Path) -> Vec<Box<dyn ReversePatch>>;

    /// Subdirectory name under the destination root for `kind`.
    ///
    /// Single source of truth for the per-`FileKind` directory mapping. The
    /// five "static" kinds map to their canonical names; `PluginManifest`
    /// delegates to [`Adapter::plugin_manifest_dir`] so providers without a
    /// plugin concept (`OpenCode`) return `None` and the orchestrator skips
    /// the write. Provided as a trait default — adapters shouldn't override
    /// it (the five static names are not provider-specific knowledge), but
    /// they're free to if a future provider has a non-standard layout.
    fn dir_for_kind(&self, kind: FileKind) -> Option<&'static str> {
        match kind {
            FileKind::Agents => Some("agents"),
            FileKind::Commands => Some("commands"),
            FileKind::Rules => Some("rules"),
            FileKind::Skills => Some("skills"),
            FileKind::Hooks => Some("hooks"),
            FileKind::PluginManifest => self.plugin_manifest_dir(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet, HashMap};
    use std::path::{Path, PathBuf};

    use indexmap::IndexMap;
    use strum::VariantArray as _;

    use super::{CompileCtx, Degradation, DegradationKind, SyncDestinationMode};
    use crate::plan::FileKind;
    use crate::presets::{
        ClaudeEffort, ClaudePreset, CursorPreset, OpenCodePreset, ProviderPresets,
        ProviderPresetsMap,
    };
    use crate::provider::Provider;
    use crate::setting::{SettingKey, SettingKind};
    use crate::spec::{
        AgentFrontmatter, AgentSpec, CapabilitiesFrontmatter, ExecutionFrontmatter, HookEvent,
        HookFrontmatter, HookSpec, RuleFrontmatter, RuleSpec, SkillFrontmatter, SkillSpec, Spec,
        ToolFrontmatter,
    };

    const PRESET: &str = "maximal";

    /// A preset configuring every option each provider can express, so that a
    /// `carriable` entry left undelivered is a bug in the table rather than a
    /// gap in the fixture.
    fn maximal_presets() -> ProviderPresetsMap {
        HashMap::from([(
            PRESET.to_owned(),
            ProviderPresets {
                claude: Some(ClaudePreset {
                    model: Some("claude-opus-5".to_owned()),
                    effort: Some(ClaudeEffort::High),
                }),
                cursor: Some(CursorPreset {
                    model: Some("claude-opus-5".to_owned()),
                    effort: Some("high".to_owned()),
                    fast: Some(false),
                    context: Some("300k".to_owned()),
                    params: BTreeMap::from([("optimize_for".to_owned(), "cost".to_owned())]),
                }),
                opencode: Some(OpenCodePreset {
                    model: Some("anthropic/claude-opus-5".to_owned()),
                    variant: Some("thinking".to_owned()),
                }),
            },
        )])
    }

    fn skill(id: &str, user_invocable: bool, agent_invocable: bool) -> Spec {
        Spec::Skill(SkillSpec {
            path: PathBuf::from("test.md"),
            frontmatter: SkillFrontmatter {
                id: id.to_owned(),
                description: Some("Test skill".to_owned()),
                tags: None,
                user_invocable,
                agent_invocable,
                execution: Some(ExecutionFrontmatter {
                    preset: Some(PRESET.to_owned()),
                }),
                capabilities: Some(CapabilitiesFrontmatter {
                    tools: Some(vec![ToolFrontmatter::Read]),
                }),
            },
            body: "Body.".to_owned(),
            supporting_files: IndexMap::new(),
        })
    }

    /// One agent, three skills spanning the invocation-flag combinations, a
    /// path-scoped rule, and a hook — the union covers every `FileKind` any
    /// adapter emits.
    fn spec_set() -> Vec<Spec> {
        vec![
            Spec::Agent(AgentSpec {
                path: PathBuf::from("test.md"),
                frontmatter: AgentFrontmatter {
                    id: "maximal-agent".to_owned(),
                    description: "Test agent".to_owned(),
                    tags: None,
                    execution: Some(ExecutionFrontmatter {
                        preset: Some(PRESET.to_owned()),
                    }),
                    capabilities: Some(CapabilitiesFrontmatter {
                        tools: Some(vec![ToolFrontmatter::Read]),
                    }),
                },
                body: "Body.".to_owned(),
            }),
            skill("dual-skill", true, true),
            skill("command-only-skill", true, false),
            skill("skill-only-skill", false, true),
            Spec::Rule(RuleSpec {
                path: PathBuf::from("test.md"),
                frontmatter: RuleFrontmatter {
                    id: "scoped-rule".to_owned(),
                    description: Some("Test rule".to_owned()),
                    tags: None,
                    paths: Some(vec!["src/**".to_owned()]),
                },
                body: "Body.".to_owned(),
            }),
            Spec::Hook(HookSpec {
                path: PathBuf::from("/tmp/hooks.toml"),
                frontmatter: HookFrontmatter {
                    id: "startup".to_owned(),
                    events: vec![HookEvent::SessionStart],
                    script: PathBuf::from("scripts/startup.sh"),
                    matcher: None,
                    timeout: None,
                    description: None,
                    tags: None,
                    args: None,
                },
                body: String::new(),
                supporting_files: IndexMap::new(),
            }),
        ]
    }

    /// The claim `carriable` makes is checked against what adapters actually
    /// record, in both directions, and nowhere else. A declared setting with
    /// no field to hold it fails the second assertion; a recorded delivery the
    /// table omits fails the first. Together they exclude the state a
    /// report-time "schema limit or adapter bug" verdict would have described,
    /// which is why the loss report carries no such verdict.
    #[test]
    fn test_carriable_agrees_with_carried() {
        let specs = spec_set();
        let presets = maximal_presets();
        // `SyncDestinationMode::Compile` maps to `HookEmitMode::Bundled`, so
        // Claude and Cursor emit `hooks/hooks.json`. Under a merged mode they
        // would emit no hook file at all and the emitted-kind assertion below
        // would wrongly demand an empty `carriable(Hooks)`.
        let ctx = CompileCtx {
            mode: SyncDestinationMode::Compile,
            home: Path::new("/tmp/home"),
            cwd: Path::new("/tmp/cwd"),
            target_dir: None,
            presets: &presets,
            adapter_config: None,
            overwrite: false,
        };

        for provider in Provider::VARIANTS {
            let adapter = provider.adapter();
            let output = adapter.compile(&specs, &ctx).expect("compile");

            for delivery in &output.deliveries {
                let declared = adapter.carriable(delivery.kind());
                assert!(
                    declared.contains(&delivery.setting().kind()),
                    "{provider} recorded {:?} on a {} file but carriable({}) omits it",
                    delivery.setting(),
                    delivery.kind(),
                    delivery.kind(),
                );
            }

            for &kind in FileKind::all() {
                let emitted_any = output.files.iter().any(|f| f.kind == kind);
                let declared = adapter.carriable(kind);
                if !emitted_any {
                    assert!(
                        declared.is_empty(),
                        "{provider} emitted no {kind} file but carriable({kind}) declares {declared:?}"
                    );
                    continue;
                }
                // `Body` is not a frontmatter field, so no `carried()` records
                // it. Its delivery is the emitted file itself — a spec-owned
                // `GeneratedFile` of this kind, or, for hooks, a registration
                // recorded off an `EmittedHookEntry`, because no hook file
                // names a single spec. Checking it against those rather than
                // skipping it is what stops a table omitting `Body` — the most
                // severe loss class there is — from passing.
                let body_delivered = output
                    .files
                    .iter()
                    .any(|f| f.kind == kind && f.spec_id.is_some())
                    || output
                        .deliveries
                        .iter()
                        .any(|d| d.kind() == kind && *d.setting() == SettingKey::Body);
                assert_eq!(
                    declared.contains(&SettingKind::Body),
                    body_delivered,
                    "{provider}: carriable({kind}) declares Body = {}, but a spec body {} reach a {kind} file",
                    declared.contains(&SettingKind::Body),
                    if body_delivered { "does" } else { "does not" },
                );

                for declared_kind in declared {
                    if matches!(*declared_kind, SettingKind::Body) {
                        continue;
                    }
                    assert!(
                        output
                            .deliveries
                            .iter()
                            .any(|d| d.kind() == kind && d.setting().kind() == *declared_kind),
                        "{provider} declares {declared_kind:?} on {kind} but recorded no such delivery"
                    );
                }
            }
        }
    }

    #[test]
    fn test_degradation_set_collapses_duplicate_provider_wide_pushes() {
        // Cardinality policy lives at the drain point: an adapter pushes once
        // per occurrence and the `BTreeSet` decides whether that collapses.
        let set: BTreeSet<Degradation> = (0..3)
            .map(|_| {
                Degradation::provider_wide(
                    Provider::OpenCode,
                    DegradationKind::PathScopedRulesUnsupported,
                )
            })
            .collect();
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_degradation_set_orders_subjects_alphabetically() {
        // Derived `Ord` compares `provider`, then `kind`, then `subject`, so a
        // set built from out-of-order pushes iterates in subject order.
        let set: BTreeSet<Degradation> = ["c", "a", "b"]
            .into_iter()
            .map(|id| {
                Degradation::for_spec(Provider::OpenCode, id, DegradationKind::HooksUnsupported)
            })
            .collect();
        let subjects: Vec<Option<&str>> = set.iter().map(Degradation::subject).collect();
        assert_eq!(subjects, vec![Some("a"), Some("b"), Some("c")]);
    }
}
