#!/usr/bin/env sh
# agentspec-generated shim: cursor stop
# schema_version: 1.0.0
# DO NOT EDIT — regenerate via `agentspec compile`.

HOOK_ID="${2:-}"
LOG_TAG="agentspec [$HOOK_ID]"

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

CANONICAL=$(jq -c '{
  schema_version: "1.0.0",
  provider: "cursor",
  event: "stop",
  session_id: (.parent_conversation_id // .conversation_id),
  agent_id: (if .parent_conversation_id then .conversation_id else null end),
  cwd: .workspace_roots[0],
  transcript_path: .transcript_path,
  provider_raw: .
}')
JQ_INPUT_EXIT=$?
if [ "$JQ_INPUT_EXIT" -ne 0 ]; then
    printf '%s: input translation failed (jq exited %s); provider stdin is not valid JSON or did not match the expected shape\n' "$LOG_TAG" "$JQ_INPUT_EXIT" >&2
    exit 1
fi

USER_OUTPUT=$(printf '%s' "$CANONICAL" | "$1")
USER_EXIT=$?

if [ -n "$USER_OUTPUT" ]; then
    exec 9>&1
    JQ_ERR=$(printf '%s' "$USER_OUTPUT" | jq -c '(if type != "object" then error("expected JSON object, got " + type) else . end) | (([keys[] | select(. as $k | ["schema_version","permission_decision","decision_reason","user_facing_message","additional_context","updated_input"] | index($k) | not)]) as $u | if ($u | length) > 0 then error("unrecognized canonical output fields: " + ($u | join(", "))) else . end) | {
  permission: .permission_decision,
  agent_message: .decision_reason,
  user_message: .user_facing_message,
  additional_context: .additional_context,
  updated_input: .updated_input
} | with_entries(select(.value != null))' 2>&1 1>&9)
    JQ_OUTPUT_EXIT=$?
    exec 9>&-
    if [ "$JQ_OUTPUT_EXIT" -ne 0 ]; then
        printf '%s: output translation failed (jq exited %s): %s\n' "$LOG_TAG" "$JQ_OUTPUT_EXIT" "$JQ_ERR" >&2
        exit 1
    fi
fi

exit "$USER_EXIT"
