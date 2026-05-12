#!/usr/bin/env bash
# Gate #21 — does Cursor's sessionStart accept plain (non-JSON) stdout
# and inject it into the agent's context, the way Claude does?
#
# Procedure:
#   1. Edit cursor-hooks-snippet-21.json to point `command` at this script's
#      absolute path. Drop into ~/.cursor/hooks.json (or
#      <project>/.cursor/hooks.json).
#   2. Quit Cursor entirely and reopen it; start a *fresh* conversation
#      (NOT a resume — sessionStart does not fire on resume per the
#      session-id-resume experiment).
#   3. Ask the agent: "What pet does the user own and what is its name?"
#   4. Result categories:
#      - Agent answers "Quizzlebottom-2026, a hamster" (or similar quoting
#        the marker) → plain stdout was injected as context. Canonical
#        context-injection path can pass through plain stdout on Cursor too,
#        no JSON envelope required.
#      - Agent says it doesn't know → plain stdout NOT injected on Cursor.
#        Canonical context injection on sessionStart MUST use the
#        `additional_context` JSON envelope when targeting Cursor.
#   5. Cross-check $HOME/.cache/agentspec-gate21.log to confirm the hook
#      actually fired (vs. didn't fire at all — a third failure mode).

set -euo pipefail

LOG_FILE="${GATE21_LOG:-$HOME/.cache/agentspec-gate21.log}"
mkdir -p "$(dirname "$LOG_FILE")"

payload="$(head -c 1048576)"
{
  printf '\n=== %s ===\nstdin payload:\n%s\n\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%S%z)" "$payload"
} >>"$LOG_FILE"

# Plain stdout — no JSON envelope. Unique marker phrase.
echo "AGENTSPEC_GATE21_CONTEXT_MARKER: The user owns a hamster named Quizzlebottom-2026."

exit 0
