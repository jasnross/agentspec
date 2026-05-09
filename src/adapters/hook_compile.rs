//! Shared helpers used by every hook-emitting adapter to shape canonical
//! hook entries and supporting-script `GeneratedFile`s.
//!
//! The helpers are provider-neutral — they take `Provider` as a parameter and
//! produce uniform output regardless of which adapter is calling. They live
//! alongside the adapter modules that call them so the dependency direction
//! is one-way (`compile.rs` orchestrates adapters; adapters and their helpers
//! do not call back into `compile.rs`).

use std::path::{Component, Path, PathBuf};

use crate::compile::{EmittedHookEntry, GeneratedFile, HookEmitMode};
use crate::provider::Provider;
use crate::spec::HookSpec;

/// Build the per-provider `Vec<GeneratedFile>` for every file under
/// `spec/hooks/scripts/`, taken from the first hook spec.
///
/// `load_hook_specs` attaches the same `supporting_files` list to every hook
/// spec parsed from a single `hooks.toml`, so reading from `specs[0]` gives
/// the full set. Emitting once per provider here (rather than once per hook
/// in `adapt_hook_spec`) avoids duplicate file entries downstream.
pub(super) fn build_hook_script_files(
    provider: Provider,
    specs: &[&HookSpec],
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

/// Build canonical `EmittedHookEntry` rows from hook specs.
///
/// The `command` field's anchor depends on `(dotdir, emit_mode)`:
/// - Bundled (Path mode): `${CLAUDE_PLUGIN_ROOT}/hooks/scripts/<f>` for both
///   providers (Cursor aliases `${CLAUDE_PLUGIN_ROOT}` at plugin scope) — the
///   dotdir parameter is unused.
/// - `MergedUser`: `$HOME/<dotdir>/hooks/scripts/<f>` (`$HOME` not `~/...`
///   because Claude's hook-command runtime isn't documented to expand `~`).
/// - `MergedProject`: `${CLAUDE_PROJECT_DIR}/<dotdir>/hooks/scripts/<f>`.
///   Cursor's behavior with `${CLAUDE_PROJECT_DIR}` outside plugin scope is
///   not documented — must be verified empirically against a real Cursor
///   build before 1.0.
///
/// `dotdir` is supplied by the calling adapter (Claude passes `".claude"`,
/// Cursor passes `".cursor"`) so this helper carries no provider knowledge.
pub(super) fn build_emitted_hook_entries(
    specs: &[&HookSpec],
    dotdir: &str,
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
            let path_under_scripts: PathBuf = s
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
                command: hook_command_anchor(dotdir, emit_mode, &filename),
                timeout: s.frontmatter.timeout,
                agentspec_id: s.frontmatter.id.clone(),
            }
        })
        .collect()
}

/// Compute the `command` string for a hook entry given the dotdir and mode.
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
fn hook_command_anchor(dotdir: &str, emit_mode: HookEmitMode, filename: &str) -> String {
    if matches!(emit_mode, HookEmitMode::Bundled) {
        return format!("${{CLAUDE_PLUGIN_ROOT}}/hooks/scripts/{filename}");
    }
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
