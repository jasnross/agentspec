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
use crate::hooks_canonical::{ProviderName, shim_template};
use crate::plan::FileKind;
use crate::provider::Provider;
use crate::spec::{HookEvent, HookSpec};

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
    files.extend(build_shim_files(provider, specs));
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
        .map(|(rel_path, sf)| {
            GeneratedFile::binary(
                provider,
                FileKind::Hooks,
                Path::new("hooks").join(rel_path),
                sf.content.clone(),
                Some(sf.mode),
            )
        })
        .collect()
}

/// Build the per-provider shim files — one per distinct [`HookEvent`] used
/// by `specs`, emitted under `hooks/scripts/_wrappers/<event>.sh`.
///
/// Shims are agentspec-generated canonical-translation wrappers. They are
/// emitted at Compile stage so each provider's tree gets its own shim with
/// the provider-specific `jq` programs baked in. Deduplication is
/// per-provider — N hook specs targeting the same event produce one shim
/// file per provider, even when the specs reference different user
/// scripts.
///
/// Returns an empty vector for providers that have no canonical wire form
/// (`Provider::OpenCode`). In practice `synthesize_hooks` is only called
/// from hook-emitting providers, so this branch is defensive rather than
/// load-bearing.
fn build_shim_files(provider: Provider, specs: &[&HookSpec]) -> Vec<GeneratedFile> {
    let Ok(canonical_provider) = ProviderName::try_from(provider) else {
        return Vec::new();
    };
    // Distinct events in source order (`Vec::contains` for dedup avoids
    // requiring `HookEvent: Hash`/`Ord` for what is typically a 1–5
    // element collection).
    let mut events: Vec<HookEvent> = Vec::new();
    for spec in specs {
        for &event in &spec.frontmatter.events {
            if !events.contains(&event) {
                events.push(event);
            }
        }
    }
    events
        .into_iter()
        .map(|event| {
            let relative_path = Path::new("hooks")
                .join("scripts")
                .join("_wrappers")
                .join(format!("{}.sh", event.snake_case()));
            GeneratedFile::binary(
                provider,
                FileKind::Hooks,
                relative_path,
                shim_template::shim_script(canonical_provider, event).into_bytes(),
                Some(0o755),
            )
        })
        .collect()
}

/// Build canonical `EmittedHookEntry` rows from hook specs.
///
/// Each entry's `command` field is computed by [`hook_command_anchor`],
/// which wraps the user script in a per-event shim invocation. The shape
/// is `<shim> <user_script>`, with both halves anchored under the same
/// per-mode base (plugin-root env var for Bundled; `$HOME/<dotdir>` for
/// `MergedUser`; `${CLAUDE_PROJECT_DIR}/<dotdir>` for `MergedProject`).
/// See [`hook_command_anchor`] for the full per-mode shape.
fn build_emitted_hook_entries(
    specs: &[&HookSpec],
    dotdir: &str,
    plugin_root_env_var: &'static str,
    emit_mode: HookEmitMode,
) -> Vec<EmittedHookEntry> {
    specs
        .iter()
        .flat_map(|s| {
            let path_under_scripts: PathBuf = s
                .frontmatter
                .script
                .components()
                .skip_while(|c| matches!(c, Component::CurDir))
                .skip(1)
                .collect();
            let filename = path_under_scripts.to_string_lossy().into_owned();
            let hook_id = s.frontmatter.id.clone();
            s.frontmatter
                .events
                .iter()
                .map(move |&event| EmittedHookEntry {
                    event,
                    matcher: s.frontmatter.matcher.clone(),
                    command: hook_command_anchor(
                        dotdir,
                        plugin_root_env_var,
                        emit_mode,
                        event,
                        &filename,
                        &hook_id,
                    ),
                    timeout: s.frontmatter.timeout,
                    agentspec_id: s.frontmatter.id.clone(),
                })
        })
        .collect()
}

