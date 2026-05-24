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
use crate::spec::{Spec, ToolFrontmatter};

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
/// `ManifestTrackedWrite`.
#[derive(Debug)]
pub struct AdapterOutput {
    pub files: Vec<GeneratedFile>,
    pub patches: Vec<Box<dyn ForwardPatch>>,
    pub dest_root: PathBuf,
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
/// templating needs them at spec-resolution time, before `compile` runs. Two
/// capability accessors (`emits_hooks`) let the orchestrator branch on
/// per-provider feature support without naming individual providers.
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

    /// Whether this provider emits hook entries.
    ///
    /// Today: `true` for Claude / Cursor, `false` for `OpenCode` (which has
    /// no `hooks.json` analog and silently drops `Spec::Hook` inputs). The
    /// `compile_specs` orchestrator consults this to push `SkippedHook`
    /// diagnostics for hook specs that the active provider can't emit.
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
    /// implementations override to `false` so the compile stage can surface
    /// an `agentspec warning:` when hook specs target a provider that won't
    /// fully honour `user_facing_message` / `decision_reason` /
    /// `additional_context`.
    ///
    /// Capability accessor — keeps the warning-firing gate provider-opaque
    /// at the orchestrator level (no `match provider { ... }` in
    /// `compile_specs`).
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
    fn session_start_fires_on_resume(&self) -> bool {
        true
    }

    /// Whether this provider supports path-scoped rules (rules that
    /// activate only when files matching a glob pattern are in context).
    /// Defaults to `true`; providers without native path scoping override
    /// to `false`. The compile stage surfaces a per-provider portability
    /// warning when path-scoped rule specs target a provider that returns
    /// `false`.
    fn supports_path_scoped_rules(&self) -> bool {
        true
    }

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
