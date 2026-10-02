#!/usr/bin/env bash
# Under an OpenCode agent `permission` map that leads with `"*": "deny"` and
# allows `read`, does reading a file outside the project raise a permission
# prompt when `external_directory: "ask"` follows the allow — and fail on the
# rule when it does not?
#
# Fully script-driven: no human step, no credentials, and no model quota.
# OpenCode talks to a fake OpenAI-compatible provider on loopback, which scripts
# one `read` tool call and writes every request body to disk. The tool result
# OpenCode sends back in the follow-up request is the oracle, so the measurement
# is at `outbound-request` depth.
set -euo pipefail

package=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=experiments/lib/probe-common.sh
. "$package/../lib/probe-common.sh"

probe_require_tools git jq opencode python3

# A runner takes no arguments. The arms discriminate against each other, so
# there is no discriminator fixture to select; see the README.
if [ $# -ne 0 ]; then
	printf 'probe: this runner takes no arguments (got: %s)\n' "$*" >&2
	printf 'probe: set PROBE_DRY_RUN=1 in the environment instead.\n' >&2
	exit 2
fi

dry_run="${PROBE_DRY_RUN:-0}"
case "$dry_run" in
0 | 1) ;;
*)
	printf 'probe: PROBE_DRY_RUN must be 0 or 1 (got: %s)\n' "$dry_run" >&2
	exit 2
	;;
esac

PROBE_MARKER=AGENTSPEC-PROBE-MARKER-OCEXTR6
OUTSIDE_CONTENT=AGENTSPEC-OUTSIDE-CONTENT-OCEXTR6
ARMS=(restated bare)

ws=$(probe_workspace_create opencode-agent-permission-external-read)

probe_fail() {
	exit 1
}

# One trap for the whole run, registered before the first fake starts. An exit
# from any arm — including a `set -e` abort partway through one, or a helper
# such as `probe_template_file` exiting on its own — then stops the fake still
# running, rather than leaving a server bound to loopback, and names the kept
# workspace. `run_arm` empties the array once it has reaped its own fake.
fake_pids=()
on_exit() {
	local rc=$? pid
	for pid in "${fake_pids[@]+"${fake_pids[@]}"}"; do
		kill "$pid" 2>/dev/null || true
	done
	if [ "$rc" -ne 0 ]; then
		printf 'probe: the workspace has been kept for inspection: %s\n' "$ws" >&2
	fi
}
trap on_exit EXIT

run_arm() {
	local arm="$1" agent="$2"
	local dir="$ws/$arm" port="" fake_pid waited=0 target

	mkdir -p "$dir/project" "$dir/outside" "$dir/sink" \
		"$dir/home" "$dir/xdg/config" "$dir/xdg/cache" "$dir/xdg/data" "$dir/xdg/state"
	cp -R "$package/fixtures/project/." "$dir/project/"

	# The target sits beside `project/`, outside it. OpenCode compares a path
	# against the git worktree when the project has one; only a non-git project,
	# whose worktree is `/`, falls back to the project directory alone, which is
	# the comparison the README traces.
	if git -C "$dir" rev-parse >/dev/null 2>&1; then
		printf 'probe: arm %s: the workspace %s is inside a git repository.\n' "$arm" "$dir" >&2
		printf 'probe: OpenCode would compare the read against that worktree instead; set TMPDIR outside any repository.\n' >&2
		probe_fail
	fi
	target="$dir/outside/target.txt"
	printf '%s\n' "$OUTSIDE_CONTENT" >"$target"

	# Created here rather than by the background child's redirect, so the first
	# poll cannot race the child to the file and abort the run under `set -e`.
	: >"$dir/fake.port"
	python3 "$package/fixtures/fake_provider.py" "$dir/sink" "$target" "$PROBE_MARKER" >"$dir/fake.port" 2>"$dir/fake.stderr" &
	fake_pid=$!
	fake_pids+=("$fake_pid")

	while [ "$waited" -lt 50 ]; do
		port=$(sed -n '1p' "$dir/fake.port")
		[ -n "$port" ] && break
		sleep 0.1
		waited=$((waited + 1))
	done
	if [ -z "$port" ]; then
		printf 'probe: arm %s: the fake provider printed no port within 5 s; its stderr follows:\n' "$arm" >&2
		cat "$dir/fake.stderr" >&2
		probe_fail
	fi

	probe_template_file "$package/fixtures/project/opencode.json" "$dir/project/opencode.json" \
		"PORT=$port"

	# The `XDG_*` redirect keeps the operator's global config, plugins, and
	# cache out of the run. `HOME` is redirected too, because OpenCode also
	# reads `~/.opencode/` as a config directory, located from the home
	# directory rather than from any `XDG_*` variable. Neither redirect blocks
	# the four cleared config variables, each of which would inject config — or
	# permission rules directly — from the operator's shell, nor
	# `OPENCODE_DISABLE_PROJECT_CONFIG`, which would discard the fixture's own
	# agents and fail every arm at the marker gate with no obvious cause.
	#
	# No `--auto`: the non-interactive runner then rejects every permission ask
	# rather than approving it, and nothing is ever approved during the run.
	# Output goes to files: `opencode run`'s output is not the oracle, but it is
	# what diagnoses a failed arm, and it carries the `auto-rejecting` line.
	if ! (
		cd "$dir/project" &&
			env -u OPENCODE_CONFIG -u OPENCODE_CONFIG_DIR -u OPENCODE_CONFIG_CONTENT -u OPENCODE_PERMISSION \
				-u OPENCODE_DISABLE_PROJECT_CONFIG \
				HOME="$dir/home" \
				XDG_CONFIG_HOME="$dir/xdg/config" \
				XDG_CACHE_HOME="$dir/xdg/cache" \
				XDG_DATA_HOME="$dir/xdg/data" \
				XDG_STATE_HOME="$dir/xdg/state" \
				OPENCODE_DISABLE_AUTOUPDATE=1 \
				OPENCODE_DISABLE_MODELS_FETCH=1 \
				opencode run --agent "$agent" -m fake/m "hi"
	) >"$dir/stdout" 2>"$dir/stderr" </dev/null; then
		printf 'probe: arm %s: "opencode run --agent %s" exited nonzero; its stderr follows:\n' "$arm" "$agent" >&2
		cat "$dir/stderr" >&2
		probe_fail
	fi

	# One fake at a time, so no arm's requests can land in another's sink.
	kill "$fake_pid" 2>/dev/null || true
	wait "$fake_pid" 2>/dev/null || true
	# Reaped, so its PID is free for reuse; the trap must not signal it again.
	fake_pids=()
}