/// Compute the `command` string for a hook entry given the dotdir, the
/// per-provider plugin-root env var name, the emit mode, the canonical
/// event, and the user script's filename.
///
/// The emitted command invokes the per-event shim with the user script's
/// path as its sole argument:
/// `<shim> <user_script>`, where `<shim>` resolves to
/// `<anchor>/hooks/scripts/_wrappers/<event>.sh` and `<user_script>`
/// resolves to `<anchor>/hooks/scripts/<filename>`. The anchor itself is
/// mode-dependent:
/// - Bundled (Plugin / compile-output mode): `${<plugin_root_env_var>}` —
///   the host runtime sets this variable to the plugin root.
/// - `MergedUser`: `$HOME/<dotdir>`, prefixed by an inline
///   `<plugin_root_env_var>=$HOME/<dotdir>` assignment so plugin-shaped
///   scripts that reference `${<plugin_root_env_var>}/...` for sibling
///   assets keep working at user scope.
/// - `MergedProject`: `${CLAUDE_PROJECT_DIR}/<dotdir>`, prefixed similarly.
///   Cursor's behavior with `${CLAUDE_PROJECT_DIR}` outside plugin scope
///   is not documented — must be verified empirically against a real
///   Cursor build before 1.0.
///
/// `dotdir`, `plugin_root_env_var`, and `event` are supplied by the
/// calling adapter or the spec — this helper carries no provider
/// knowledge.
fn hook_command_anchor(
    dotdir: &str,
    plugin_root_env_var: &'static str,
    emit_mode: HookEmitMode,
    event: HookEvent,
    filename: &str,
    hook_id: &str,
) -> String {
    let (env_assignment, anchor_base) = match emit_mode {
        HookEmitMode::Bundled => (String::new(), format!("${{{plugin_root_env_var}}}")),
        HookEmitMode::MergedUser => {
            let cd = format!("$HOME/{dotdir}");
            (format!("{plugin_root_env_var}={cd} "), cd)
        }
        HookEmitMode::MergedProject => {
            let cd = format!("${{CLAUDE_PROJECT_DIR}}/{dotdir}");
            (format!("{plugin_root_env_var}={cd} "), cd)
        }
    };
    let shim_path = format!(
        "{anchor_base}/hooks/scripts/_wrappers/{}.sh",
        event.snake_case()
    );
    let user_script_path = format!("{anchor_base}/hooks/scripts/{filename}");
    format!("{env_assignment}{shim_path} {user_script_path} {hook_id}")
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::*;
    use crate::spec::HookFrontmatter;

    fn hook_spec(id: &str, events: Vec<HookEvent>) -> HookSpec {
        HookSpec {
            path: PathBuf::from("/tmp/hooks.toml"),
            frontmatter: HookFrontmatter {
                id: id.to_string(),
                events,
                script: format!("scripts/{id}.sh").into(),
                matcher: None,
                timeout: None,
                description: None,
                tags: None,
            },
            body: String::new(),
            supporting_files: IndexMap::new(),
        }
    }

    #[test]
    fn build_shim_files_deduplicates_per_event_per_provider() {
        // Two specs both targeting `pre_tool_use` → exactly one shim file
        // per provider. Deduplication is provider-scoped: each provider's
        // tree gets its own copy with provider-specific jq programs.
        let a = hook_spec("audit-bash", vec![HookEvent::PreToolUse]);
        let b = hook_spec("audit-edit", vec![HookEvent::PreToolUse]);
        let specs: Vec<&HookSpec> = vec![&a, &b];

        let claude = build_shim_files(Provider::Claude, &specs);
        assert_eq!(claude.len(), 1, "expected 1 Claude shim, got {claude:?}");
        assert_eq!(claude[0].provider, Provider::Claude);
        assert!(
            claude[0]
                .path
                .ends_with("hooks/scripts/_wrappers/pre_tool_use.sh"),
            "unexpected Claude shim path: {:?}",
            claude[0].path
        );

        let cursor = build_shim_files(Provider::Cursor, &specs);
        assert_eq!(cursor.len(), 1, "expected 1 Cursor shim, got {cursor:?}");
        assert_eq!(cursor[0].provider, Provider::Cursor);
        assert!(
            cursor[0]
                .path
                .ends_with("hooks/scripts/_wrappers/pre_tool_use.sh"),
            "unexpected Cursor shim path: {:?}",
            cursor[0].path
        );

        // Provider-specific content: bytes differ even though path is
        // identically-shaped across providers.
        assert_ne!(
            claude[0].content, cursor[0].content,
            "Claude and Cursor shim bytes should differ"
        );
    }

    #[test]
    fn build_shim_files_empty_specs_returns_empty_vec() {
        // Empty input → no shims. Guards against accidentally emitting an
        // empty `_wrappers/` directory or stray files when no hooks are
        // configured.
        let specs: Vec<&HookSpec> = Vec::new();
        assert!(build_shim_files(Provider::Claude, &specs).is_empty());
        assert!(build_shim_files(Provider::Cursor, &specs).is_empty());
    }

    #[test]
    fn build_shim_files_opencode_returns_empty_vec() {
        // OpenCode's adapter doesn't emit hooks. Even if the helper is
        // accidentally called with `Provider::OpenCode`, it must return
        // an empty Vec rather than panicking or emitting incorrect output.
        let a = hook_spec("init", vec![HookEvent::SessionStart]);
        let specs: Vec<&HookSpec> = vec![&a];
        assert!(build_shim_files(Provider::OpenCode, &specs).is_empty());
    }

    #[test]
    fn build_shim_files_emits_one_per_distinct_event() {
        // Three specs across two events → 2 shims per provider. Source
        // order is preserved (the `Vec::contains` dedup keeps insertion
        // order), so iteration is deterministic.
        let a = hook_spec("a", vec![HookEvent::PreToolUse]);
        let b = hook_spec("b", vec![HookEvent::PreToolUse]);
        let c = hook_spec("c", vec![HookEvent::SessionStart]);
        let specs: Vec<&HookSpec> = vec![&a, &b, &c];
        let claude = build_shim_files(Provider::Claude, &specs);
        assert_eq!(claude.len(), 2);
        assert!(
            claude[0].path.ends_with("_wrappers/pre_tool_use.sh"),
            "first shim should be pre_tool_use, got: {:?}",
            claude[0].path
        );
        assert!(
            claude[1].path.ends_with("_wrappers/session_start.sh"),
            "second shim should be session_start, got: {:?}",
            claude[1].path
        );
    }

    #[test]
    fn build_emitted_hook_entries_expands_multi_event_spec() {
        let spec = hook_spec(
            "multi",
            vec![HookEvent::PreToolUse, HookEvent::SessionStart],
        );
        let specs: Vec<&HookSpec> = vec![&spec];
        let entries = build_emitted_hook_entries(
            &specs,
            ".claude",
            "CLAUDE_PLUGIN_ROOT",
            HookEmitMode::Bundled,
        );
        assert_eq!(
            entries.len(),
            2,
            "one spec with 2 events should produce 2 entries"
        );
        assert_eq!(entries[0].event, HookEvent::PreToolUse);
        assert_eq!(entries[1].event, HookEvent::SessionStart);
        assert_eq!(entries[0].agentspec_id, "multi");
        assert_eq!(entries[1].agentspec_id, "multi");
        assert!(entries[0].command.contains("_wrappers/pre_tool_use.sh"));
        assert!(entries[1].command.contains("_wrappers/session_start.sh"));
    }

    #[test]
    fn hook_command_anchor_bundled_invokes_shim_with_user_script_and_hook_id() {
        let cmd = hook_command_anchor(
            ".claude",
            "CLAUDE_PLUGIN_ROOT",
            HookEmitMode::Bundled,
            HookEvent::PreToolUse,
            "audit.sh",
            "audit-bash",
        );
        assert_eq!(
            cmd,
            "${CLAUDE_PLUGIN_ROOT}/hooks/scripts/_wrappers/pre_tool_use.sh ${CLAUDE_PLUGIN_ROOT}/hooks/scripts/audit.sh audit-bash"
        );
    }
}
