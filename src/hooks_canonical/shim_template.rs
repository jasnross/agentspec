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
/// Every shim carries both Claude and Cursor jq dialects and selects the
/// correct pair at runtime via `cursor_version` field detection. The
/// `provider` argument determines only the banner comment (which
/// provider's plugin tree this shim was compiled into).
///
/// Codegen is deterministic — calling this with the same arguments always
/// produces byte-identical output, which is what makes snapshot tests
/// meaningful as a drift detector.
pub fn shim_script(provider: ProviderName, event: HookEvent) -> String {
    let plugin_provider = provider.wire_name();
    let event_snake = event.snake_case();
    let claude_in = claude_input_jq(event);
    let cursor_in = cursor_input_jq(event);
    let claude_out = claude_output_jq(event);
    let cursor_out = cursor_output_jq();
    let log_tag_init = r#"LOG_TAG="agentspec [$HOOK_ID]""#.to_string();

    // String replacement (not `format!`) so the embedded `jq` programs need
    // no `{{` / `}}` escaping for their literal `{` / `}` braces.
    //
    // Order matters: insert the jq programs first (they themselves contain
    // `__SCHEMA_VERSION__` placeholders), then substitute the literal
    // tokens last so the placeholder substitution reaches into the
    // freshly-inserted jq program text.
    SHIM_TEMPLATE
        .replace("__CLAUDE_INPUT_JQ__", claude_in)
        .replace("__CURSOR_INPUT_JQ__", cursor_in)
        .replace("__CLAUDE_OUTPUT_JQ__", &claude_out)
        .replace("__CURSOR_OUTPUT_JQ__", &cursor_out)
        .replace("__LOG_TAG_INIT__", &log_tag_init)
        .replace("__PLUGIN_PROVIDER__", plugin_provider)
        .replace("__EVENT_SNAKE__", event_snake)
        .replace("__SCHEMA_VERSION__", SCHEMA_VERSION)
}

const SHIM_TEMPLATE: &str = r#"#!/usr/bin/env sh
# agentspec-generated shim: __PLUGIN_PROVIDER__ __EVENT_SNAKE__
# schema_version: __SCHEMA_VERSION__
# DO NOT EDIT — regenerate via `agentspec compile`.

HOOK_ID="${2:-}"
__LOG_TAG_INIT__

_ALOG="${AGENTSPEC_HOOK_LOG:-}"
_alog() {
    if [ -n "$_ALOG" ]; then
        printf '[%s] %s: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$LOG_TAG" "$1" >> "$_ALOG"
    fi
}

if ! command -v jq >/dev/null 2>&1; then
    printf '%s: jq is required for canonical hook translation but was not found on PATH. Install jq (e.g., `brew install jq`, `apt install jq`) and reload the hook host.\n' "$LOG_TAG" >&2
    exit 1
fi

if [ -z "$1" ]; then
    printf '%s: hook script path missing (expected as first argument)\n' "$LOG_TAG" >&2
    exit 1
fi

if [ ! -x "$1" ]; then
    printf '%s: hook script not found or not executable: %s\n' "$LOG_TAG" "$1" >&2
    exit 1
fi

RAW=$(cat)
_alog "raw_input"
_alog "$RAW"

if printf '%s' "$RAW" | jq -e '.cursor_version' >/dev/null 2>&1; then
    _detected=cursor
    INPUT_JQ='__CURSOR_INPUT_JQ__'
    OUTPUT_JQ='__CURSOR_OUTPUT_JQ__'
else
    _detected=claude
    INPUT_JQ='__CLAUDE_INPUT_JQ__'
    OUTPUT_JQ='__CLAUDE_OUTPUT_JQ__'
fi
_alog "event=__EVENT_SNAKE__ provider=$_detected"

CANONICAL=$(printf '%s' "$RAW" | jq -c "$INPUT_JQ")
JQ_INPUT_EXIT=$?
if [ "$JQ_INPUT_EXIT" -ne 0 ]; then
    _alog "error: input translation failed"
    printf '%s: input translation failed (jq exited %s); provider stdin is not valid JSON or did not match the expected shape\n' "$LOG_TAG" "$JQ_INPUT_EXIT" >&2
    exit 1
