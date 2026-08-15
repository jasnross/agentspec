#!/usr/bin/env bash
# Gate #19 — when a preToolUse hook exits 2 AND emits deny JSON, which of its
# marker strings does Cursor surface?
#
# Human-judged: the oracle is a person looking at the screen. The runner proves
# the hook fired before asking, so the human answers only what no machine can.
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

probe_human_run "$package" cursor-gate-19-output-json "
  1. Open Cursor on that directory.
  2. Fully quit and reopen Cursor — it reads .cursor/hooks.json at start.
  3. Ask the agent to run a shell command, e.g. \"Run: ls\".
     The hook will deny it.
  4. Look for AGENTSPEC_GATE19_USER_MARKER_0123456789 in Cursor's UI.
  5. In a follow-up prompt, ask the agent to repeat back the last hook
     message it saw — look for AGENTSPEC_GATE19_AGENT_MARKER_9876543210.

This script is waiting. Once the hook has fired it will ask you what you saw.
"
