#!/usr/bin/env bash
# Does Cursor parse a `[effort=…]` bracket option in subagent frontmatter?
#
# Human-driven, machine-observed. One blocking invocation: it materializes a
# throwaway workspace, prints what to do, then polls the capture and records
# automatically. No second command and no keypress.
#
# Blocking is what deletes the stale-capture failure this probe would otherwise
# have to guard against — a process that never exits has no last week, because
# the workspace was created by the invocation still running. The two-invocation
# form survives as `--capture <dir>` for a terminal that has since closed.
set -euo pipefail

package=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=experiments/lib/probe-common.sh
. "$package/../lib/probe-common.sh"

probe_require_tools jq

resume_workspace=""
while [ $# -gt 0 ]; do
	case "$1" in
	--capture)
		# An empty value must not fall through to the blocking path: that would
		# turn a resume into a fresh 15-minute live-session run.
		if [ $# -lt 2 ] || [ -z "$2" ]; then
			printf 'probe: --capture requires a workspace path\n' >&2
			exit 1
		fi
		[ -d "$2" ] || {
			printf 'probe: no such workspace: %s\n' "$2" >&2
			exit 1
		}
		resume_workspace="$2"
		shift 2
		;;
	*)
		printf 'probe: unknown argument: %s\n' "$1" >&2
		exit 1
		;;
	esac
done

record_from_capture() {
	local ws="$1"
	local payloads="$ws/capture/payloads.jsonl"

	# Distinguish "not a probe workspace" from "the hook never fired" — the
	# same message for both sends the operator after the wrong cause.
	if [ ! -d "$ws/capture" ]; then
		printf 'probe: %s is not a probe workspace (no capture/ directory)\n' "$ws" >&2
		exit 1
	fi

	# Refusing to record is the correct outcome for an empty capture: the
	# failure stays loud rather than becoming a record nobody can trust.
	if [ ! -s "$payloads" ]; then
		printf 'probe: no captured payloads at %s\n' "$payloads" >&2
		printf 'probe: the hook did not fire. Check that Cursor was fully quit and reopened.\n' >&2
		exit 1
	fi

	jq -s . "$payloads" >"$ws/view.json"
	"$package/../lib/record.sh" \
		--manifest "$package/probe.json" \
		--view "$ws/view.json" \
		--capture "$ws"
}

if [ -n "$resume_workspace" ]; then
	record_from_capture "$resume_workspace"
	exit 0
fi

ws=$(probe_workspace_create cursor-subagent-effort)

# Never write outside the generated workspace. A temp directory is never an
# `agentspec sync` target, which makes a collision with sync's `_agentspec_id`
# ownership of .cursor/hooks.json structurally impossible rather than merely
# warned against.
capture_script="$ws/capture/dump-hook.sh"
# jq's absolute path is baked into the hook: a Cursor-spawned hook may not
# inherit an interactive PATH, and an empty capture costs a live session.
probe_template_file "$package/fixtures/capture/dump-hook.sh" "$capture_script" \
	"JQ=$(command -v jq)" \
	"PAYLOADS=$ws/capture/payloads.jsonl" \
	"RUN_STAMP=$(probe_run_stamp_token "$ws")"
chmod +x "$capture_script"

probe_template_file "$package/fixtures/.cursor/hooks.json" "$ws/.cursor/hooks.json" \
	"CAPTURE_SCRIPT=$capture_script"

# A workspace path containing a quote or backslash would produce a hooks.json
# Cursor silently ignores. Fail here instead, where the cause is obvious.
jq -e . "$ws/.cursor/hooks.json" >/dev/null 2>&1 || {
	printf 'probe: generated hooks.json is not valid JSON — check the workspace path: %s\n' "$ws" >&2
	exit 1
}

mkdir -p "$ws/.cursor/agents"
cp "$package/fixtures/.cursor/agents/"*.md "$ws/.cursor/agents/"

cat >&2 <<INSTRUCTIONS

Workspace ready: $ws

  1. Open Cursor on that directory.
  2. Fully quit and reopen Cursor — it reads .cursor/hooks.json at start,
     so a window opened before this point will not fire the hook.
  3. Invoke the "arm-effort-low" subagent.

This script is waiting and will record the result automatically.
Nothing else is required of you.

INSTRUCTIONS

wait_for=$(jq -r '.wait_for // ""' "$package/probe.json")
# A missing wait_for would poll a filter of the literal string "null", which
# never matches — the run would burn its full timeout for no reason.
[ -n "$wait_for" ] || {
	printf 'probe: manifest declares no wait_for filter\n' >&2
	exit 1
}

if probe_wait_for_capture "$ws" "$wait_for" 900; then
	record_from_capture "$ws"
	exit 0
fi

# Never delete the workspace on timeout. The capture may still arrive, and
# discarding a workspace the operator spent a live session on is the one
# unrecoverable mistake this runner could make.
cat >&2 <<RESUME

probe: timed out waiting for the subagentStart payload.

The workspace has been kept. If the capture arrives later, finish the run with:

  $0 --capture $ws

RESUME
exit 1
