#!/usr/bin/env sh
# agentspec-generated shim: cursor session_start
# schema_version: 1.0.0
# DO NOT EDIT — regenerate via `agentspec compile`.

HOOK_ID="${2:-}"
LOG_TAG="agentspec [$HOOK_ID]"

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

# The hook id (argv[2]) is guarded because 27 existing call sites invoke
# this shim with only the script path (argv[1]); an unguarded second
# `shift` on some `sh` implementations (dash) aborts the script when no
# argv[2] is present. Whenever argv[3..] (this entry's args) are supplied,
# argv[2] is therefore positionally mandatory — a caller that passes args
# without a hook id would have its first arg silently consumed here.
SCRIPT="$1"
shift; [ "$#" -gt 0 ] && shift

RAW=$(cat)
_alog "raw_input"
_alog "$RAW"

if printf '%s' "$RAW" | jq -e '.cursor_version' >/dev/null 2>&1; then
    _detected=cursor
    INPUT_JQ='{
  schema_version: "1.0.0",
  provider: "cursor",
  event: "session_start",
  session_id: (.parent_conversation_id // .conversation_id),
  agent_id: (if .parent_conversation_id then .conversation_id else null end),
  cwd: .workspace_roots[0],
  transcript_path: .transcript_path,
  provider_raw: .
}'
    OUTPUT_JQ='(if type != "object" then error("expected JSON object, got " + type) else . end) | (([keys[] | select(. as $k | ["schema_version","permission_decision","decision_reason","user_facing_message","additional_context","updated_input"] | index($k) | not)]) as $u | if ($u | length) > 0 then error("unrecognized canonical output fields: " + ($u | join(", "))) else . end) | {
  permission: .permission_decision,
  agent_message: .decision_reason,
  user_message: .user_facing_message,
  additional_context: .additional_context,
  updated_input: .updated_input
} | with_entries(select(.value != null))'
else
    _detected=claude
    INPUT_JQ='{
  schema_version: "1.0.0",
  provider: "claude",
  event: "session_start",
  session_id: .session_id,
  agent_id: .agent_id,
  cwd: .cwd,
  transcript_path: .transcript_path,
  provider_raw: .
}'
    OUTPUT_JQ='(if type != "object" then error("expected JSON object, got " + type) else . end) | (([keys[] | select(. as $k | ["schema_version","permission_decision","decision_reason","user_facing_message","additional_context","updated_input"] | index($k) | not)]) as $u | if ($u | length) > 0 then error("unrecognized canonical output fields: " + ($u | join(", "))) else . end) | {
  hookSpecificOutput: ({
    hookEventName: "SessionStart",
    permissionDecision: .permission_decision,
    permissionDecisionReason: (.decision_reason // .user_facing_message),
    additionalContext: .additional_context,
    updatedInput: .updated_input
  } | with_entries(select(.value != null)))
}'
fi
# Logs argument count, not the argument values themselves — unlike
# raw_input/canonical_input below, which log the full payload. Argument
# values may carry secrets a hooks.toml author did not intend to persist
# to a log file.
_alog "event=session_start provider=$_detected argc=$#"

CANONICAL=$(printf '%s' "$RAW" | jq -c "$INPUT_JQ")
JQ_INPUT_EXIT=$?
if [ "$JQ_INPUT_EXIT" -ne 0 ]; then
    _alog "error: input translation failed"
    printf '%s: input translation failed (jq exited %s); provider stdin is not valid JSON or did not match the expected shape\n' "$LOG_TAG" "$JQ_INPUT_EXIT" >&2
    exit 1
fi
_alog "canonical_input"
_alog "$CANONICAL"

USER_OUTPUT=$(printf '%s' "$CANONICAL" | "$SCRIPT" "$@")
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
