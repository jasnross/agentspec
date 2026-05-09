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
use crate::plan::{FileKind, PostWriteHook};
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

/// Provider-neutral adapter contract.
///
/// Every provider-specific decision lives behind this trait. Non-adapter
/// modules MUST dispatch through `Provider::adapter()` rather than naming a
/// specific adapter; the only exceptions are tests (which are exempt per the
/// project rule) and `Provider::adapter()` itself.
pub trait ProviderAdapter {
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
pub trait HookAdapter: ProviderAdapter + std::fmt::Debug {
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
