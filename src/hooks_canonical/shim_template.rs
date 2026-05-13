//! POSIX shell shim code generator.
//!
//! Generates a self-contained shell script per `(ProviderName, HookEvent)`
//! pair that wraps a user's hook script with bidirectional canonical-payload
//! translation. The generated shim has no Rust-binary dependency on the
//! user's machine — it requires only `sh` and `jq` at runtime.
//!
//! ## Shim ABI (public contract between agentspec versions)
//!
//! - stdin: provider-shaped hook payload, fed through the input-translation
//!   `jq` program inlined at codegen time to produce canonical JSON.
//! - argv\[1\]: path to the user's hook script (must exist and be
//!   executable).
//! - stdout: provider-shaped hook output, synthesized from the user
//!   script's canonical stdout via the output-translation `jq` program
//!   inlined at codegen time. Empty when the user script emits nothing.
//! - exit code: passthrough of the user script's exit code; never altered
//!   by the shim itself except for the two pre-flight errors below.
//! - stderr: passthrough of the user script's stderr; pre-flight errors
//!   from the shim emit `agentspec:`-prefixed messages.
//!
//! ## Pre-flight error exits
//!
//! - `jq` missing on `PATH` → exit 1 with a `brew install jq` hint.
//! - `$1` empty or not executable → exit 1 with the supplied path.
//!
//! ## Translation-failure exits
//!
//! - Provider stdin not valid JSON (input-`jq` exits non-zero) → exit 1
//!   with an `agentspec:`-prefixed stderr line, before the user script is
//!   run. Surfaces malformed provider payloads loudly rather than letting
//!   them propagate as a silently-corrupt canonical payload.
//! - User script stdout not valid canonical JSON (output-`jq` exits
//!   non-zero) → exit 1 with an `agentspec:`-prefixed stderr line,
//!   overriding the user script's own exit code. Surfaces user-script
//!   output-format bugs loudly rather than letting them appear as "hook
//!   silently emitted nothing" to the provider runtime.

use std::borrow::Cow;

use crate::hooks_canonical::{ProviderName, SCHEMA_VERSION};
use crate::spec::HookEvent;

/// Render a complete POSIX shell shim for the given (`provider`, `event`)
/// pair.
///
/// The returned string is a complete shell script suitable for writing to
/// disk with executable permissions. It is self-contained: aside from `sh`
/// and `jq` on `PATH`, and a user-supplied hook script at `$1`, the shim
/// has no runtime dependencies.
///
/// Codegen is deterministic — calling this with the same arguments always
/// produces byte-identical output, which is what makes snapshot tests
/// meaningful as a drift detector.
pub fn shim_script(provider: ProviderName, event: HookEvent) -> String {
    let provider_wire = provider.wire_name();
    let event_snake = event.snake_case();
    let input_jq = input_jq_program(provider, event);
    let output_jq = output_jq_program(provider, event);

    // String replacement (not `format!`) so the embedded `jq` programs need
    // no `{{` / `}}` escaping for their literal `{` / `}` braces.
    //
    // Order matters: insert the jq programs first (they themselves contain
    // `__SCHEMA_VERSION__` placeholders), then substitute the literal
    // tokens last so the placeholder substitution reaches into the
    // freshly-inserted jq program text. `String::replace` is single-pass
    // and does not recurse — if `__SCHEMA_VERSION__` were substituted
    // before the jq insertion, the placeholders inside the jq program
    // would survive unsubstituted.
    SHIM_TEMPLATE
        .replace("__INPUT_JQ__", input_jq)
        .replace("__OUTPUT_JQ__", &output_jq)
        .replace("__PROVIDER__", provider_wire)
        .replace("__EVENT_SNAKE__", event_snake)
        .replace("__SCHEMA_VERSION__", SCHEMA_VERSION)
}

const SHIM_TEMPLATE: &str = r#"#!/usr/bin/env sh
# agentspec-generated shim: __PROVIDER__ __EVENT_SNAKE__
# schema_version: __SCHEMA_VERSION__
# DO NOT EDIT — regenerate via `agentspec compile`.

if ! command -v jq >/dev/null 2>&1; then
    printf 'agentspec: jq is required for canonical hook translation but was not found on PATH. Install jq (e.g., `brew install jq`, `apt install jq`) and reload the hook host.\n' >&2
    exit 1
fi

if [ -z "$1" ]; then
    printf 'agentspec: hook script path missing (expected as first argument)\n' >&2
    exit 1
fi

if [ ! -x "$1" ]; then
    printf 'agentspec: hook script not found or not executable: %s\n' "$1" >&2
    exit 1
fi

CANONICAL=$(jq -c '__INPUT_JQ__')
JQ_INPUT_EXIT=$?
if [ "$JQ_INPUT_EXIT" -ne 0 ]; then
    printf 'agentspec: input translation failed (jq exited %s); provider stdin is not valid JSON or did not match the expected shape\n' "$JQ_INPUT_EXIT" >&2
    exit 1
fi

