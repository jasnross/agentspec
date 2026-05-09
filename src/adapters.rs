//! Provider adapter traits.
//!
//! Every provider-specific decision (file paths, frontmatter shapes, hook JSON
//! layout, post-write patchers) lives behind these two traits. Non-adapter
//! modules dispatch through `Provider::adapter()` / `Provider::hook_adapter()`
//! exclusively — see `.claude/rules/provider-logic-in-adapters.md`.

mod claude;
mod cursor;
mod hook_compile;
mod hooks_helpers;
mod opencode;

use std::path::{Path, PathBuf};

use anyhow::Result;
pub use claude::ClaudeAdapter;
pub use cursor::CursorAdapter;
use jsonc_parser::cst::CstObject;
pub use opencode::OpenCodeAdapter;

use crate::compile::{AdapterConfig, EmittedHookEntry, GeneratedFile, HookEmitMode, HookSynthesis};
use crate::plan::{ConfigPatch, FileKind, PostWriteHook};
use crate::presets::ProviderPresetsMap;
use crate::spec::{HookEvent, HookSpec, Spec, ToolFrontmatter};

/// Library-side mirror of the binary's `SyncMode`.
///
/// Defined here (with no clap or serde derives) so trait methods can stay in
/// the library while the binary owns the CLI/config-loading parts of
/// `SyncMode`. The binary translates at the boundary, paralleling the existing
/// `SyncMode → HookEmitMode` translation in `src/config.rs`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncDestinationMode {
    User,
    Project,
    Path,
}

impl SyncDestinationMode {
    /// Map to the `HookEmitMode` that controls per-provider hook emission
    /// shape. User/Project map to merged-mode patches; Path maps to a
    /// self-contained `hooks/hooks.json` bundle.
    ///
    /// This collapses the previous binary-side `SyncMode → HookEmitMode`
    /// translation into a library-side method so adapters can derive their
    /// emit mode directly from `CompileCtx.mode` without threading
    /// `AdapterConfig.hook_emit_mode` through.
    pub fn to_hook_emit_mode(self) -> HookEmitMode {
        match self {
            Self::User => HookEmitMode::MergedUser,
            Self::Project => HookEmitMode::MergedProject,
            Self::Path => HookEmitMode::Bundled,
        }
    }
}

/// Provider-neutral adapter contract.
///
/// Every provider-specific decision lives behind this trait. Non-adapter
/// modules MUST dispatch through `Provider::adapter()` rather than naming a
/// specific adapter; the only exceptions are tests (which are exempt per the
/// project rule) and `Provider::adapter()` itself.
///
/// `Send + Sync` are supertrait bounds so `&'static dyn ProviderAdapter` /
/// `&'static dyn HookAdapter` references stored inside `Box<dyn ConfigPatch>`
/// satisfy the `ConfigPatch: Send + Sync` requirement during the bridge
/// phase. All current adapters (`ClaudeAdapter`, `CursorAdapter`,
/// `OpenCodeAdapter`) are zero-sized unit structs and trivially `Send + Sync`.
pub trait ProviderAdapter: Send + Sync {
    /// Adapt one spec into provider-specific generated files.
    fn adapt(
        &self,
        spec: Spec,
        presets: &ProviderPresetsMap,
        cfg: Option<&AdapterConfig>,
    ) -> Result<Vec<GeneratedFile>>;

    /// Resolve a canonical tool to the body-level name this provider expects
    /// in spec content (e.g. Claude's `"Read"`, Cursor's `"Read files"`,
    /// `OpenCode`'s `"read"`).
    fn body_tool_name(&self, tool: &ToolFrontmatter) -> &'static str;

    /// Compute the model-facing name for a spec (with prefix transforms applied).
    fn model_facing_name(&self, spec: &Spec, cfg: Option<&AdapterConfig>) -> String;

    /// Optional post-write hook for the sync pipeline.
    ///
    /// Each provider returns `Some(...)` only for the kinds it cares about
    /// (Claude/Cursor key off `Hooks` in merged modes; `OpenCode` keys off
    /// `Rules`). Non-matching kinds and modes return `None`.
    fn post_write_hook(
        &self,
        kind: FileKind,
        dest: &Path,
        config_dir: &Path,
        emit_mode: HookEmitMode,
        owned_entries: &[EmittedHookEntry],
        overwrite: bool,
    ) -> Option<Box<dyn PostWriteHook>>;

    /// Optional post-write hook for the remove pipeline (the inverse of
    /// `post_write_hook`).
    fn remove_post_write_hook(
        &self,
        kind: FileKind,
        dest: &Path,
        config_dir: &Path,
        emit_mode: HookEmitMode,
    ) -> Option<Box<dyn PostWriteHook>>;