run_arm restated probe-restated
run_arm bare probe-bare

view_parts=()
for arm in "${ARMS[@]}"; do
	shopt -s nullglob
	bodies=("$ws/$arm/sink"/*.request.json)
	shopt -u nullglob
	if [ "${#bodies[@]}" -eq 0 ]; then
		printf 'probe: arm %s: the fake provider received no request\n' "$arm" >&2
		probe_fail
	fi
	jq -s . "${bodies[@]}" >"$ws/$arm/requests.json"
	view_parts+=(--slurpfile "$arm" "$ws/$arm/requests.json")
done
jq -n "${view_parts[@]}" \
	'{restated: $restated[0], bare: $bare[0]}' >"$ws/view.json"

# OpenCode warns rather than fails on an unknown `--agent`, and runs the default
# agent instead. That agent's requests carry no marker, so an arm without one
# measured an agent other than its fixture.
# shellcheck disable=SC2016 # the jq program is correctly single-quoted; $m is a jq variable
governed='[.[] | select(any(.messages[]?; .role == "system" and (.content | tostring | contains($m))))]'
for arm in "${ARMS[@]}"; do
	if ! jq -e --arg m "$PROBE_MARKER" --arg arm "$arm" ".[\$arm] | $governed | length > 0" "$ws/view.json" >/dev/null; then
		printf 'probe: arm %s: no request carried %s in a system message.\n' "$arm" "$PROBE_MARKER" >&2
		printf 'probe: OpenCode may have fallen back to its default agent; see %s.\n' "$ws/$arm/stderr" >&2
		printf 'probe: this is an apparatus failure, not a refutation; no record written.\n' >&2
		probe_fail
	fi
done

# The oracle is the tool result in the follow-up request. Without one, the arm
# says nothing about permissions: either OpenCode never executed the scripted
# call, or the session stopped at the rejection before sending a follow-up.
for arm in "${ARMS[@]}"; do
	if ! jq -e --arg m "$PROBE_MARKER" --arg arm "$arm" \
		".[\$arm] | $governed | any(.[]; any(.messages[]?; .role == \"tool\"))" "$ws/view.json" >/dev/null; then
		printf 'probe: arm %s: no marked request carried a tool result.\n' "$arm" >&2
		printf 'probe: either OpenCode never executed the scripted read, or the session stopped after the rejection —\n' >&2
		printf 'probe: a release that no longer honors experimental.continue_loop_on_deny does that. See %s.\n' "$ws/$arm/stderr" >&2
		printf 'probe: this is an apparatus failure, not a refutation; no record written.\n' >&2
		probe_fail
	fi
done

# Printed before any recording branch: a failed or dry `record.sh` leaves the
# workspace, and this path is what a candidate projection is iterated against.
printf 'probe: the assembled view is at %s\n' "$ws/view.json" >&2

if [ "$dry_run" = 1 ]; then
	"$package/../lib/record.sh" \
		--manifest "$package/probe.json" \
		--view "$ws/view.json" \
		--dry-run || probe_fail
	exit 0
fi

"$package/../lib/record.sh" \
	--manifest "$package/probe.json" \
	--view "$ws/view.json" || probe_fail

# Only on a successful recording run. Every path above keeps the workspace.
rm -rf "$ws"
