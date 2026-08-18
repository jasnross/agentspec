#!/usr/bin/env bash
# Does Cursor's sessionStart hook fire again when a conversation is resumed?
#
# The finding is an absence, so the assertion is a COUNT of sessionStart
# payloads. A poll cannot wait for a payload that never comes, so wait_for
# counts prompts instead: two beforeSubmitPrompt payloads prove the operator
# reached the second half of the procedure.
set -euo pipefail

package=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=experiments/lib/probe-common.sh
. "$package/../lib/probe-common.sh"

probe_require_tools jq

# A runner takes no arguments: it is one blocking invocation, so there is no
# workspace for a second one to point at.
if [ $# -ne 0 ]; then
	printf '%s: unexpected argument: %s\n' "$(basename "${BASH_SOURCE[0]}")" "$1" >&2
	exit 2
fi

probe_human_run "$package" cursor-session-start "
  1. Open Cursor on that directory.
  2. Fully quit and reopen Cursor — it reads .cursor/hooks.json at start.
  3. Submit a prompt in a new conversation.
  4. Fully quit Cursor, reopen it, and RESUME that same conversation
     from the conversation list.
  5. Submit another prompt in the resumed conversation.

This script is waiting and will record the result automatically.
"
