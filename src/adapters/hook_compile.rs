//! Shared helpers used by every hook-emitting adapter to shape canonical
//! hook entries and supporting-script `GeneratedFile`s.
//!
//! The helpers are provider-neutral — they take `Provider` as a parameter and
//! produce uniform output regardless of which adapter is calling. They live
//! alongside the adapter modules that call them so the dependency direction
//! is one-way (`compile.rs` orchestrates adapters; adapters and their helpers
//! do not call back into `compile.rs`).

use std::path::{Component, Path, PathBuf};

use anyhow::Result;

use crate::compile::{EmittedHookEntry, GeneratedFile, HookEmitMode};
use crate::plan::FileKind;
use crate::provider::Provider;
use crate::spec::HookSpec;

/// Per-provider hook synthesis result — entries (always populated when there
/// are hook specs) plus the supporting-script and bundled-JSON files this
/// provider emits.
#[derive(Debug, Default)]
pub(super) struct HookSynthesis {
    pub entries: Vec<EmittedHookEntry>,
    pub files: Vec<GeneratedFile>,
}

/// Build the `EmittedHookEntry` list and supporting `GeneratedFile`s for a
/// hook-emitting provider.
///
/// Returns an empty `HookSynthesis` when there are no hook specs. In merged
/// modes, `entries` is populated for the post-write patcher to consume but
/// `files` omits `hooks/hooks.json` (the patcher edits the host config file
/// instead). Bundled mode owns the whole `hooks/hooks.json` and uses
/// `build_bundled_json` to serialize it in the provider's expected shape.
///
/// `plugin_root_env_var` is the host runtime variable each adapter passes
/// in for plugin-scope path anchoring — `"CLAUDE_PLUGIN_ROOT"` for Claude,
/// `"CURSOR_PLUGIN_ROOT"` for Cursor. This shared helper carries no
/// provider knowledge; the adapter is the source of truth for which env var
/// name its host runtime exposes.
pub(super) fn synthesize_hooks<F>(
    provider: Provider,
    dotdir: &str,
    plugin_root_env_var: &'static str,
    specs: &[&HookSpec],
    emit_mode: HookEmitMode,
    build_bundled_json: F,
) -> Result<HookSynthesis>
where
    F: FnOnce(&[EmittedHookEntry]) -> Result<String>,
{
    if specs.is_empty() {
        return Ok(HookSynthesis::default());
    }

    let entries = build_emitted_hook_entries(specs, dotdir, plugin_root_env_var, emit_mode);
    let mut files = build_hook_script_files(provider, specs);
    if matches!(emit_mode, HookEmitMode::Bundled) {
        let json = build_bundled_json(&entries)?;
        files.push(GeneratedFile::text(
            provider,
            FileKind::Hooks,
            Path::new("hooks").join("hooks.json"),
            json,
        ));
    }
    Ok(HookSynthesis { entries, files })
}

/// Build the per-provider `Vec<GeneratedFile>` for every file under
/// `spec/hooks/scripts/`, taken from the first hook spec.
///
/// `load_hook_specs` attaches the same `supporting_files` list to every hook
/// spec parsed from a single `hooks.toml`, so reading from `specs[0]` gives
/// the full set. Emitting once per provider here (rather than once per hook
/// in `adapt_hook_spec`) avoids duplicate file entries downstream.
fn build_hook_script_files(provider: Provider, specs: &[&HookSpec]) -> Vec<GeneratedFile> {
    let Some(first) = specs.first() else {
        return Vec::new();
    };
    first
        .supporting_files
        .iter()
        .map(|sf| {
            GeneratedFile::binary(
                provider,
                FileKind::Hooks,
                Path::new("hooks").join(&sf.relative_path),
                sf.content.clone(),
                Some(sf.mode),
            )
        })
        .collect()
}

/// Build canonical `EmittedHookEntry` rows from hook specs.
///
/// The `command` field's anchor depends on `(dotdir, plugin_root_env_var,
/// emit_mode)`:
/// - Bundled (Plugin / compile-output mode): `${<plugin_root_env_var>}/hooks/scripts/<f>`
///   — the host runtime sets this variable to the plugin root, so the
///   command can reference the script directly. The `dotdir` parameter is
///   unused in this mode.
/// - `MergedUser`: `<plugin_root_env_var>=$HOME/<dotdir> $HOME/<dotdir>/hooks/scripts/<f>`.
///   `$HOME` not `~/...` because Claude's hook-command runtime isn't
///   documented to expand `~`.
/// - `MergedProject`: `<plugin_root_env_var>=${CLAUDE_PROJECT_DIR}/<dotdir>
///   ${CLAUDE_PROJECT_DIR}/<dotdir>/hooks/scripts/<f>`. Cursor's behavior
///   with `${CLAUDE_PROJECT_DIR}` outside plugin scope is not documented —
///   must be verified empirically against a real Cursor build before 1.0.
///
/// `dotdir` and `plugin_root_env_var` are supplied by the calling adapter
/// (Claude: `".claude"` + `"CLAUDE_PLUGIN_ROOT"`; Cursor: `".cursor"` +
/// `"CURSOR_PLUGIN_ROOT"`) so this helper carries no provider knowledge.
fn build_emitted_hook_entries(
    specs: &[&HookSpec],
    dotdir: &str,
    plugin_root_env_var: &'static str,
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
                command: hook_command_anchor(dotdir, plugin_root_env_var, emit_mode, &filename),
                timeout: s.frontmatter.timeout,
                agentspec_id: s.frontmatter.id.clone(),
            }
        })
        .collect()
}

/// Compute the `command` string for a hook entry given the dotdir, the
/// per-provider plugin-root env var name, and the emit mode.
///
/// In Bundled (plugin or compile-output) mode the host runtime sets
/// `${<plugin_root_env_var>}` to the plugin root, so we just reference the
/// script directly. In Merged (User/Project) modes the host doesn't set
/// that variable — but hook scripts authored for the plugin distribution
/// model commonly reference `${<plugin_root_env_var>}/rules`,
/// `${<plugin_root_env_var>}/skills`, etc. to find sibling assets. We assign
/// it inline (`FOO=bar cmd`, standard POSIX) so plugin-shaped scripts keep
/// working when synced project/user-wide. The assigned value is the config
/// dir (e.g., `$HOME/.claude` for User mode), where agentspec also writes
/// those sibling kinds.
fn hook_command_anchor(
    dotdir: &str,
    plugin_root_env_var: &'static str,
    emit_mode: HookEmitMode,
    filename: &str,
) -> String {
    match emit_mode {
        HookEmitMode::Bundled => {
            format!("${{{plugin_root_env_var}}}/hooks/scripts/{filename}")
        }
        HookEmitMode::MergedUser => {
            let cd = format!("$HOME/{dotdir}");
            format!("{plugin_root_env_var}={cd} {cd}/hooks/scripts/{filename}")
        }
        HookEmitMode::MergedProject => {
            let cd = format!("${{CLAUDE_PROJECT_DIR}}/{dotdir}");
            format!("{plugin_root_env_var}={cd} {cd}/hooks/scripts/{filename}")
        }
    }
}
