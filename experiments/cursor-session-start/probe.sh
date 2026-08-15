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

probe_parse_runner_args "$@"

if [ -n "$probe_resume_workspace" ]; then
	probe_record_capture "$package" "$probe_resume_workspace"
	exit 0
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
