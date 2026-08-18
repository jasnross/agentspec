#!/usr/bin/env bash
# Does Claude Code's SessionStart hook fire again when a session is resumed?
#
# Human-driven, machine-observed: the assertion is the ordered list of `source`
# values, so a person drives Claude but nobody has to interpret the answer.
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

probe_human_run "$package" claude-session-start "
  1. cd into that directory and run: claude
  2. Submit one prompt (anything), then exit.
  3. Relaunch with: claude --resume   (or: claude -c)
  4. Submit another prompt.

This script is waiting and will record the result automatically.
"
