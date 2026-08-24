#!/usr/bin/env bash
# Does OpenCode read a top-level `variant:` key in command frontmatter?
#
# Fully script-driven: no human step, no network, no credentials.
set -euo pipefail

package=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=experiments/lib/probe-common.sh
. "$package/../lib/probe-common.sh"

probe_require_tools jq opencode

# A runner takes no arguments. The other fixture arms exist for the authoring-
# time discrimination check, which runs `record.sh --dry-run` against a saved
# view rather than through this script; see the README. Whether they should be
# reachable from here at all is `TODO.md` #24.
if [ $# -ne 0 ]; then
	printf '%s: unexpected argument: %s\n' "$(basename "${BASH_SOURCE[0]}")" "$1" >&2
	exit 2
fi

ws=$(probe_workspace_create opencode-command-variant)
cp -R "$package/fixtures/assertion/." "$ws/"

# Redirected to a file rather than piped: `opencode debug config` emits roughly
# 458 KB and truncates at 65536 bytes when stdout is a pipe. Piping would hand
# `record.sh` a half-written JSON document with no error to distinguish it.
if ! (cd "$ws" && opencode debug config --pure) >"$ws/view.json" 2>"$ws/oracle.stderr"; then
	printf 'probe: "opencode debug config --pure" exited nonzero:\n' >&2
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

# This oracle resolves the whole config rather than addressing one command by
# name, so a fixture it never discovered is not an error: the run exits 0 with
# a valid view, `.command["agentspec-probe-variant"]` is `null`, the projection
# yields `{"model":null,"variant":null}`, and `record.sh` writes `refuted`.
# Under the contract `refuted` is the strong "the provider changed" signal, and
# records are append-only — so an apparatus failure would masquerade as a
# provider finding, permanently. The discriminator arm refutes on a *present*
# command resolving `variant` to `null`, which is a different observation.
if ! jq -e '.command | has("agentspec-probe-variant")' "$ws/view.json" >/dev/null 2>&1; then
	# The names, not just a count: the likeliest cause of this branch firing is a
	# change in how OpenCode discovers or namespaces commands, and only the names
	# reveal that. A count says the apparatus broke but not how.
	printf 'probe: the fixture command did not resolve. OpenCode resolved %s commands:\n' \
		"$(jq '.command | length' "$ws/view.json")" >&2
	jq -r '.command | keys[]' "$ws/view.json" | sed 's/^/probe:   /' >&2
	printf 'probe: this is an apparatus failure, not a refutation; no record written.\n' >&2
	printf 'probe: the workspace is preserved at %s\n' "$ws" >&2
	exit 1
fi

"$package/../lib/record.sh" --manifest "$package/probe.json" --view "$ws/view.json"

# Only on success. Every failure path above exits before this, leaving the
# workspace and the oracle's stderr on disk for whoever has to diagnose it.
rm -rf "$ws"
