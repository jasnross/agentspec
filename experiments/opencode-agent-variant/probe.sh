#!/usr/bin/env bash
# Does OpenCode read a top-level `variant:` key in agent frontmatter?
#
# Fully script-driven: no human step, no network, no credentials.
set -euo pipefail

package=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=experiments/lib/probe-common.sh
. "$package/../lib/probe-common.sh"

probe_require_tools jq opencode

ws=$(probe_workspace_create opencode-agent-variant)
cp -R "$package/fixtures/assertion/." "$ws/"

# The oracle's raw output goes to view.json unprojected — `record.sh` owns
# projection, so the expression a probe author validates is the one that runs.
if ! (cd "$ws" && opencode debug agent probe) >"$ws/view.json" 2>"$ws/oracle.stderr"; then
	printf 'probe: "opencode debug agent probe" exited nonzero:\n' >&2
	cat "$ws/oracle.stderr" >&2
	exit 1
fi

if [ ! -s "$ws/view.json" ]; then
	printf 'probe: the oracle produced no output; its stderr follows:\n' >&2
	cat "$ws/oracle.stderr" >&2
	exit 1
fi

"$package/../lib/record.sh" --manifest "$package/probe.json" --view "$ws/view.json"

# Only on success. Every failure path above exits before this, leaving the
# workspace and the oracle's stderr on disk for whoever has to diagnose it.
rm -rf "$ws"