    /// File kinds this provider emits.
    fn file_kinds(&self) -> &'static [FileKind];

    /// User-level destination directory for a given file kind
    /// (e.g. `~/.claude/agents`, `~/.config/opencode/skills`).
    fn user_dest_dir(&self, home: &Path, kind: FileKind) -> PathBuf;

    /// Project-local destination directory for a given file kind.
    fn project_dest_dir(&self, cwd: &Path, kind: FileKind) -> PathBuf;

    /// Provider config directory used as the parent of post-write merge
    /// targets (`<config>/settings.json` for Claude, `<config>/hooks.json`
    /// for Cursor, `<config>/opencode.json` for `OpenCode`).
    fn config_dir(
        &self,
        mode: SyncDestinationMode,
        dir: Option<&str>,
        home: &Path,
        cwd: &Path,
    ) -> PathBuf;
}

/// Outcome of a per-provider tidy. The implementation mutates the supplied
/// top-level `CstObject` in place via `jsonc-parser`'s interior-mutability
/// API (the shell holds the `CstRootNode` for the duration of the tidy and
/// serializes it after) and reports (a) how many user-authored entries
/// survived (for the existing summary line) and (b) whether the file is
/// effectively empty per the provider's predicate, which drives the
/// delete-on-empty branch in the generic shell.
#[derive(Debug)]
pub struct TidyOutcome {
    pub user_entries_remaining: usize,
    pub file_should_be_deleted: bool,
}

/// Hook-emitting providers' contract.
///
/// `Provider::hook_adapter()` returns `Some(_)` only for providers that emit
/// hooks (Claude, Cursor today). `OpenCode` does not implement this trait.
///
/// `Debug` is a supertrait so the generic `HooksPatch` / `RemoveHooksPatch`
/// post-write structs in `hooks_merge` (which store `&'static dyn
/// HookAdapter`) can derive `Debug` and satisfy the `PostWriteHook` bound.
pub trait HookAdapter: ProviderAdapter + std::fmt::Debug + Send + Sync {
    /// Synthesize the per-provider hooks bundle (entries plus, in Bundled
    /// mode, the `hooks/hooks.json` file).
    fn synthesize_hooks(
        &self,
        specs: &[&HookSpec],
        cfg: Option<&AdapterConfig>,
    ) -> Result<HookSynthesis>;

    /// Translate a canonical `HookEvent` to the provider's event-name string.
    fn event_name(&self, event: HookEvent) -> &'static str;

    /// Per-entry JSON shape for the provider's `hooks.json` / `settings.json`.
    fn entry_to_json(&self, entry: &EmittedHookEntry) -> serde_json::Value;

    /// String-fragment dotdir embedded in hook command shell paths
    /// (e.g. `.claude` / `.cursor`). Scoped to `HookAdapter` because only the
    /// per-provider hook-command anchor builder needs the dotdir as a string
    /// fragment; every other path consumer uses the `PathBuf`-returning
    /// methods on `ProviderAdapter`.
    fn hook_command_dotdir(&self) -> &'static str;

    /// Filename within `<config_dir>/` that this provider's hook merge writes
    /// (e.g. `"settings.json"` for Claude, `"hooks.json"` for Cursor).
    fn host_filename(&self) -> &'static str;

    /// Merge agentspec-owned entries into a parsed top-level CST object.
    ///
    /// `top` is already parsed — the generic shell handles file I/O. The
    /// implementation owns every provider-specific shape decision: top-level
    /// extras (e.g. Cursor's `version: 1`), opening the `hooks` object, the
    /// per-event nesting depth, and the per-entry shape.
    ///
    /// `force` propagates through the `force`-aware helpers in
    /// `hooks_helpers` so non-object/non-array existing values can be
    /// replaced when `--force` is set.
    ///
    /// Implementations MUST NOT prune empty event arrays — locked by
    /// `test_merge_claude_leaves_empty_event_array_after_removing_all_owned_entries`.
    fn merge_into(
        &self,
        top: &CstObject,
        owned_entries: &[EmittedHookEntry],
        force: bool,
    ) -> Result<()>;

    /// Strip agentspec-owned entries from a parsed top-level CST object,
    /// prune emptied containers, and report whether the host file should be
    /// deleted.
    ///
    /// Implementations are responsible for the provider-specific
    /// delete-on-empty predicate — Claude requires zero surviving top-level
    /// keys; Cursor tolerates a residual `version` key. The generic shell
    /// uses `TidyOutcome::file_should_be_deleted` to decide whether to
    /// delete the host file vs. write the tidied CST back.
    fn tidy_after_remove(&self, top: &CstObject) -> TidyOutcome;
}