USER_OUTPUT=$(printf '%s' "$CANONICAL" | "$1")
USER_EXIT=$?

if [ -n "$USER_OUTPUT" ]; then
    printf '%s' "$USER_OUTPUT" | jq -c '__OUTPUT_JQ__'
    JQ_OUTPUT_EXIT=$?
    if [ "$JQ_OUTPUT_EXIT" -ne 0 ]; then
        printf 'agentspec: output translation failed (jq exited %s); user script emitted output that is not valid canonical JSON\n' "$JQ_OUTPUT_EXIT" >&2
        exit 1
    fi
fi

exit "$USER_EXIT"
"#;

fn input_jq_program(provider: ProviderName, event: HookEvent) -> &'static str {
    match (provider, event) {
        (ProviderName::Claude, HookEvent::PreToolUse) => CLAUDE_PRE_TOOL_USE_IN,
        (ProviderName::Claude, HookEvent::PostToolUse) => CLAUDE_POST_TOOL_USE_IN,
        (ProviderName::Claude, HookEvent::PostToolUseFailure) => CLAUDE_POST_TOOL_USE_FAILURE_IN,
        (ProviderName::Claude, HookEvent::SessionStart) => CLAUDE_ENVELOPE_IN_SESSION_START,
        (ProviderName::Claude, HookEvent::SessionEnd) => CLAUDE_ENVELOPE_IN_SESSION_END,
        (ProviderName::Claude, HookEvent::Stop) => CLAUDE_ENVELOPE_IN_STOP,
        (ProviderName::Claude, HookEvent::PreCompact) => CLAUDE_ENVELOPE_IN_PRE_COMPACT,
        (ProviderName::Claude, HookEvent::SubagentStart) => CLAUDE_ENVELOPE_IN_SUBAGENT_START,
        (ProviderName::Claude, HookEvent::SubagentStop) => CLAUDE_ENVELOPE_IN_SUBAGENT_STOP,
        (ProviderName::Claude, HookEvent::UserPromptSubmit) => CLAUDE_USER_PROMPT_SUBMIT_IN,
        (ProviderName::Cursor, HookEvent::PreToolUse) => CURSOR_PRE_TOOL_USE_IN,
        (ProviderName::Cursor, HookEvent::PostToolUse) => CURSOR_POST_TOOL_USE_IN,
        (ProviderName::Cursor, HookEvent::PostToolUseFailure) => CURSOR_POST_TOOL_USE_FAILURE_IN,
        (ProviderName::Cursor, HookEvent::SessionStart) => CURSOR_ENVELOPE_IN_SESSION_START,
        (ProviderName::Cursor, HookEvent::SessionEnd) => CURSOR_ENVELOPE_IN_SESSION_END,
        (ProviderName::Cursor, HookEvent::Stop) => CURSOR_ENVELOPE_IN_STOP,
        (ProviderName::Cursor, HookEvent::PreCompact) => CURSOR_ENVELOPE_IN_PRE_COMPACT,
        (ProviderName::Cursor, HookEvent::SubagentStart) => CURSOR_ENVELOPE_IN_SUBAGENT_START,
        (ProviderName::Cursor, HookEvent::SubagentStop) => CURSOR_ENVELOPE_IN_SUBAGENT_STOP,
        (ProviderName::Cursor, HookEvent::UserPromptSubmit) => CURSOR_USER_PROMPT_SUBMIT_IN,
    }
}

fn output_jq_program(provider: ProviderName, event: HookEvent) -> Cow<'static, str> {
    match provider {
        ProviderName::Claude => {
            Cow::Owned(CLAUDE_OUT.replace("__EVENT_PASCAL__", event.pascal_case()))
        }
        // Cursor's output program is event-uniform — no substitution
        // needed, so we can return the `'static` constant as-is and avoid
        // the allocation.
        ProviderName::Cursor => Cow::Borrowed(CURSOR_OUT),
    }
}

// ---------------------------------------------------------------------------
// Input-translation jq programs (per `(ProviderName, HookEvent)` pair).
//
// Each emits the canonical envelope (`schema_version`, `provider`, `event`,
// `session_id`, `agent_id`, `cwd`, `transcript_path`) plus the event-specific
// fields plus a verbatim `provider_raw`. Field set is in lock-step with
// `CanonicalInputBody` in `src/hooks_canonical.rs`.

const CLAUDE_PRE_TOOL_USE_IN: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "claude",
  event: "pre_tool_use",
  session_id: .session_id,
  agent_id: .agent_id,
  cwd: .cwd,
  transcript_path: .transcript_path,
  tool_name: .tool_name,
  tool_use_id: (.tool_use_id // ""),
  tool_input: .tool_input,
  provider_raw: .
}"#;

const CLAUDE_POST_TOOL_USE_IN: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "claude",
  event: "post_tool_use",
  session_id: .session_id,
  agent_id: .agent_id,
  cwd: .cwd,
  transcript_path: .transcript_path,
  tool_name: .tool_name,
  tool_use_id: (.tool_use_id // ""),
  tool_input: .tool_input,
  tool_response: .tool_response,
  provider_raw: .
}"#;

