#!/usr/bin/env bash
# Does OpenCode discard `model`, `variant`, and `tools` from skill frontmatter?
#
# Fully script-driven: no human step, no network, no credentials.
set -euo pipefail

package=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=experiments/lib/probe-common.sh
. "$package/../lib/probe-common.sh"

probe_require_tools jq opencode

# A runner takes no arguments: it is one invocation against one fixture, so
# there is no second arm for an argument to select.
if [ $# -ne 0 ]; then
	printf '%s: unexpected argument: %s\n' "$(basename "${BASH_SOURCE[0]}")" "$1" >&2
	exit 2
fi

ws=$(probe_workspace_create opencode-skill-frontmatter-discard)
cp -R "$package/fixtures/assertion/." "$ws/"

# Redirected to a file rather than piped: `opencode debug` truncates its stdout
# at 65536 bytes when it is a pipe, and the resolved skill list is far larger.
# A pipe would silently hand `record.sh` a half-written JSON document.
if ! (cd "$ws" && opencode debug skill --pure) >"$ws/view.json" 2>"$ws/oracle.stderr"; then
	printf 'probe: "opencode debug skill --pure" exited nonzero:\n' >&2
	cat "$ws/oracle.stderr" >&2
	exit 1
fi

if [ ! -s "$ws/view.json" ]; then
	printf 'probe: the oracle produced no output; its stderr follows:\n' >&2
	cat "$ws/oracle.stderr" >&2
	exit 1
fi

# Named here rather than left to `record.sh`, which would report the failure as
# "projection failed" and send the reader after an expression that is correct.
if ! jq -e . "$ws/view.json" >/dev/null 2>&1; then
	printf 'probe: the oracle wrote a document jq cannot parse — %s bytes at %s\n' \
		"$(wc -c <"$ws/view.json")" "$ws/view.json" >&2
	exit 1
fi

# This oracle enumerates every resolvable skill rather than addressing one by
# name, so a fixture it never discovered is not an error: the run exits 0 with
# a valid view of all the *other* skills, the projection resolves to `null`,
# and `record.sh` writes `refuted`. Under the contract `refuted` is the strong
# "the provider changed" signal, and records are append-only — so an apparatus
# failure would masquerade as a provider finding, permanently.
#
# The sibling `opencode-agent-variant` needs no such guard: `opencode debug
# agent <name>` is name-addressed and exits nonzero when the agent is missing.
matches=$(jq '[.[] | select(.name == "agentspec-probe-discard")] | length' "$ws/view.json")
if [ "$matches" != 1 ]; then
	printf 'probe: the fixture resolved %s times, expected exactly 1.\n' "$matches" >&2
	printf 'probe: this is an apparatus failure, not a refutation; no record written.\n' >&2
	printf 'probe: the workspace is preserved at %s\n' "$ws" >&2
	exit 1
fi

"$package/../lib/record.sh" --manifest "$package/probe.json" --view "$ws/view.json"

# Only on success. Every failure path above exits before this, leaving the
# workspace and the oracle's stderr on disk for whoever has to diagnose it.
rm -rf "$ws"