// ── New unified trait surface (Phase 2 of adapter API consolidation) ─────────
//
// The new `Adapter` trait below is the long-term replacement for
// `ProviderAdapter` + `HookAdapter`. During the bridge phase both surfaces
// coexist; adapter modules `impl` both. The orchestrator and downstream
// pipeline progressively migrate to call `Adapter::compile` /
// `Adapter::removal_patches`. The old surfaces are deleted once every
// production call site is migrated.

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
    /// `SyncDestinationMode::Path` with `target_dir: None`.
    pub mode: SyncDestinationMode,
    /// Effective home directory (for User-mode dest resolution).
    pub home: &'a Path,
    /// Effective current working directory (for Project-mode dest resolution).
    pub cwd: &'a Path,
    /// Explicit destination directory when `mode == Path`; `None` for User /
    /// Project modes (and for `compile`-command runs that don't target a sync
    /// destination).
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
    /// erroring. `false` for the `compile` command path. Plan-deviation
    /// addition: the original Plan-2 design omitted this field, but it's
    /// required for forward-direction patch construction.
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
/// the post-write patches to apply after files land — Claude/Cursor settings
/// merges, `OpenCode` instructions registration, etc. `dest_root` is the
/// adapter-computed sync-mode destination root that downstream `sync_plan`
/// uses to anchor each `(provider, kind)` `ManifestTrackedWrite`.
#[derive(Debug)]
pub struct AdapterOutput {
    pub files: Vec<GeneratedFile>,
    pub patches: Vec<Box<dyn ConfigPatch>>,
    pub dest_root: PathBuf,
}

/// Provider-neutral adapter contract — the consolidated successor to
/// `ProviderAdapter` + `HookAdapter`.
///
/// One primary entry point (`compile`) absorbs today's two-phase per-spec /
/// per-provider ordering. A second method (`removal_patches`) constructs the
/// reverse-direction patches for the `remove` pipeline (reverse direction has
/// no spec input — patches identify owned entries by on-disk sentinels, so no
/// `compile`-style spec list is threaded through). Two accessor methods
/// (`body_tool_name`, `model_facing_name`) survive because templating needs
/// them at spec-resolution time, before `compile` runs.
///
/// Object-safe by design — `&dyn Adapter` is the dispatch shape used by
/// `Provider::adapter()`. No associated types.
///
/// `Send + Sync` mirrors the supertrait bounds on `ProviderAdapter` — every
/// adapter today is a zero-sized unit struct and trivially `Send + Sync`, and
/// the bound future-proofs sub-step D when adapter-constructed
/// `Box<dyn ConfigPatch>` instances may directly hold `&'static dyn Adapter`
/// references (`ConfigPatch: Send + Sync` cascades the bound).
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
    /// after manifest-tracked file deletions.
    ///
    /// No spec input — owned entries are identified via on-disk
    /// `_agentspec_id` sentinels. Adapters that have no removal-side cleanup
    /// (e.g., the file-only branches of any adapter) return an empty `Vec`.
    fn removal_patches(&self, ctx: &RemoveCtx<'_>) -> Vec<Box<dyn ConfigPatch>>;

    /// Resolve a canonical tool to the body-level name this provider expects
    /// in spec content (e.g. Claude's `"Read"`, Cursor's `"Read files"`,
    /// `OpenCode`'s `"read"`).
    fn body_tool_name(&self, tool: &ToolFrontmatter) -> &'static str;

    /// Compute the model-facing name for a spec (with prefix transforms applied).
    fn model_facing_name(&self, spec: &Spec, cfg: Option<&AdapterConfig>) -> String;

    /// File kinds this provider emits — the per-provider set used by `sync`
    /// and `remove` to drive `ManifestTrackedWrite` / `RemoveWrite`
    /// construction (one batch per `(provider, kind)` even when empty, so
    /// stale manifest entries are cleaned up).
    ///
    /// Bridge-phase carry-over: this method survives sub-step C so
    /// `sync_plan` / `remove_plan` can keep their per-kind inner loop without
    /// branching on `Provider`. The plan's end-state shape derives the kind
    /// set from `GeneratedFile.kind` instead — sub-step D will replace this
    /// accessor once `GeneratedFile` carries an explicit `kind` field.
    fn file_kinds(&self) -> &'static [FileKind];

    /// Sync-mode destination root for the `remove` pipeline.
    ///
    /// Mirrors `AdapterOutput.dest_root` (which `compile` returns for the
    /// sync pipeline) so `remove_plan` — which does not call `compile` —
    /// can compute per-kind `RemoveWrite` destinations without branching on
    /// `Provider`. Bridge-phase accessor; sub-step D folds this into a
    /// richer `removal_patches` return type.
    fn remove_dest_root(&self, ctx: &RemoveCtx<'_>) -> PathBuf;

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
}
