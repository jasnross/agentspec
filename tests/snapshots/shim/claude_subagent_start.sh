#!/usr/bin/env sh
# agentspec-generated shim: claude subagent_start
# schema_version: 1.0.0
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

CANONICAL=$(jq -c '{
  schema_version: "1.0.0",
  provider: "claude",
  event: "subagent_start",
  session_id: .session_id,
  agent_id: .agent_id,
  cwd: .cwd,
  transcript_path: .transcript_path,
  provider_raw: .
}')
JQ_INPUT_EXIT=$?
if [ "$JQ_INPUT_EXIT" -ne 0 ]; then
    printf 'agentspec: input translation failed (jq exited %s); provider stdin is not valid JSON or did not match the expected shape\n' "$JQ_INPUT_EXIT" >&2
    exit 1
fi

USER_OUTPUT=$(printf '%s' "$CANONICAL" | "$1")
USER_EXIT=$?

if [ -n "$USER_OUTPUT" ]; then
    printf '%s' "$USER_OUTPUT" | jq -c '{
  hookSpecificOutput: ({
    hookEventName: "SubagentStart",
    permissionDecision: .permission_decision,
    permissionDecisionReason: (.decision_reason // .user_facing_message),
    additionalContext: .additional_context,
    updatedInput: .updated_input
  } | with_entries(select(.value != null)))
}'
    JQ_OUTPUT_EXIT=$?
    if [ "$JQ_OUTPUT_EXIT" -ne 0 ]; then
        printf 'agentspec: output translation failed (jq exited %s); user script emitted output that is not valid canonical JSON\n' "$JQ_OUTPUT_EXIT" >&2
        exit 1
    fi
fi

exit "$USER_EXIT"
