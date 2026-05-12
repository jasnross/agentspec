#!/usr/bin/env bash
# Gate #19 — does Cursor consume JSON output fields when the script exits 2?
#
# Procedure:
#   1. Symlink (or copy) cursor-hooks-snippet-19.json contents into
#      ~/.cursor/hooks.json (or <project>/.cursor/hooks.json after editing the
#      `command` to point at this script's absolute path).
#   2. Restart Cursor; open a fresh conversation.
#   3. Ask the agent to run a shell command, e.g. "Run: ls".
#   4. Observe Cursor's UI for the user_message marker
#      (AGENTSPEC_GATE19_USER_MARKER_0123456789).
#   5. In a follow-up prompt, ask the agent to repeat back the last hook
#      message it saw — look for the agent_message marker
#      (AGENTSPEC_GATE19_AGENT_MARKER_9876543210).
#   6. Inspect $HOME/.cache/agentspec-gate19.log to confirm the hook fired
#      and to see the stdin payload Cursor sent.
#
# Result categories:
#   - Both markers visible → Cursor consumes JSON on exit 2 (best case).
#     Canonical schema can pair `permission: deny` + `user_facing_message`
#     with exit 2 and have both honored.
#   - Only user_message visible → partial consumption; agent_message routing
#     is silently dropped under exit 2.
#   - Neither visible (just generic deny) → exit 2 short-circuits JSON
#     entirely. Wrapper must choose: either exit 0 with deny JSON OR exit 2
#     with no JSON; can't combine.

set -euo pipefail

LOG_FILE="${GATE19_LOG:-$HOME/.cache/agentspec-gate19.log}"
mkdir -p "$(dirname "$LOG_FILE")"

# Capture stdin so we know the hook fired and have the payload for inspection.
payload="$(head -c 1048576)"
{
  printf '\n=== %s ===\nstdin payload:\n%s\n\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%S%z)" "$payload"
} >>"$LOG_FILE"

# Emit deny JSON with unique marker strings.
cat <<'JSON'
{
  "permission": "deny",
  "user_message": "AGENTSPEC_GATE19_USER_MARKER_0123456789",
  "agent_message": "AGENTSPEC_GATE19_AGENT_MARKER_9876543210"
}
JSON

exit 2
