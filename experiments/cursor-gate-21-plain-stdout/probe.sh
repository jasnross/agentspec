#!/usr/bin/env bash
# Gate #21 — does Cursor inject a sessionStart hook's plain stdout into the
# agent's context, the way Claude does?
#
# Human-judged: whether the agent knew the planted fact is a person's call.
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

probe_human_run "$package" cursor-gate-21-plain-stdout "
  1. Open Cursor on that directory.
  2. Fully quit and reopen Cursor — it reads .cursor/hooks.json at start.
  3. Start a FRESH conversation (not a resume — sessionStart does not fire
     on resume, per the cursor-session-start probe).
  4. Ask the agent: \"What pet does the user own and what is its name?\"

This script is waiting. Once the hook has fired it will ask you what happened.
"
