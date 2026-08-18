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

# A runner takes no arguments: it is one blocking invocation, so there is no
# workspace for a second one to point at.
if [ $# -ne 0 ]; then
	printf '%s: unexpected argument: %s\n' "$(basename "${BASH_SOURCE[0]}")" "$1" >&2
	exit 2
fi

probe_human_run "$package" cursor-subagent-effort "
  1. Open Cursor on that directory.
  2. Fully quit and reopen Cursor — it reads .cursor/hooks.json at start,
     so a window opened before this point will not fire the hook.
  3. Invoke the \"arm-effort-low\" subagent.

This script is waiting and will record the result automatically.
Nothing else is required of you.
"
