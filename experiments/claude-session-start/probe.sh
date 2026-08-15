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

probe_parse_runner_args "$@"

if [ -n "$probe_resume_workspace" ]; then
	probe_record_capture "$package" "$probe_resume_workspace"
	exit 0
fi

probe_human_run "$package" claude-session-start "
  1. cd into that directory and run: claude
  2. Submit one prompt (anything), then exit.
  3. Relaunch with: claude --resume   (or: claude -c)
  4. Submit another prompt.

This script is waiting and will record the result automatically.
"
