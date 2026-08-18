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

# A runner takes no arguments: it is one blocking invocation, so there is no
# workspace for a second one to point at.
if [ $# -ne 0 ]; then
	printf '%s: unexpected argument: %s\n' "$(basename "${BASH_SOURCE[0]}")" "$1" >&2
	exit 2
fi

probe_human_run "$package" cursor-gate-21-plain-stdout "
  1. Open Cursor on that directory.
  2. Fully quit and reopen Cursor — it reads .cursor/hooks.json at start.
  3. Start a FRESH conversation (not a resume — sessionStart does not fire
     on resume, per the cursor-session-start probe).
  4. Ask the agent: \"What pet does the user own and what is its name?\"

This script is waiting. Once the hook has fired it will ask you what happened.
"