fi
_alog "canonical_input"
_alog "$CANONICAL"

USER_OUTPUT=$(printf '%s' "$CANONICAL" | "$1")
USER_EXIT=$?
_alog "user_stdout"
_alog "$USER_OUTPUT"
_alog "user_exit=$USER_EXIT"

if [ -n "$USER_OUTPUT" ]; then
    PROVIDER_OUTPUT=$(printf '%s' "$USER_OUTPUT" | jq -c "$OUTPUT_JQ" 2>&1)
    JQ_OUTPUT_EXIT=$?
    if [ "$JQ_OUTPUT_EXIT" -ne 0 ]; then
        _alog "error: output translation failed: $PROVIDER_OUTPUT"
        printf '%s: output translation failed (jq exited %s): %s\n' "$LOG_TAG" "$JQ_OUTPUT_EXIT" "$PROVIDER_OUTPUT" >&2
        exit 1
    fi
    _alog "provider_output"
    _alog "$PROVIDER_OUTPUT"
    printf '%s\n' "$PROVIDER_OUTPUT"
fi

exit "$USER_EXIT"
"#;

fn claude_input_jq(event: HookEvent) -> &'static str {
    match event {
        HookEvent::PreToolUse => CLAUDE_PRE_TOOL_USE_IN,
        HookEvent::PostToolUse => CLAUDE_POST_TOOL_USE_IN,
        HookEvent::PostToolUseFailure => CLAUDE_POST_TOOL_USE_FAILURE_IN,
        HookEvent::SessionStart => CLAUDE_ENVELOPE_IN_SESSION_START,
        HookEvent::SessionEnd => CLAUDE_ENVELOPE_IN_SESSION_END,
        HookEvent::Stop => CLAUDE_ENVELOPE_IN_STOP,
        HookEvent::PreCompact => CLAUDE_ENVELOPE_IN_PRE_COMPACT,
        HookEvent::SubagentStart => CLAUDE_ENVELOPE_IN_SUBAGENT_START,
        HookEvent::SubagentStop => CLAUDE_ENVELOPE_IN_SUBAGENT_STOP,
        HookEvent::UserPromptSubmit => CLAUDE_USER_PROMPT_SUBMIT_IN,
    }
}

fn cursor_input_jq(event: HookEvent) -> &'static str {
    match event {
        HookEvent::PreToolUse => CURSOR_PRE_TOOL_USE_IN,
        HookEvent::PostToolUse => CURSOR_POST_TOOL_USE_IN,
        HookEvent::PostToolUseFailure => CURSOR_POST_TOOL_USE_FAILURE_IN,
        HookEvent::SessionStart => CURSOR_ENVELOPE_IN_SESSION_START,
        HookEvent::SessionEnd => CURSOR_ENVELOPE_IN_SESSION_END,
        HookEvent::Stop => CURSOR_ENVELOPE_IN_STOP,
        HookEvent::PreCompact => CURSOR_ENVELOPE_IN_PRE_COMPACT,
        HookEvent::SubagentStart => CURSOR_ENVELOPE_IN_SUBAGENT_START,
        HookEvent::SubagentStop => CURSOR_ENVELOPE_IN_SUBAGENT_STOP,
        HookEvent::UserPromptSubmit => CURSOR_USER_PROMPT_SUBMIT_IN,
    }
}

#[cfg(test)]
fn input_jq_program(provider: ProviderName, event: HookEvent) -> &'static str {
    match provider {
        ProviderName::Claude => claude_input_jq(event),
        ProviderName::Cursor => cursor_input_jq(event),
    }
}

const CANONICAL_OUTPUT_VALIDATE: &str = r#"(if type != "object" then error("expected JSON object, got " + type) else . end) | (([keys[] | select(. as $k | ["schema_version","permission_decision","decision_reason","user_facing_message","additional_context","updated_input"] | index($k) | not)]) as $u | if ($u | length) > 0 then error("unrecognized canonical output fields: " + ($u | join(", "))) else . end) | "#;

fn claude_output_jq(event: HookEvent) -> String {
    let transform = CLAUDE_OUT.replace("__EVENT_PASCAL__", event.pascal_case());
    format!("{CANONICAL_OUTPUT_VALIDATE}{transform}")
}

