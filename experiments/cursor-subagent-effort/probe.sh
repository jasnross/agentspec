#!/usr/bin/env bash
# Does Cursor parse a `[effort=…]` bracket option in subagent frontmatter?
#
# Human-driven, machine-observed: a person drives Cursor, but the answer lands
# in a hook payload and no one has to interpret it.
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

probe_human_run "$package" cursor-subagent-effort "
  1. Open Cursor on that directory.
  2. Fully quit and reopen Cursor — it reads .cursor/hooks.json at start,
     so a window opened before this point will not fire the hook.
  3. Invoke the \"arm-effort-low\" subagent.

This script is waiting and will record the result automatically.
Nothing else is required of you.
"
