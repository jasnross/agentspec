//! Provider adapter traits.
//!
//! Every provider-specific decision (file paths, frontmatter shapes, hook JSON
//! layout, post-write patchers) lives behind these two traits. Non-adapter
//! modules dispatch through `Provider::adapter()` / `Provider::hook_adapter()`
//! exclusively — see `.claude/rules/provider-logic-in-adapters.md`.

mod claude;
mod cursor;
mod opencode;

use std::path::{Path, PathBuf};

use anyhow::Result;
pub use claude::ClaudeAdapter;
pub use cursor::CursorAdapter;
pub use opencode::OpenCodeAdapter;

use crate::compile::{AdapterConfig, EmittedHookEntry, GeneratedFile, HookEmitMode, HookSynthesis};
use crate::plan::{FileKind, PostWriteHook};
use crate::presets::ProviderPresetsMap;
use crate::spec::{HookEvent, NormalizedHookSpec, NormalizedSpec, ToolFrontmatter};

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
    /// Adapt one normalized spec into provider-specific generated files.
    fn adapt(
        &self,
        spec: NormalizedSpec,
        presets: &ProviderPresetsMap,
        cfg: Option<&AdapterConfig>,
    ) -> Result<Vec<GeneratedFile>>;

    /// Resolve a canonical tool to the body-level name this provider expects
    /// in spec content (e.g. Claude's `"Read"`, Cursor's `"Read files"`,
    /// `OpenCode`'s `"read"`).
    fn body_tool_name(&self, tool: &ToolFrontmatter) -> &'static str;

    /// Compute the model-facing name for a spec (with prefix transforms applied).
    fn model_facing_name(&self, spec: &NormalizedSpec, cfg: Option<&AdapterConfig>) -> String;

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

/// Hook-emitting providers' contract.
///
/// `Provider::hook_adapter()` returns `Some(_)` only for providers that emit
/// hooks (Claude, Cursor today). `OpenCode` does not implement this trait.
pub trait HookAdapter: ProviderAdapter {
    /// Synthesize the per-provider hooks bundle (entries plus, in Bundled
    /// mode, the `hooks/hooks.json` file).
    fn synthesize_hooks(
        &self,
        specs: &[&NormalizedHookSpec],
        cfg: Option<&AdapterConfig>,
    ) -> Result<HookSynthesis>;

    /// Translate a canonical `HookEvent` to the provider's event-name string.
    fn event_name(&self, event: HookEvent) -> &'static str;

    /// Per-entry JSON shape for the provider's `hooks.json` / `settings.json`.
    fn entry_to_json(&self, entry: &EmittedHookEntry) -> serde_json::Value;

    /// String-fragment dotdir embedded in hook command shell paths
    /// (e.g. `.claude` / `.cursor`). Scoped to `HookAdapter` because only
    /// `compile::hook_command_anchor` needs the dotdir as a string fragment;
    /// every other path consumer uses the `PathBuf`-returning methods on
    /// `ProviderAdapter`.
    fn hook_command_dotdir(&self) -> &'static str;
}
