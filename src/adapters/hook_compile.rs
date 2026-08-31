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
use crate::spec::{HookEvent, HookSpec, ToolFrontmatter};

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

    let entries =
        build_emitted_hook_entries(specs, provider, dotdir, plugin_root_env_var, emit_mode);
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

/// Translate a matcher string from canonical token names to the
/// provider-specific names the adapter expects.
///
/// Event-type-aware: tool-execute events dispatch through
/// `matcher_tool_name` (typed `ToolFrontmatter` parsing), subagent events
/// dispatch through `matcher_subagent_type` (string-to-string mapping),
/// and all other events pass through unchanged.
///
/// Non-canonical tokens (MCP tool names, provider-specific names, custom
/// subagent types) pass through unchanged.
fn translate_matcher(adapter: &dyn super::Adapter, event: HookEvent, matcher: &str) -> String {
    if !event.allows_matcher() {
        return matcher.to_owned();
    }
    matcher
        .split('|')
        .map(|token| {
            let token = token.trim();
            if event.is_subagent_event() {
                adapter.matcher_subagent_type(token)
            } else {
                token
                    .parse::<ToolFrontmatter>()
                    .ok()
                    .and_then(|t| adapter.matcher_tool_name(&t))
                    .unwrap_or(token)
            }
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// Strip a leading `./` and the `scripts/` component from a spec's
/// `script` path, yielding the filename `hook_command_anchor` appends
/// under `<anchor>/hooks/scripts/`.
pub(super) fn script_filename(script: &Path) -> String {
    script
        .components()
        .skip_while(|c| matches!(c, Component::CurDir))
        .skip(1)
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned()
}

/// Render `arg` as a single POSIX `sh` word. Wrapping in single quotes
/// suppresses every expansion; the only byte needing further treatment
/// is the closing quote itself, emitted as `'\''` — close, escaped
/// literal, reopen.
fn sh_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// Build canonical `EmittedHookEntry` rows from hook specs.
///
/// Each entry's `command` field is computed by [`hook_command_anchor`],
/// which wraps the user script in a per-event shim invocation. The shape
/// is `<shim> <user_script> <hook_id> [<arg>…]`, with both anchor-relative
/// paths under the same per-mode base (plugin-root env var for Bundled;
/// `$HOME/<dotdir>` for `MergedUser`; `${CLAUDE_PROJECT_DIR}/<dotdir>` for
/// `MergedProject`). See [`hook_command_anchor`] for the full per-mode
/// shape.
fn build_emitted_hook_entries(
    specs: &[&HookSpec],
    provider: Provider,
    dotdir: &str,
    plugin_root_env_var: &'static str,
    emit_mode: HookEmitMode,
) -> Vec<EmittedHookEntry> {
    let adapter = provider.adapter();
    specs
        .iter()
        .flat_map(|s| {
            let filename = script_filename(&s.frontmatter.script);
            let hook_id = s.frontmatter.id.clone();
            let args = s.frontmatter.args.as_deref().unwrap_or(&[]);
            s.frontmatter
                .events
                .iter()
                .map(move |&event| EmittedHookEntry {
                    event,
                    matcher: s
                        .frontmatter
                        .matcher
                        .as_deref()
                        .map(|m| translate_matcher(adapter, event, m)),
                    command: hook_command_anchor(
                        dotdir,
                        plugin_root_env_var,
                        emit_mode,
                        event,
                        &filename,
                        &hook_id,
                        args,
                    ),
                    timeout: s.frontmatter.timeout,
                    agentspec_id: s.frontmatter.id.clone(),
                })
        })
        .collect()
}

/// Compute the `command` string for a hook entry given the dotdir, the
/// per-provider plugin-root env var name, the emit mode, the canonical
/// event, the user script's filename, the hook id, and the entry's
/// `args`.
///
/// The emitted command invokes the per-event shim with the user script's
/// path, the hook id, and each argument, quoted:
/// `<shim> <user_script> <hook_id> [<arg>…]`, where `<shim>` resolves to
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
/// Only `args` are quoted via [`sh_quote`]. `env_assignment` and the
/// `$HOME` / `${<plugin_root_env_var>}` / `${CLAUDE_PROJECT_DIR}` anchors
/// are agentspec-authored shell syntax that must keep expanding.
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
    args: &[String],
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
    let mut command = format!("{env_assignment}{shim_path} {user_script_path} {hook_id}");
    for arg in args {
        command.push(' ');
        command.push_str(&sh_quote(arg));
    }
    command
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::*;
    use crate::adapters::{ClaudeAdapter, CursorAdapter};
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
                args: None,
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
            Provider::Claude,
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
    fn build_emitted_hook_entries_forwards_args_from_frontmatter() {
        // Exercises the field this phase adds through the actual call
        // site — `s.frontmatter.args.as_deref().unwrap_or(&[])` inside
        // `build_emitted_hook_entries` — rather than only through
        // `hook_command_anchor` called directly with a hand-built slice.
        let mut spec = hook_spec("audit", vec![HookEvent::PreToolUse]);
        spec.frontmatter.args = Some(vec!["--strict".to_string(), "two words".to_string()]);
        let specs: Vec<&HookSpec> = vec![&spec];
        let entries = build_emitted_hook_entries(
            &specs,
            Provider::Claude,
            ".claude",
            "CLAUDE_PLUGIN_ROOT",
            HookEmitMode::Bundled,
        );
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0].command.ends_with("audit '--strict' 'two words'"),
            "expected quoted args appended after the hook id, got: {}",
            entries[0].command
        );
    }

    #[test]
    fn translate_matcher_splits_and_translates() {
        let result = translate_matcher(&ClaudeAdapter, HookEvent::PreToolUse, "shell|read");
        assert_eq!(result, "Bash|Read");
    }

    #[test]
    fn translate_matcher_passes_through_non_canonical() {
        let result =
            translate_matcher(&ClaudeAdapter, HookEvent::PreToolUse, "mcp__memory__create");
        assert_eq!(result, "mcp__memory__create");
    }

    #[test]
    fn translate_matcher_mixed_canonical_and_non_canonical() {
        let result = translate_matcher(
            &CursorAdapter,
            HookEvent::PreToolUse,
            "shell|mcp__memory__create",
        );
        assert_eq!(result, "Shell|mcp__memory__create");
    }

    #[test]
    fn translate_matcher_unavailable_tool_passes_through() {
        let result = translate_matcher(&CursorAdapter, HookEvent::PreToolUse, "shell|question");
        assert_eq!(result, "Shell|question");
    }

    #[test]
    fn translate_matcher_subagent_event() {
        let result = translate_matcher(&ClaudeAdapter, HookEvent::SubagentStart, "general|explore");
        assert_eq!(result, "general-purpose|Explore");
    }

    #[test]
    fn translate_matcher_subagent_pass_through() {
        let result = translate_matcher(&CursorAdapter, HookEvent::SubagentStart, "custom-agent");
        assert_eq!(result, "custom-agent");
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
            &[],
        );
        assert_eq!(
            cmd,
            "${CLAUDE_PLUGIN_ROOT}/hooks/scripts/_wrappers/pre_tool_use.sh ${CLAUDE_PLUGIN_ROOT}/hooks/scripts/audit.sh audit-bash"
        );
    }

    #[test]
    fn hook_command_anchor_no_args_merged_user_unchanged() {
        let cmd = hook_command_anchor(
            ".claude",
            "CLAUDE_PLUGIN_ROOT",
            HookEmitMode::MergedUser,
            HookEvent::PreToolUse,
            "audit.sh",
            "audit-bash",
            &[],
        );
        assert_eq!(
            cmd,
            "CLAUDE_PLUGIN_ROOT=$HOME/.claude $HOME/.claude/hooks/scripts/_wrappers/pre_tool_use.sh $HOME/.claude/hooks/scripts/audit.sh audit-bash"
        );
    }

    #[test]
    fn hook_command_anchor_no_args_merged_project_unchanged() {
        let cmd = hook_command_anchor(
            ".claude",
            "CLAUDE_PLUGIN_ROOT",
            HookEmitMode::MergedProject,
            HookEvent::PreToolUse,
            "audit.sh",
            "audit-bash",
            &[],
        );
        assert_eq!(
            cmd,
            "CLAUDE_PLUGIN_ROOT=${CLAUDE_PROJECT_DIR}/.claude ${CLAUDE_PROJECT_DIR}/.claude/hooks/scripts/_wrappers/pre_tool_use.sh ${CLAUDE_PROJECT_DIR}/.claude/hooks/scripts/audit.sh audit-bash"
        );
    }

    #[test]
    fn hook_command_anchor_empty_args_list_matches_no_args() {
        let no_args = hook_command_anchor(
            ".claude",
            "CLAUDE_PLUGIN_ROOT",
            HookEmitMode::Bundled,
            HookEvent::PreToolUse,
            "audit.sh",
            "audit-bash",
            &[],
        );
        let empty_args: Vec<String> = Vec::new();
        let with_empty_list = hook_command_anchor(
            ".claude",
            "CLAUDE_PLUGIN_ROOT",
            HookEmitMode::Bundled,
            HookEvent::PreToolUse,
            "audit.sh",
            "audit-bash",
            &empty_args,
        );
        assert_eq!(no_args, with_empty_list);
        assert!(
            !no_args.ends_with(' '),
            "no trailing space when there are no args: {no_args:?}"
        );
    }

    #[test]
    fn hook_command_anchor_empty_string_arg_emits_quoted_empty_word() {
        // Distinguishes `args = []` (previous test) from `args = [""]`: an
        // empty-list entry and a one-empty-string-argument entry look
        // similar on the page but must compose differently — the latter
        // raises the script's `$#` by one.
        let args = vec![String::new()];
        let cmd = hook_command_anchor(
            ".claude",
            "CLAUDE_PLUGIN_ROOT",
            HookEmitMode::Bundled,
            HookEvent::PreToolUse,
            "audit.sh",
            "audit-bash",
            &args,
        );
        assert!(
            cmd.ends_with("audit-bash ''"),
            "expected a trailing quoted-empty word, got: {cmd}"
        );
    }

    #[test]
    fn hook_command_anchor_two_entries_same_script_differ_only_in_args() {
        // Models the feature's motivating case: two `hooks.toml` entries
        // naming the same script. Holding every other input fixed isolates
        // `args` as the one axis of variation the composed command carries.
        let cmd_a = hook_command_anchor(
            ".claude",
            "CLAUDE_PLUGIN_ROOT",
            HookEmitMode::Bundled,
            HookEvent::PreToolUse,
            "shared.sh",
            "shared-hook",
            &["one".to_string()],
        );
        let cmd_b = hook_command_anchor(
            ".claude",
            "CLAUDE_PLUGIN_ROOT",
            HookEmitMode::Bundled,
            HookEvent::PreToolUse,
            "shared.sh",
            "shared-hook",
            &["two".to_string()],
        );
        assert_ne!(cmd_a, cmd_b);
        assert_eq!(
            cmd_a.strip_suffix("'one'").expect("cmd_a ends with 'one'"),
            cmd_b.strip_suffix("'two'").expect("cmd_b ends with 'two'"),
            "everything but the trailing argument should be identical"
        );
    }

    #[test]
    fn sh_quote_wraps_plain_value() {
        assert_eq!(sh_quote("plain"), "'plain'");
    }

    #[test]
    fn sh_quote_escapes_embedded_single_quote() {
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn sh_quote_suppresses_command_substitution() {
        assert_eq!(sh_quote("$(echo pwned)"), "'$(echo pwned)'");
    }

    #[test]
    fn sh_quote_preserves_utf8() {
        assert_eq!(sh_quote("café-日本-ñ"), "'café-日本-ñ'");
    }

    #[test]
    fn hook_command_anchor_round_trips_through_sh_c() {
        // The only test where a real shell parses agentspec's own
        // emission. Rather than exercising the full canonical-translation
        // shim (which needs `jq`), stand a marker script in at the shim's
        // path so `sh -c` runs *something* real at that position — proving
        // `sh_quote`'s composition survives an actual shell parse, not
        // just Rust-side string equality.
        let dir = tempfile::tempdir().expect("tempdir");
        let wrapper_dir = dir.path().join("hooks").join("scripts").join("_wrappers");
        std::fs::create_dir_all(&wrapper_dir).expect("mkdir wrappers");
        let marker = dir.path().join("argv.txt");
        let wrapper = wrapper_dir.join("pre_tool_use.sh");
        std::fs::write(
            &wrapper,
            format!(
                "#!/usr/bin/env sh\nfor a in \"$@\"; do printf '%s\\0' \"$a\"; done > '{}'\n",
                marker.display()
            ),
        )
        .expect("write wrapper");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&wrapper).expect("metadata").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&wrapper, perms).expect("chmod wrapper");
        }

        let args = vec![
            "has space".to_string(),
            "it's".to_string(),
            "\"quoted\"".to_string(),
            "$(echo pwned)".to_string(),
            ";".to_string(),
            "|".to_string(),
            String::new(),
            "café-日本-ñ".to_string(),
        ];
        let cmd = hook_command_anchor(
            ".claude",
            "CLAUDE_PLUGIN_ROOT",
            HookEmitMode::Bundled,
            HookEvent::PreToolUse,
            "audit.sh",
            "audit-bash",
            &args,
        );

        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .env("CLAUDE_PLUGIN_ROOT", dir.path())
            .status()
            .expect("run composed command");
        assert!(status.success(), "composed command exited non-zero: {cmd}");

        let raw = std::fs::read(&marker).expect("read marker");
        let mut observed: Vec<&[u8]> = raw.split(|&b| b == 0).collect();
        if observed.last() == Some(&&b""[..]) {
            observed.pop();
        }
        // The wrapper's own `$@` starts at the user script path (its `$0`
        // is the wrapper itself, uncounted) — index 0 is the user script
        // path, index 1 is the hook id, and the forwarded args start at 2.
        let observed_args: Vec<String> = observed[2..]
            .iter()
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        assert_eq!(observed_args, args);
    }
}