const CLAUDE_POST_TOOL_USE_FAILURE_IN: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "claude",
  event: "post_tool_use_failure",
  session_id: .session_id,
  agent_id: .agent_id,
  cwd: .cwd,
  transcript_path: .transcript_path,
  tool_name: .tool_name,
  tool_use_id: (.tool_use_id // ""),
  tool_input: .tool_input,
  tool_response: .tool_response,
  provider_raw: .
}"#;

// Envelope-only Claude events. Differ only in the `event` literal — keep
// distinct constants so the snapshot diff is per-event when changes land.

const CLAUDE_ENVELOPE_IN_SESSION_START: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "claude",
  event: "session_start",
  session_id: .session_id,
  agent_id: .agent_id,
  cwd: .cwd,
  transcript_path: .transcript_path,
  provider_raw: .
}"#;

const CLAUDE_ENVELOPE_IN_SESSION_END: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "claude",
  event: "session_end",
  session_id: .session_id,
  agent_id: .agent_id,
  cwd: .cwd,
  transcript_path: .transcript_path,
  provider_raw: .
}"#;

const CLAUDE_ENVELOPE_IN_STOP: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "claude",
  event: "stop",
  session_id: .session_id,
  agent_id: .agent_id,
  cwd: .cwd,
  transcript_path: .transcript_path,
  provider_raw: .
}"#;

const CLAUDE_ENVELOPE_IN_PRE_COMPACT: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "claude",
  event: "pre_compact",
  session_id: .session_id,
  agent_id: .agent_id,
  cwd: .cwd,
  transcript_path: .transcript_path,
  provider_raw: .
}"#;

const CLAUDE_ENVELOPE_IN_SUBAGENT_START: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "claude",
  event: "subagent_start",
  session_id: .session_id,
  agent_id: .agent_id,
  cwd: .cwd,
  transcript_path: .transcript_path,
  provider_raw: .
}"#;

const CLAUDE_ENVELOPE_IN_SUBAGENT_STOP: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "claude",
  event: "subagent_stop",
  session_id: .session_id,
  agent_id: .agent_id,
  cwd: .cwd,
  transcript_path: .transcript_path,
  provider_raw: .
}"#;

const CLAUDE_USER_PROMPT_SUBMIT_IN: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "claude",
  event: "user_prompt_submit",
  session_id: .session_id,
  agent_id: .agent_id,
  cwd: .cwd,
  transcript_path: .transcript_path,
  prompt: (.prompt // ""),
  provider_raw: .
}"#;

// Cursor input translation — `session_id` reconstructs from
// `parent_conversation_id` when present (subagent firings), with the child
// id surfaced as `agent_id`. `cwd` comes from `workspace_roots[0]`. See
// `from_cursor` in `src/hooks_canonical.rs` for the Rust ref impl that this
// must match semantically (key ordering may differ) on shared fixtures.

