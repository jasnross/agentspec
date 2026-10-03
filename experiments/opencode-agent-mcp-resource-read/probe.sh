#!/usr/bin/env bash
# Under an OpenCode agent map that leads with `"*": "deny"` and sets `read` to
# `{"*": "allow", "mcp:*": "deny"}`, which tools is the agent offered, and does
# each MCP resource call fail on the rule while a project-file read still
# succeeds?
#
# Fully script-driven: no human step, no credentials, and no model quota.
# OpenCode talks to a fake OpenAI-compatible provider on loopback, which scripts
# one tool call per arm and writes every request body to disk. The tool result
# OpenCode sends back in the follow-up request is the oracle, so the measurement
# is at `outbound-request` depth.
set -euo pipefail

package=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=experiments/lib/probe-common.sh
. "$package/../lib/probe-common.sh"

probe_require_tools jq opencode python3

# A runner takes no arguments. The arms discriminate against each other, so
# there is no discriminator fixture to select (`TODO.md` #24).
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

PROBE_MARKER=AGENTSPEC-PROBE-MARKER-OCRES3
RESOURCE_CALL='{"server":"fx","uri":"fx://AGENTSPEC-RESOURCE-OCRES3"}'
ARMS=(map_read_resource map_list_resources map_list_templates map_read_file plain_read_resource plain_list_resources plain_list_templates)

ws=$(probe_workspace_create opencode-agent-mcp-resource-read)

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

# The MCP server sits outside every arm's project. The project's
# `opencode.json` still names its path, as it must for OpenCode to spawn it;
# that is harmless here, because the oracle is the request dump and the model
# answering it is a fake that searches nothing.
mkdir -p "$ws/fx"
cp "$package/fixtures/fx_server.py" "$ws/fx/fx_server.py"

# Each arm names the agent it runs and the one call the fake scripts for it. A
# `read` arm's arguments name the arm's own project file, which exists only
# once the arm's directory does, so `run_arm` fills in the path.
run_arm() {
	local arm="$1" agent="$2" tool="$3" arguments="$4"
	local dir="$ws/$arm" port="" fake_pid waited=0

	mkdir -p "$dir/project" "$dir/sink" \
		"$dir/home" "$dir/xdg/config" "$dir/xdg/cache" "$dir/xdg/data" "$dir/xdg/state"
	cp -R "$package/fixtures/project/." "$dir/project/"

	# A missing file would fail the `read` arm's call for a reason that has
	# nothing to do with permissions.
	if [ ! -e "$dir/project/notes.txt" ]; then
		printf 'probe: arm %s: %s is missing, so a read of it could not succeed.\n' "$arm" "$dir/project/notes.txt" >&2
		probe_fail
	fi
	# The path is the physical one. On macOS `$TMPDIR` sits under `/var`, a
	# symlink to `/private/var`; OpenCode compares the path against its resolved
	# working directory, so the unresolved spelling of a project file reads as
	# outside the project and the call fails on `external_directory` instead.
	if [ "$tool" = read ]; then
		arguments=$(jq -cn --arg p "$(cd "$dir/project" && pwd -P)/notes.txt" '{filePath: $p}')
	fi

	# Created here rather than by the background child's redirect, so the first
	# poll cannot race the child to the file and abort the run under `set -e`.
	: >"$dir/fake.port"
	python3 "$package/fixtures/fake_provider.py" "$dir/sink" "$PROBE_MARKER" "$tool" "$arguments" >"$dir/fake.port" 2>"$dir/fake.stderr" &
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
		"PORT=$port" \
		"FX_SERVER=$ws/fx/fx_server.py" \
		"FX_STAMP=$dir/fx.stamp"

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

run_arm map_read_resource probe-pattern-map read_mcp_resource "$RESOURCE_CALL"
run_arm map_list_resources probe-pattern-map list_mcp_resources '{"server":"fx"}'
run_arm map_list_templates probe-pattern-map list_mcp_resource_templates '{"server":"fx"}'
run_arm map_read_file probe-pattern-map read ''
run_arm plain_read_resource probe-plain-read read_mcp_resource "$RESOURCE_CALL"
run_arm plain_list_resources probe-plain-read list_mcp_resources '{"server":"fx"}'
run_arm plain_list_templates probe-plain-read list_mcp_resource_templates '{"server":"fx"}'

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
	'{map_read_resource: $map_read_resource[0], map_list_resources: $map_list_resources[0], map_list_templates: $map_list_templates[0], map_read_file: $map_read_file[0], plain_read_resource: $plain_read_resource[0], plain_list_resources: $plain_list_resources[0], plain_list_templates: $plain_list_templates[0]}' >"$ws/view.json"

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

# Without a connected server, OpenCode offers no resource tool, and a scripted
# resource call fails as an unknown tool rather than on any permission.
for arm in "${ARMS[@]}"; do
	if [ ! -e "$ws/$arm/fx.stamp" ]; then
		printf 'probe: arm %s: the fx server never answered tools/list, so the arm describes nothing.\n' "$arm" >&2
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
		printf 'probe: either OpenCode never executed the scripted call, or the session stopped after the rejection —\n' >&2
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
