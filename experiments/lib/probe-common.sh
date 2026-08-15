#!/usr/bin/env bash
# Shared *Arrange* helpers for provider probes: workspace creation, run stamps,
# capture polling, and operator prompting.
#
# This file is meant to be sourced, not executed. Sourcing therefore sets
# `errexit`, `nounset`, and `pipefail` in the caller's shell. That is harmless
# here — `record.sh` and every `probe.sh` set them anyway — but it is surprising
# enough to state so nobody reads it as a bug.
set -euo pipefail

# Create a throwaway workspace for a probe run and print its path on stdout.
# The workspace always carries a `capture/` directory and a run stamp, so the
# `--capture` fallback path works for every probe whether or not it needs one.
probe_workspace_create() {
	local probe_name="$1"
	local ws
	ws=$(mktemp -d "${TMPDIR:-/tmp}/probe-${probe_name}.XXXXXX")
	mkdir -p "$ws/capture"
	probe_run_stamp_write "$ws"
	printf '%s\n' "$ws"
}

# Write `<token> <epoch-seconds>` to <workspace>/capture/.run_stamp.
#
# The token is opaque and per-invocation: a capture script templates it into
# every payload it appends, which is how `record.sh` proves a capture came from
# the invocation that is recording it rather than from a workspace left over
# from a previous run.
probe_run_stamp_write() {
	local ws="$1"
	local token
	token=$(od -An -tx1 -N8 /dev/urandom | tr -d ' \n')
	printf '%s %s\n' "$token" "$(date +%s)" >"$ws/capture/.run_stamp"
}

probe_run_stamp_token() {
	cut -d' ' -f1 <"$1/capture/.run_stamp"
}

probe_run_stamp_epoch() {
	cut -d' ' -f2 <"$1/capture/.run_stamp"
}

# Exit 1 naming every missing tool, so a contributor without `jq` gets one clear
# error instead of a cryptic failure partway through a probe.
probe_require_tools() {
	local missing="" tool
	for tool in "$@"; do
		if ! command -v "$tool" >/dev/null 2>&1; then
			missing="${missing:+$missing }$tool"
		fi
	done
	if [ -n "$missing" ]; then
		printf 'probe: missing required tool(s): %s\n' "$missing" >&2
		exit 1
	fi
}

# Poll <workspace>/capture/payloads.jsonl until <jq-filter> succeeds against it.
# Returns 0 on a match and 1 on timeout, recording nothing either way.
#
# The filter is evaluated with `jq -s`, so every jq expression in the probe
# contract — `wait_for` filters, `version_source.jq`, and projections over a
# capture — sees the same array-shaped input. Without `-s` an `any(.[]; …)`
# filter never matches and the runner polls to timeout on a capture that
# already holds the answer.
probe_wait_for_capture() {
	local ws="$1" filter="$2"
	local timeout="${PROBE_TIMEOUT_SECONDS:-${3:-900}}"
	local payloads="$ws/capture/payloads.jsonl"
	local interval=2 waited=0 last_progress=0

	while [ "$waited" -lt "$timeout" ]; do
		if probe_capture_matches "$payloads" "$filter"; then
			return 0
		fi
		sleep "$interval"
		waited=$((waited + interval))
		if [ $((waited - last_progress)) -ge 10 ]; then
			printf 'probe: waiting for capture (%ss elapsed, timeout %ss)\n' "$waited" "$timeout" >&2
			last_progress=$waited
		fi
	done

	# One final check past the deadline: a capture that landed during the last
	# sleep is a match, not a timeout.
	probe_capture_matches "$payloads" "$filter"
}

probe_capture_matches() {
	local payloads="$1" filter="$2"
	[ -s "$payloads" ] || return 1
	jq -s -e "$filter" "$payloads" >/dev/null 2>&1
}

# Present a pre-declared option set and print the selected id on stdout.
#
# Human-judged probes have no machine-readable oracle, so the answer is a
# selection from options the probe author declared in advance. Re-prompts on an
# unrecognized id; the caller passes the ids straight to `record.sh --selection`.
probe_prompt_selection() {
	local options="$1" question="${2:-}"

	if [ ! -t 0 ]; then
		# Without this guard a human-judged probe swept up by `probe-run` or a
		# CI job would block forever on a read that can never be answered.
		printf 'probe: this probe requires a human selection, but stdin is not a terminal.\n' >&2
		if [ -n "$question" ]; then printf 'probe: %s\n' "$question" >&2; fi
		probe_render_options "$options" >&2
		printf 'probe: run this probe directly from an interactive terminal.\n' >&2
		exit 2
	fi

	probe_read_selection "$options" "$question"
}

# The prompt loop, split out from the tty guard above so it is reachable with a
# non-tty stdin. `probe_prompt_selection` is the only production caller; the
# split exists because the guard would otherwise make this loop untestable.
probe_read_selection() {
	local options="$1" question="${2:-}" answer

	while :; do
		printf '\n' >&2
		if [ -n "$question" ]; then printf '%s\n\n' "$question" >&2; fi
		probe_render_options "$options" >&2
		printf '\nSelection (id): ' >&2
		read -r answer || exit 2
		if jq -e --arg a "$answer" 'any(.[]; .id == $a)' <<<"$options" >/dev/null; then
			printf '%s\n' "$answer"
			return 0
		fi
		printf 'probe: "%s" is not one of the option ids.\n' "$answer" >&2
	done
}

probe_render_options() {
	jq -r '.[] | "  \(.id)  —  \(.text)"' <<<"$1"
}