const CURSOR_PRE_TOOL_USE_IN: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "cursor",
  event: "pre_tool_use",
  session_id: (.parent_conversation_id // .conversation_id),
  agent_id: (if .parent_conversation_id then .conversation_id else null end),
  cwd: .workspace_roots[0],
  transcript_path: .transcript_path,
  tool_name: .tool_name,
  tool_use_id: (.tool_use_id // ""),
  tool_input: .tool_input,
  provider_raw: .
}"#;

const CURSOR_POST_TOOL_USE_IN: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "cursor",
  event: "post_tool_use",
  session_id: (.parent_conversation_id // .conversation_id),
  agent_id: (if .parent_conversation_id then .conversation_id else null end),
  cwd: .workspace_roots[0],
  transcript_path: .transcript_path,
  tool_name: .tool_name,
  tool_use_id: (.tool_use_id // ""),
  tool_input: .tool_input,
  tool_response: .tool_output,
  provider_raw: .
}"#;

const CURSOR_POST_TOOL_USE_FAILURE_IN: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "cursor",
  event: "post_tool_use_failure",
  session_id: (.parent_conversation_id // .conversation_id),
  agent_id: (if .parent_conversation_id then .conversation_id else null end),
  cwd: .workspace_roots[0],
  transcript_path: .transcript_path,
  tool_name: .tool_name,
  tool_use_id: (.tool_use_id // ""),
  tool_input: .tool_input,
  tool_response: .tool_output,
  provider_raw: .
}"#;

const CURSOR_ENVELOPE_IN_SESSION_START: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "cursor",
  event: "session_start",
  session_id: (.parent_conversation_id // .conversation_id),
  agent_id: (if .parent_conversation_id then .conversation_id else null end),
  cwd: .workspace_roots[0],
  transcript_path: .transcript_path,
  provider_raw: .
}"#;

const CURSOR_ENVELOPE_IN_SESSION_END: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "cursor",
  event: "session_end",
  session_id: (.parent_conversation_id // .conversation_id),
  agent_id: (if .parent_conversation_id then .conversation_id else null end),
  cwd: .workspace_roots[0],
  transcript_path: .transcript_path,
  provider_raw: .
}"#;

const CURSOR_ENVELOPE_IN_STOP: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "cursor",
  event: "stop",
  session_id: (.parent_conversation_id // .conversation_id),
  agent_id: (if .parent_conversation_id then .conversation_id else null end),
  cwd: .workspace_roots[0],
  transcript_path: .transcript_path,
  provider_raw: .
}"#;

const CURSOR_ENVELOPE_IN_PRE_COMPACT: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "cursor",
  event: "pre_compact",
  session_id: (.parent_conversation_id // .conversation_id),
  agent_id: (if .parent_conversation_id then .conversation_id else null end),
  cwd: .workspace_roots[0],
  transcript_path: .transcript_path,
  provider_raw: .
}"#;

const CURSOR_ENVELOPE_IN_SUBAGENT_START: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "cursor",
  event: "subagent_start",
  session_id: (.parent_conversation_id // .conversation_id),
  agent_id: (if .parent_conversation_id then .conversation_id else null end),
  cwd: .workspace_roots[0],
  transcript_path: .transcript_path,
  provider_raw: .
}"#;

const CURSOR_ENVELOPE_IN_SUBAGENT_STOP: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "cursor",
  event: "subagent_stop",
  session_id: (.parent_conversation_id // .conversation_id),
  agent_id: (if .parent_conversation_id then .conversation_id else null end),
  cwd: .workspace_roots[0],
  transcript_path: .transcript_path,
  provider_raw: .
}"#;

const CURSOR_USER_PROMPT_SUBMIT_IN: &str = r#"{
  schema_version: "__SCHEMA_VERSION__",
  provider: "cursor",
  event: "user_prompt_submit",
  session_id: (.parent_conversation_id // .conversation_id),
  agent_id: (if .parent_conversation_id then .conversation_id else null end),
  cwd: .workspace_roots[0],
  transcript_path: .transcript_path,
  prompt: (.prompt // ""),
  provider_raw: .
}"#;

// ---------------------------------------------------------------------------
// Output-translation jq programs.
//
// The output shape is event-uniform within a provider: Claude wraps every
// event in `hookSpecificOutput` with `hookEventName` set to the event's
// PascalCase form; Cursor flattens fields at the top level. Both filter
// null fields via `with_entries(select(.value != null))` so absent canonical
// fields don't pollute provider stdout.
//
// On Claude, `(.decision_reason // .user_facing_message)` implements the
// documented fallback: Claude has one reason slot
// (`permissionDecisionReason`), `decision_reason` is the primary, and
// `user_facing_message` falls back when it's absent. Matches
// `to_claude_stdout` in `src/hooks_canonical.rs`.

const CLAUDE_OUT: &str = r#"{
  hookSpecificOutput: ({
    hookEventName: "__EVENT_PASCAL__",
    permissionDecision: .permission_decision,
    permissionDecisionReason: (.decision_reason // .user_facing_message),
    additionalContext: .additional_context,
    updatedInput: .updated_input
  } | with_entries(select(.value != null)))
}"#;

const CURSOR_OUT: &str = r"{
  permission: .permission_decision,
  agent_message: .decision_reason,
  user_message: .user_facing_message,
  additional_context: .additional_context,
  updated_input: .updated_input
} | with_entries(select(.value != null))";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks_canonical::{CanonicalInput, CanonicalOutput, PermissionDecision};
    use serde_json::Value;
    use std::process::{Command, Stdio};

    // ------------------------------------------------------------------
    // Content spot-checks: cheap structural assertions that the generated
    // shim contains the expected literals. Full byte-level coverage lives
    // in the snapshot tests under `tests/shim_snapshot.rs`.

    #[test]
    fn shim_contains_jq_guard_and_passthrough() {
        let s = shim_script(ProviderName::Claude, HookEvent::PreToolUse);
        assert!(s.starts_with("#!/usr/bin/env sh"));
        assert!(s.contains("command -v jq"));
        assert!(s.contains(r#"exit "$USER_EXIT""#));
    }

    #[test]
    fn shim_banner_names_provider_and_event() {
        let s = shim_script(ProviderName::Claude, HookEvent::PreToolUse);
        assert!(s.contains("agentspec-generated shim: claude pre_tool_use"));
        assert!(s.contains("schema_version: 1.0.0"));
    }

    #[test]
    fn cursor_shim_provider_literal() {
        let s = shim_script(ProviderName::Cursor, HookEvent::PreToolUse);
        assert!(s.contains(r#"provider: "cursor""#));
        assert!(s.contains(r#"event: "pre_tool_use""#));
        // Regression guard for the per-provider env-var fix: Cursor shim
        // must not reference the Claude wire name in any literal.
        assert!(!s.contains(r#"provider: "claude""#));
    }

    #[test]
    fn claude_shim_provider_literal() {
        let s = shim_script(ProviderName::Claude, HookEvent::SessionStart);
        assert!(s.contains(r#"provider: "claude""#));
        assert!(s.contains(r#"event: "session_start""#));
    }

    #[test]
    fn output_jq_embeds_hook_event_name_pascal() {
        let s = shim_script(ProviderName::Claude, HookEvent::UserPromptSubmit);
        assert!(s.contains(r#"hookEventName: "UserPromptSubmit""#));
    }

    #[test]
    fn no_unresolved_placeholders_in_any_generated_shim() {
        // Regression guard: every codegen placeholder must be substituted
        // by the time `shim_script` returns. A previous version of the
        // chained `String::replace` ordering left `__SCHEMA_VERSION__`
        // unresolved inside the inserted jq programs because the literal
        // substitution ran before the jq-program insertion.
        //
        // The check scans for any `__WORD__` substring (uppercase letters
        // or underscores between double-underscores) rather than a
        // hard-coded list, so future placeholder additions are caught
        // automatically. No `__`-prefixed identifiers appear legitimately
        // in the generated shim today.
        let events = [
            HookEvent::PreToolUse,
            HookEvent::PostToolUse,
            HookEvent::PostToolUseFailure,
            HookEvent::SessionStart,
            HookEvent::SessionEnd,
            HookEvent::Stop,
            HookEvent::PreCompact,
            HookEvent::SubagentStart,
            HookEvent::SubagentStop,
            HookEvent::UserPromptSubmit,
        ];
        for provider in [ProviderName::Claude, ProviderName::Cursor] {
            for event in events {
                let s = shim_script(provider, event);
                if let Some(placeholder) = find_placeholder(&s) {
                    panic!("unresolved placeholder `{placeholder}` in {provider:?}/{event:?} shim");
                }
            }
        }
    }

    /// Find the first `__WORD__` substring in `s`, if any (WORD is one or
    /// more uppercase ASCII letters or underscores).
    fn find_placeholder(s: &str) -> Option<&str> {
        let bytes = s.as_bytes();
        let mut i = 0;
        while i + 4 <= bytes.len() {
            if bytes[i] == b'_' && bytes[i + 1] == b'_' {
                let body_start = i + 2;
                let mut j = body_start;
                while j < bytes.len() && (bytes[j].is_ascii_uppercase() || bytes[j] == b'_') {
                    j += 1;
                }
                if j > body_start && j + 1 < bytes.len() && bytes[j] == b'_' && bytes[j + 1] == b'_'
                {
                    return Some(&s[i..j + 2]);
                }
            }
            i += 1;
        }
        None
    }

    #[test]
    fn deterministic_codegen() {
        // Two calls with the same inputs must produce byte-identical
        // output — this is what makes snapshot tests meaningful as a
        // drift detector.
        let a = shim_script(ProviderName::Claude, HookEvent::PreToolUse);
        let b = shim_script(ProviderName::Claude, HookEvent::PreToolUse);
        assert_eq!(a, b);
    }

    // ------------------------------------------------------------------
    // End-to-end exec tests: write the generated shim to a temp dir, mark
    // executable, exec via `sh` with synthesized provider-shaped stdin and
    // a no-op or assertion-bearing user script. Each test exercises a
    // different leg of the shim's runtime contract.
    //
    // These require `jq` on the test host. If `jq` is missing the tests
    // gracefully skip; CI hosts must have `jq` installed.

    fn jq_available() -> bool {
        match Command::new("jq")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(status) if status.success() => true,
            Ok(status) => {
                // jq is on PATH but `--version` returned non-zero — broken
                // install. Warn so the silent skip doesn't hide a real
                // problem on the test host.
                eprintln!(
                    "warning: `jq --version` exited with status {} — jq appears broken; e2e shim tests will skip",
                    status.code().unwrap_or(-1),
                );
                false
            }
            Err(_) => false,
        }
    }

    fn write_shim(
        dir: &std::path::Path,
        provider: ProviderName,
        event: HookEvent,
    ) -> std::path::PathBuf {
        let path = dir.join(format!(
            "shim_{}_{}.sh",
            provider.wire_name(),
            event.snake_case()
        ));
        let body = shim_script(provider, event);
        std::fs::write(&path, body).expect("write shim");
        let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o755);
        }
        std::fs::set_permissions(&path, perms).expect("chmod shim");
        path
    }

    fn write_user_script(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write user script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).expect("chmod user script");
        }
        path
    }

    fn exec_shim_with_stdin(
        shim: &std::path::Path,
        user_script: &std::path::Path,
        stdin: &str,
    ) -> (i32, String, String) {
        let mut child = Command::new("sh")
            .arg(shim)
            .arg(user_script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn shim");
        {
            use std::io::Write;
            let mut child_stdin = child.stdin.take().expect("child stdin");
            child_stdin
                .write_all(stdin.as_bytes())
                .expect("write stdin");
        }
        let out = child.wait_with_output().expect("wait shim");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    #[test]
    fn e2e_claude_pre_tool_use_user_script_sees_canonical_stdin() {
        if !jq_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = write_shim(dir.path(), ProviderName::Claude, HookEvent::PreToolUse);
        // User script asserts canonical fields and exits 0 on match, 7 on mismatch.
        let user = write_user_script(
            dir.path(),
            "user.sh",
            r#"#!/usr/bin/env sh
jq -e '.session_id == "sess-abc" and .tool_name == "Bash" and .event == "pre_tool_use"' > /dev/null || exit 7
exit 0
"#,
        );
        let provider_stdin = r#"{"session_id":"sess-abc","cwd":"/p","tool_name":"Bash","tool_use_id":"t1","tool_input":{"command":"ls"}}"#;
        let (code, _stdout, stderr) = exec_shim_with_stdin(&shim, &user, provider_stdin);
        assert_eq!(code, 0, "exit code; stderr={stderr}");
    }

    #[test]
    fn e2e_claude_pre_tool_use_deny_translates_to_hook_specific_output() {
        if !jq_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = write_shim(dir.path(), ProviderName::Claude, HookEvent::PreToolUse);
        let user = write_user_script(
            dir.path(),
            "user.sh",
            r#"#!/usr/bin/env sh
cat > /dev/null
printf '%s' '{"permission_decision":"deny","decision_reason":"blocked"}'
"#,
        );
        let provider_stdin = r#"{"session_id":"sess-abc","cwd":"/p","tool_name":"Bash","tool_input":{"command":"ls"}}"#;
        let (code, stdout, stderr) = exec_shim_with_stdin(&shim, &user, provider_stdin);
        assert_eq!(code, 0, "exit code; stderr={stderr}");
        let parsed: Value = serde_json::from_str(stdout.trim()).expect("parse output");
        assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            parsed["hookSpecificOutput"]["permissionDecisionReason"],
            "blocked"
        );
    }

    #[test]
    fn e2e_cursor_pre_tool_use_deny_translates_to_flat_output() {
        if !jq_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = write_shim(dir.path(), ProviderName::Cursor, HookEvent::PreToolUse);
        let user = write_user_script(
            dir.path(),
            "user.sh",
            r#"#!/usr/bin/env sh
cat > /dev/null
printf '%s' '{"permission_decision":"deny","decision_reason":"blocked"}'
"#,
        );
        let provider_stdin = r#"{"conversation_id":"conv-1","workspace_roots":["/p"],"tool_name":"shell","tool_input":{"command":"ls"}}"#;
        let (code, stdout, stderr) = exec_shim_with_stdin(&shim, &user, provider_stdin);
        assert_eq!(code, 0, "exit code; stderr={stderr}");
        let parsed: Value = serde_json::from_str(stdout.trim()).expect("parse output");
        assert_eq!(parsed["permission"], "deny");
        assert_eq!(parsed["agent_message"], "blocked");
    }

    #[test]
    fn e2e_exit_code_propagates() {
        if !jq_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = write_shim(dir.path(), ProviderName::Claude, HookEvent::PreToolUse);
        let user = write_user_script(
            dir.path(),
            "user.sh",
            "#!/usr/bin/env sh\ncat > /dev/null\nexit 2\n",
        );
        let provider_stdin = r#"{"session_id":"s","cwd":"/p","tool_name":"Bash","tool_input":{}}"#;
        let (code, _stdout, _stderr) = exec_shim_with_stdin(&shim, &user, provider_stdin);
        assert_eq!(code, 2);
    }

    #[test]
    fn e2e_missing_user_script_exits_one() {
        if !jq_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = write_shim(dir.path(), ProviderName::Claude, HookEvent::PreToolUse);
        let nonexistent = dir.path().join("does-not-exist.sh");
        let provider_stdin = r#"{"session_id":"s","cwd":"/p","tool_name":"Bash","tool_input":{}}"#;
        let (code, _stdout, stderr) = exec_shim_with_stdin(&shim, &nonexistent, provider_stdin);
        assert_eq!(code, 1);
        assert!(stderr.contains("hook script not found"), "stderr={stderr}");
    }

    #[test]
    fn e2e_missing_jq_exits_one() {
        if !jq_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = write_shim(dir.path(), ProviderName::Claude, HookEvent::PreToolUse);
        let user = write_user_script(dir.path(), "user.sh", "#!/usr/bin/env sh\nexit 0\n");
        // Empty subdirectory used as a jq-less PATH. Setting `PATH=""` is
        // unreliable because POSIX shells substitute a default when PATH
        // is empty or unset; pointing at a real-but-empty directory is the
        // portable way to defeat the `command -v jq` lookup.
        let empty_path_dir = dir.path().join("empty_path");
        std::fs::create_dir(&empty_path_dir).expect("mkdir empty_path");
        let mut child = Command::new("/bin/sh")
            .arg(&shim)
            .arg(&user)
            .env("PATH", &empty_path_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn shim");
        {
            use std::io::Write;
            let mut stdin = child.stdin.take().expect("child stdin");
            stdin.write_all(b"{}").expect("write stdin");
        }
        let out = child.wait_with_output().expect("wait shim");
        assert_eq!(out.status.code().unwrap_or(-1), 1);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("jq is required"), "stderr={stderr}");
    }

    #[test]
    fn e2e_malformed_provider_stdin_exits_one_with_translation_error() {
        if !jq_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = write_shim(dir.path(), ProviderName::Claude, HookEvent::PreToolUse);
        let user = write_user_script(dir.path(), "user.sh", "#!/usr/bin/env sh\nexit 0\n");
        // Provider stdin is not valid JSON — input-jq exits non-zero, the
        // shim surfaces the error and exits 1 before running the user
        // script. Asserts the documented translation-failure exit path.
        let (code, stdout, stderr) = exec_shim_with_stdin(&shim, &user, "this is not json");
        assert_eq!(code, 1, "stdout={stdout} stderr={stderr}");
        assert!(
            stderr.contains("input translation failed"),
            "stderr={stderr}"
        );
    }

    #[test]
    fn e2e_user_script_emits_invalid_json_exits_one() {
        if !jq_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = write_shim(dir.path(), ProviderName::Cursor, HookEvent::PreToolUse);
        // User script emits non-JSON on stdout; output-jq fails and shim
        // exits 1, overriding the user script's exit code.
        let user = write_user_script(
            dir.path(),
            "user.sh",
            "#!/usr/bin/env sh\ncat > /dev/null\nprintf '%s' 'this is not canonical json'\n",
        );
        let provider_stdin = r#"{"conversation_id":"c","workspace_roots":["/p"],"tool_name":"shell","tool_input":{}}"#;
        let (code, _stdout, stderr) = exec_shim_with_stdin(&shim, &user, provider_stdin);
        assert_eq!(code, 1, "stderr={stderr}");
        assert!(
            stderr.contains("output translation failed"),
            "stderr={stderr}"
        );
    }

    #[test]
    fn e2e_large_stdin_payload() {
        if !jq_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = write_shim(dir.path(), ProviderName::Claude, HookEvent::PreToolUse);
        let user = write_user_script(
            dir.path(),
            "user.sh",
            "#!/usr/bin/env sh\ncat > /dev/null\nexit 0\n",
        );
        // ~1 MB of payload via a long string field — stays well within
        // POSIX shell variable size limits on macOS/Linux. The test
        // depends on `jq` actively draining its stdin from the parent
        // pipe; a future input-jq program that buffered all of stdin
        // before reading could deadlock the parent's blocking write here.
        let big = "x".repeat(1_000_000);
        let provider_stdin = format!(
            r#"{{"session_id":"s","cwd":"/p","tool_name":"Bash","tool_input":{{"data":"{big}"}}}}"#,
        );
        let (code, _stdout, stderr) = exec_shim_with_stdin(&shim, &user, &provider_stdin);
        assert_eq!(code, 0, "stderr={stderr}");
    }

    // ------------------------------------------------------------------
    // Rust-vs-jq parity tests: for each `(ProviderName, HookEvent)` pair,
    // run both the Rust ref impl in `src/hooks_canonical.rs` and the
    // generated shim's input-jq program over a shared fixture, and assert
    // the canonical JSON outputs are semantically equivalent (key order
    // ignored; `serde_json`'s default `BTreeMap` and jq's insertion-order
    // construction differ in byte form but produce identical `Value`).

    fn run_jq(program: &str, stdin: &str) -> Value {
        let mut child = Command::new("jq")
            .arg("-c")
            .arg(program)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn jq");
        {
            use std::io::Write;
            let mut s = child.stdin.take().expect("jq stdin");
            s.write_all(stdin.as_bytes()).expect("write");
        }
        let out = child.wait_with_output().expect("wait jq");
        assert!(
            out.status.success(),
            "jq failed: stderr={}, program={program}",
            String::from_utf8_lossy(&out.stderr),
        );
        serde_json::from_slice(&out.stdout).expect("parse jq output")
    }

    fn fixture_for(provider: ProviderName, event: HookEvent) -> &'static str {
        match (provider, event) {
            (ProviderName::Claude, HookEvent::PreToolUse) => {
                r#"{"session_id":"sess","agent_id":null,"cwd":"/p","transcript_path":"/t","tool_name":"Bash","tool_use_id":"t1","tool_input":{"command":"ls"}}"#
            }
            (ProviderName::Claude, HookEvent::PostToolUse) => {
                r#"{"session_id":"sess","cwd":"/p","tool_name":"Bash","tool_use_id":"t1","tool_input":{"command":"ls"},"tool_response":{"stdout":"hi"}}"#
            }
            (ProviderName::Claude, HookEvent::PostToolUseFailure) => {
                r#"{"session_id":"sess","cwd":"/p","tool_name":"Bash","tool_use_id":"t1","tool_input":{"command":"ls"},"tool_response":{"error":"boom"}}"#
            }
            (ProviderName::Claude, HookEvent::UserPromptSubmit) => {
                r#"{"session_id":"sess","cwd":"/p","prompt":"hello"}"#
            }
            (ProviderName::Claude, _) => r#"{"session_id":"sess","cwd":"/p"}"#,
            (ProviderName::Cursor, HookEvent::PreToolUse) => {
                r#"{"conversation_id":"conv","workspace_roots":["/p"],"tool_name":"shell","tool_use_id":"t1","tool_input":{"command":"ls"}}"#
            }
            (ProviderName::Cursor, HookEvent::PostToolUse) => {
                r#"{"conversation_id":"conv","workspace_roots":["/p"],"tool_name":"shell","tool_use_id":"t1","tool_input":{"command":"ls"},"tool_output":"{\"stdout\":\"hi\"}"}"#
            }
            (ProviderName::Cursor, HookEvent::PostToolUseFailure) => {
                r#"{"conversation_id":"conv","workspace_roots":["/p"],"tool_name":"shell","tool_use_id":"t1","tool_input":{"command":"ls"},"tool_output":"{\"error\":\"boom\"}"}"#
            }
            (ProviderName::Cursor, HookEvent::UserPromptSubmit) => {
                r#"{"conversation_id":"conv","workspace_roots":["/p"],"prompt":"hello"}"#
            }
            (ProviderName::Cursor, _) => r#"{"conversation_id":"conv","workspace_roots":["/p"]}"#,
        }
    }

    fn all_pairs() -> Vec<(ProviderName, HookEvent)> {
        let events = [
            HookEvent::PreToolUse,
            HookEvent::PostToolUse,
            HookEvent::PostToolUseFailure,
            HookEvent::SessionStart,
            HookEvent::SessionEnd,
            HookEvent::Stop,
            HookEvent::PreCompact,
            HookEvent::SubagentStart,
            HookEvent::SubagentStop,
            HookEvent::UserPromptSubmit,
        ];
        let mut v = Vec::with_capacity(20);
        for p in [ProviderName::Claude, ProviderName::Cursor] {
            for e in events {
                v.push((p, e));
            }
        }
        v
    }

    #[test]
    fn parity_input_rust_vs_jq() {
        if !jq_available() {
            return;
        }
        for (provider, event) in all_pairs() {
            let raw = fixture_for(provider, event);
            // Rust reference impl.
            let rust_canonical = CanonicalInput::from_provider_stdin(provider, raw, event)
                .unwrap_or_else(|e| panic!("Rust failed for {provider:?}/{event:?}: {e}"));
            let rust_value: Value =
                serde_json::to_value(&rust_canonical).expect("serialize Rust canonical");

            // Generated jq.
            let program =
                input_jq_program(provider, event).replace("__SCHEMA_VERSION__", SCHEMA_VERSION);
            let jq_value = run_jq(&program, raw);

            assert_eq!(
                rust_value, jq_value,
                "parity mismatch for {provider:?}/{event:?}:\n  rust={rust_value}\n  jq={jq_value}",
            );
        }
    }

    #[test]
    fn parity_output_rust_vs_jq() {
        if !jq_available() {
            return;
        }
        let canonical_outputs = [
            // Empty (envelope-only) — Cursor produces {}, Claude
            // produces {hookSpecificOutput: {hookEventName: "..."}}.
            CanonicalOutput {
                schema_version: SCHEMA_VERSION.to_string(),
                permission_decision: None,
                decision_reason: None,
                user_facing_message: None,
                additional_context: None,
                updated_input: None,
            },
            // Deny with reason.
            CanonicalOutput {
                schema_version: SCHEMA_VERSION.to_string(),
                permission_decision: Some(PermissionDecision::Deny),
                decision_reason: Some("blocked".into()),
                user_facing_message: None,
                additional_context: None,
                updated_input: None,
            },
            // Deny with both reasons + additional_context + updated_input.
            CanonicalOutput {
                schema_version: SCHEMA_VERSION.to_string(),
                permission_decision: Some(PermissionDecision::Deny),
                decision_reason: Some("for model".into()),
                user_facing_message: Some("for user".into()),
                additional_context: Some("ctx".into()),
                updated_input: Some(serde_json::json!({"command": "ls -al"})),
            },
        ];
        for (provider, event) in [
            (ProviderName::Claude, HookEvent::PreToolUse),
            (ProviderName::Cursor, HookEvent::PreToolUse),
            (ProviderName::Claude, HookEvent::UserPromptSubmit),
            (ProviderName::Cursor, HookEvent::UserPromptSubmit),
        ] {
            let program = output_jq_program(provider, event);
            for out in &canonical_outputs {
                let canonical_wire = serde_json::to_string(out).expect("serialize canonical out");
                let rust_text = out
                    .to_provider_stdout(provider, event)
                    .expect("Rust to_provider_stdout");
                let rust_value: Value = serde_json::from_str(&rust_text).expect("parse Rust out");
                let jq_value = run_jq(&program, &canonical_wire);
                assert_eq!(
                    rust_value, jq_value,
                    "output parity mismatch for {provider:?}/{event:?}:\n  rust={rust_value}\n  jq={jq_value}",
                );
            }
        }
    }
}