fn cursor_output_jq() -> String {
    format!("{CANONICAL_OUTPUT_VALIDATE}{CURSOR_OUT}")
}

#[cfg(test)]
fn output_jq_program(provider: ProviderName, event: HookEvent) -> String {
    match provider {
        ProviderName::Claude => claude_output_jq(event),
        ProviderName::Cursor => cursor_output_jq(),
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
    use std::process::{Command, Stdio};

    use serde_json::Value;

    use super::*;
    use crate::hooks_canonical::{CanonicalInput, CanonicalOutput, PermissionDecision};

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
        // Both providers' input jq programs are embedded for cross-host detection.
        assert!(s.contains(r#"provider: "cursor""#));
        assert!(s.contains(r#"provider: "claude""#));
        assert!(s.contains(r#"event: "pre_tool_use""#));
        // Banner identifies the plugin provider.
        assert!(s.contains("agentspec-generated shim: cursor pre_tool_use"));
    }

    #[test]
    fn claude_shim_provider_literal() {
        let s = shim_script(ProviderName::Claude, HookEvent::SessionStart);
        assert!(s.contains(r#"provider: "claude""#));
        assert!(s.contains(r#"provider: "cursor""#));
        assert!(s.contains(r#"event: "session_start""#));
        assert!(s.contains("agentspec-generated shim: claude session_start"));
    }

    #[test]
    fn output_jq_embeds_hook_event_name_pascal() {
        let s = shim_script(ProviderName::Claude, HookEvent::UserPromptSubmit);
        assert!(s.contains(r#"hookEventName: "UserPromptSubmit""#));
    }

    #[test]
    fn output_includes_log_tag() {
        let s = shim_script(ProviderName::Claude, HookEvent::PreToolUse);
        assert!(s.contains(r#"LOG_TAG="agentspec [$HOOK_ID]""#));
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
        exec_shim_with_env(shim, user_script, stdin, &[])
    }

    fn exec_shim_with_env(
        shim: &std::path::Path,
        user_script: &std::path::Path,
        stdin: &str,
        env: &[(&str, &str)],
    ) -> (i32, String, String) {
        let mut cmd = Command::new("sh");
        cmd.arg(shim)
            .arg(user_script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn shim");
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
        let provider_stdin = r#"{"cursor_version":"3.2","conversation_id":"conv-1","workspace_roots":["/p"],"tool_name":"shell","tool_input":{"command":"ls"}}"#;
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
        let provider_stdin = r#"{"cursor_version":"3.2","conversation_id":"c","workspace_roots":["/p"],"tool_name":"shell","tool_input":{}}"#;
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
        crate::hooks_canonical::provider_fixture(provider, event)
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

    #[test]
    fn e2e_provider_shaped_output_rejected() {
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
printf '%s' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny"}}'
"#,
        );
        let provider_stdin = r#"{"session_id":"s","cwd":"/p","tool_name":"Bash","tool_input":{}}"#;
        let (code, _stdout, stderr) = exec_shim_with_stdin(&shim, &user, provider_stdin);
        assert_eq!(code, 1, "stderr={stderr}");
        assert!(stderr.contains("unrecognized"), "stderr={stderr}");
        assert!(stderr.contains("hookSpecificOutput"), "stderr={stderr}");
    }

    #[test]
    fn e2e_typo_in_canonical_field_rejected() {
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
printf '%s' '{"permmission_decision":"deny"}'
"#,
        );
        let provider_stdin = r#"{"session_id":"s","cwd":"/p","tool_name":"Bash","tool_input":{}}"#;
        let (code, _stdout, stderr) = exec_shim_with_stdin(&shim, &user, provider_stdin);
        assert_eq!(code, 1, "stderr={stderr}");
        assert!(stderr.contains("unrecognized"), "stderr={stderr}");
        assert!(stderr.contains("permmission_decision"), "stderr={stderr}");
    }

    #[test]
    fn e2e_non_object_output_rejected() {
        if !jq_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = write_shim(dir.path(), ProviderName::Claude, HookEvent::PreToolUse);
        let user = write_user_script(
            dir.path(),
            "user.sh",
            "#!/usr/bin/env sh\ncat > /dev/null\nprintf '%s' '[\"deny\"]'\n",
        );
        let provider_stdin = r#"{"session_id":"s","cwd":"/p","tool_name":"Bash","tool_input":{}}"#;
        let (code, _stdout, stderr) = exec_shim_with_stdin(&shim, &user, provider_stdin);
        assert_eq!(code, 1, "stderr={stderr}");
        assert!(stderr.contains("expected JSON object"), "stderr={stderr}");
    }

    #[test]
    fn e2e_valid_canonical_output_passes_validation() {
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
        let provider_stdin = r#"{"cursor_version":"3.2","conversation_id":"c","workspace_roots":["/p"],"tool_name":"shell","tool_input":{}}"#;
        let (code, stdout, stderr) = exec_shim_with_stdin(&shim, &user, provider_stdin);
        assert_eq!(code, 0, "stderr={stderr}");
        let parsed: Value = serde_json::from_str(stdout.trim()).expect("parse output");
        assert_eq!(parsed["permission"], "deny");
        assert_eq!(parsed["agent_message"], "blocked");
    }

    #[test]
    fn e2e_empty_output_skips_validation() {
        if !jq_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = write_shim(dir.path(), ProviderName::Claude, HookEvent::PreToolUse);
        let user = write_user_script(
            dir.path(),
            "user.sh",
            "#!/usr/bin/env sh\ncat > /dev/null\n",
        );
        let provider_stdin = r#"{"session_id":"s","cwd":"/p","tool_name":"Bash","tool_input":{}}"#;
        let (code, stdout, stderr) = exec_shim_with_stdin(&shim, &user, provider_stdin);
        assert_eq!(code, 0, "stderr={stderr}");
        assert!(stdout.is_empty(), "stdout should be empty: {stdout}");
    }

    #[test]
    fn e2e_schema_version_accepted() {
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
printf '%s' '{"schema_version":"1.0.0","permission_decision":"deny"}'
"#,
        );
        let provider_stdin = r#"{"session_id":"s","cwd":"/p","tool_name":"Bash","tool_input":{}}"#;
        let (code, _stdout, stderr) = exec_shim_with_stdin(&shim, &user, provider_stdin);
        assert_eq!(code, 0, "stderr={stderr}");
    }

    #[test]
    fn parity_output_rejection_rust_vs_jq() {
        if !jq_available() {
            return;
        }
        let invalid_outputs = [
            (
                r#"{"hookSpecificOutput":{"permissionDecision":"deny"}}"#,
                "unknown fields",
            ),
            (r#"{"permmission_decision":"deny"}"#, "typo field"),
            (r#"["deny"]"#, "non-object"),
            (
                r#"{"permission_decision":"deny","extra_debug":"foo"}"#,
                "extra field",
            ),
        ];
        let program = output_jq_program(ProviderName::Claude, HookEvent::PreToolUse);
        for (input, label) in invalid_outputs {
            let rust_result = serde_json::from_str::<CanonicalOutput>(input);
            assert!(rust_result.is_err(), "Rust should reject {label}: {input}");

            let jq_result = Command::new("jq")
                .arg("-c")
                .arg(&program)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    use std::io::Write;
                    child
                        .stdin
                        .take()
                        .expect("jq stdin")
                        .write_all(input.as_bytes())
                        .expect("write");
                    child.wait_with_output()
                })
                .expect("run jq");
            assert!(
                !jq_result.status.success(),
                "jq should reject {label}: {input} (stderr={})",
                String::from_utf8_lossy(&jq_result.stderr),
            );
        }
    }

    #[test]
    fn e2e_claude_shim_receives_cursor_input_detects_host() {
        if !jq_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = write_shim(dir.path(), ProviderName::Claude, HookEvent::PreToolUse);
        let user = write_user_script(
            dir.path(),
            "user.sh",
            r#"#!/usr/bin/env sh
jq -e '.provider == "cursor" and .session_id == "conv-1" and .cwd == "/ws" and .tool_name == "shell"' > /dev/null || exit 7
"#,
        );
        let cursor_stdin = r#"{"cursor_version":"3.2","conversation_id":"conv-1","workspace_roots":["/ws"],"tool_name":"shell","tool_use_id":"t1","tool_input":{"command":"ls"}}"#;
        let (code, _stdout, stderr) = exec_shim_with_stdin(&shim, &user, cursor_stdin);
        assert_eq!(code, 0, "cross-host detection failed; stderr={stderr}");
    }

    #[test]
    fn e2e_claude_shim_receives_cursor_input_deny_outputs_flat() {
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
        let cursor_stdin = r#"{"cursor_version":"3.2","conversation_id":"conv-1","workspace_roots":["/ws"],"tool_name":"shell","tool_use_id":"t1","tool_input":{"command":"ls"}}"#;
        let (code, stdout, stderr) = exec_shim_with_stdin(&shim, &user, cursor_stdin);
        assert_eq!(code, 0, "stderr={stderr}");
        let parsed: Value = serde_json::from_str(stdout.trim()).expect("parse output");
        assert_eq!(parsed["permission"], "deny");
        assert_eq!(parsed["agent_message"], "blocked");
        assert!(
            parsed.get("hookSpecificOutput").is_none(),
            "cross-host output should be flat Cursor format, not nested Claude"
        );
    }

    #[test]
    fn e2e_cursor_shim_receives_claude_input_detects_host() {
        if !jq_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = write_shim(dir.path(), ProviderName::Cursor, HookEvent::PreToolUse);
        let user = write_user_script(
            dir.path(),
            "user.sh",
            r#"#!/usr/bin/env sh
jq -e '.provider == "claude" and .session_id == "sess-1" and .cwd == "/home/u"' > /dev/null || exit 7
"#,
        );
        let claude_stdin = r#"{"session_id":"sess-1","cwd":"/home/u","tool_name":"Bash","tool_use_id":"t1","tool_input":{"command":"ls"}}"#;
        let (code, _stdout, stderr) = exec_shim_with_stdin(&shim, &user, claude_stdin);
        assert_eq!(
            code, 0,
            "reverse cross-host detection failed; stderr={stderr}"
        );
    }

    #[test]
    fn e2e_cursor_shim_receives_claude_input_deny_outputs_nested() {
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
        let claude_stdin = r#"{"session_id":"sess-1","cwd":"/home/u","tool_name":"Bash","tool_use_id":"t1","tool_input":{"command":"ls"}}"#;
        let (code, stdout, stderr) = exec_shim_with_stdin(&shim, &user, claude_stdin);
        assert_eq!(code, 0, "stderr={stderr}");
        let parsed: Value = serde_json::from_str(stdout.trim()).expect("parse output");
        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            parsed["hookSpecificOutput"]["permissionDecisionReason"],
            "blocked"
        );
        assert!(
            parsed.get("permission").is_none(),
            "cross-host output should be nested Claude format, not flat Cursor"
        );
    }

    #[test]
    fn e2e_native_host_unchanged_after_refactor() {
        if !jq_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");

        // Claude native: verify canonical input fields and nested output
        let shim = write_shim(dir.path(), ProviderName::Claude, HookEvent::PreToolUse);
        let user = write_user_script(
            dir.path(),
            "user_claude.sh",
            r#"#!/usr/bin/env sh
jq -e '.provider == "claude" and .session_id == "sess" and .cwd == "/p"' > /dev/null || exit 7
printf '%s' '{"permission_decision":"deny","decision_reason":"test"}'
"#,
        );
        let (code, stdout, stderr) = exec_shim_with_stdin(
            &shim,
            &user,
            r#"{"session_id":"sess","cwd":"/p","tool_name":"Bash","tool_use_id":"t1","tool_input":{"command":"ls"}}"#,
        );
        assert_eq!(code, 0, "Claude native; stderr={stderr}");
        let parsed: Value = serde_json::from_str(stdout.trim()).expect("parse Claude output");
        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "deny");

        // Cursor native: verify canonical input fields and flat output
        let shim = write_shim(dir.path(), ProviderName::Cursor, HookEvent::PreToolUse);
        let user = write_user_script(
            dir.path(),
            "user_cursor.sh",
            r#"#!/usr/bin/env sh
jq -e '.provider == "cursor" and .session_id == "conv" and .cwd == "/p"' > /dev/null || exit 7
printf '%s' '{"permission_decision":"deny","decision_reason":"test"}'
"#,
        );
        let (code, stdout, stderr) = exec_shim_with_stdin(
            &shim,
            &user,
            r#"{"cursor_version":"3.2","conversation_id":"conv","workspace_roots":["/p"],"tool_name":"shell","tool_use_id":"t1","tool_input":{"command":"ls"}}"#,
        );
        assert_eq!(code, 0, "Cursor native; stderr={stderr}");
        let parsed: Value = serde_json::from_str(stdout.trim()).expect("parse Cursor output");
        assert_eq!(parsed["permission"], "deny");
        assert_eq!(parsed["agent_message"], "test");
    }

    #[test]
    fn parity_cross_host_input_rust_vs_jq() {
        if !jq_available() {
            return;
        }
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
        for event in events {
            for fixture_provider in [ProviderName::Claude, ProviderName::Cursor] {
                let raw = fixture_for(fixture_provider, event);
                let rust_canonical = CanonicalInput::from_provider_stdin_auto(raw, event)
                    .unwrap_or_else(|e| {
                        panic!("Rust auto-detect failed for {fixture_provider:?}/{event:?}: {e}")
                    });
                let rust_value: Value =
                    serde_json::to_value(&rust_canonical).expect("serialize Rust canonical");

                let detected = CanonicalInput::detect_provider(raw)
                    .unwrap_or_else(|e| panic!("detect_provider failed: {e}"));
                let jq_program =
                    input_jq_program(detected, event).replace("__SCHEMA_VERSION__", SCHEMA_VERSION);
                let jq_value = run_jq(&jq_program, raw);

                assert_eq!(
                    rust_value, jq_value,
                    "cross-host input parity mismatch: fixture={fixture_provider:?}, event={event:?}, detected={detected:?}",
                );
            }
        }
    }

    #[test]
    fn parity_cross_host_output_rust_vs_jq() {
        if !jq_available() {
            return;
        }
        let canonical_outputs = [
            CanonicalOutput {
                schema_version: SCHEMA_VERSION.to_string(),
                permission_decision: Some(PermissionDecision::Deny),
                decision_reason: Some("blocked".into()),
                user_facing_message: None,
                additional_context: None,
                updated_input: None,
            },
            CanonicalOutput {
                schema_version: SCHEMA_VERSION.to_string(),
                permission_decision: Some(PermissionDecision::Deny),
                decision_reason: Some("for model".into()),
                user_facing_message: Some("for user".into()),
                additional_context: Some("ctx".into()),
                updated_input: Some(serde_json::json!({"command": "ls -al"})),
            },
        ];
        let events = [
            HookEvent::PreToolUse,
            HookEvent::PostToolUse,
            HookEvent::SessionStart,
            HookEvent::UserPromptSubmit,
        ];
        for event in events {
            for detected in [ProviderName::Claude, ProviderName::Cursor] {
                let program = output_jq_program(detected, event);
                for out in &canonical_outputs {
                    let canonical_wire = serde_json::to_string(out).expect("serialize");
                    let rust_text = out
                        .to_provider_stdout(detected, event)
                        .expect("Rust to_provider_stdout");
                    let rust_value: Value =
                        serde_json::from_str(&rust_text).expect("parse Rust out");
                    let jq_value = run_jq(&program, &canonical_wire);
                    assert_eq!(
                        rust_value, jq_value,
                        "cross-host output parity mismatch: detected={detected:?}, event={event:?}",
                    );
                }
            }
        }
    }

    #[test]
    fn shim_contains_logging_primitives() {
        let s = shim_script(ProviderName::Claude, HookEvent::PreToolUse);
        assert!(s.contains("AGENTSPEC_HOOK_LOG"), "missing env var capture");
        assert!(s.contains("_alog"), "missing _alog helper");
        assert!(s.contains("_ALOG"), "missing _ALOG variable");
    }

    #[test]
    fn e2e_no_log_file_created_when_env_unset() {
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
printf '%s' '{"permission_decision":"allow"}'
"#,
        );
        let log_path = dir.path().join("hooks.log");
        let provider_stdin = r#"{"session_id":"s","cwd":"/p","tool_name":"Bash","tool_input":{}}"#;
        let (code, stdout, stderr) = exec_shim_with_stdin(&shim, &user, provider_stdin);
        assert_eq!(code, 0, "stderr={stderr}");
        assert!(
            !log_path.exists(),
            "log file should not be created when AGENTSPEC_HOOK_LOG is unset"
        );
        let parsed: Value = serde_json::from_str(stdout.trim()).expect("parse output");
        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "allow");
    }

    #[test]
    fn e2e_log_file_captures_all_pipeline_stages() {
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
printf '%s' '{"permission_decision":"allow"}'
"#,
        );
        let log_path = dir.path().join("hooks.log");
        let provider_stdin = r#"{"session_id":"test-sess","cwd":"/p","tool_name":"Bash","tool_input":{"command":"ls"}}"#;
        let (code, stdout, stderr) = exec_shim_with_env(
            &shim,
            &user,
            provider_stdin,
            &[("AGENTSPEC_HOOK_LOG", &log_path.to_string_lossy())],
        );
        assert_eq!(code, 0, "stderr={stderr}");

        assert!(log_path.exists(), "log file should be created");
        let log = std::fs::read_to_string(&log_path).expect("read log");

        assert!(log.contains("raw_input"), "log missing raw_input label");
        assert!(log.contains("test-sess"), "log missing raw input payload");
        assert!(
            log.contains("canonical_input"),
            "log missing canonical_input label"
        );
        assert!(
            log.contains("schema_version"),
            "log missing canonical fields"
        );
        assert!(
            log.contains("\"provider\""),
            "log missing provider in canonical"
        );
        assert!(log.contains("\"event\""), "log missing event in canonical");
        assert!(log.contains("user_stdout"), "log missing user_stdout label");
        assert!(log.contains("user_exit=0"), "log missing user_exit");
        assert!(
            log.contains("provider_output"),
            "log missing provider_output label"
        );
        assert!(
            log.contains("hookSpecificOutput"),
            "log missing provider-shaped output"
        );

        let parsed: Value = serde_json::from_str(stdout.trim()).expect("parse output");
        assert_eq!(
            parsed["hookSpecificOutput"]["permissionDecision"], "allow",
            "logging must not interfere with stdout"
        );
    }

    #[test]
    fn e2e_log_empty_user_stdout() {
        if !jq_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = write_shim(dir.path(), ProviderName::Claude, HookEvent::PreToolUse);
        let user = write_user_script(
            dir.path(),
            "user.sh",
            "#!/usr/bin/env sh\ncat > /dev/null\n",
        );
        let log_path = dir.path().join("hooks.log");
        let provider_stdin = r#"{"session_id":"s","cwd":"/p","tool_name":"Bash","tool_input":{}}"#;
        let (code, stdout, stderr) = exec_shim_with_env(
            &shim,
            &user,
            provider_stdin,
            &[("AGENTSPEC_HOOK_LOG", &log_path.to_string_lossy())],
        );
        assert_eq!(code, 0, "stderr={stderr}");
        assert!(stdout.is_empty(), "stdout should be empty");

        let log = std::fs::read_to_string(&log_path).expect("read log");
        assert!(log.contains("user_stdout"), "log missing user_stdout label");
        assert!(log.contains("user_exit=0"), "log missing user_exit");
        assert!(
            !log.contains("provider_output"),
            "provider_output should not appear when user output is empty"
        );
    }

    #[test]
    fn e2e_log_nonzero_exit() {
        if !jq_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = write_shim(dir.path(), ProviderName::Claude, HookEvent::PreToolUse);
        let user = write_user_script(
            dir.path(),
            "user.sh",
            "#!/usr/bin/env sh\ncat > /dev/null\nexit 3\n",
        );
        let log_path = dir.path().join("hooks.log");
        let provider_stdin = r#"{"session_id":"s","cwd":"/p","tool_name":"Bash","tool_input":{}}"#;
        let (code, _stdout, _stderr) = exec_shim_with_env(
            &shim,
            &user,
            provider_stdin,
            &[("AGENTSPEC_HOOK_LOG", &log_path.to_string_lossy())],
        );
        assert_eq!(code, 3);

        let log = std::fs::read_to_string(&log_path).expect("read log");
        assert!(
            log.contains("user_exit=3"),
            "log should capture non-zero exit code"
        );
    }
}
