#!/usr/bin/env bash
# Shared *Arrange* helpers for provider probes: workspace creation, capture
# polling, and operator prompting.
#
# This file is meant to be sourced, not executed. Sourcing therefore sets
# `errexit`, `nounset`, and `pipefail` in the caller's shell. That is harmless
# here — `record.sh` and every `probe.sh` set them anyway — but it is surprising
# enough to state so nobody reads it as a bug.
set -euo pipefail

# This file's own directory, so helpers can reach `record.sh` without every
# caller passing a path.
PROBE_LIB_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

# How long a human-driven probe waits for its capture, in seconds. One
# constant rather than a default repeated at each layer, where a change to
# one copy would silently not take effect. `PROBE_TIMEOUT_SECONDS` overrides
# it; the bats suite drives that to one second so the suite cannot hang.
#
# It is generous because its job is diagnostic, not resource protection: an
# expiry means the hook never fired, which is worth an hour of an operator's
# patience to distinguish from a slow session. Nothing depends on it being
# short, and with no resume path a premature expiry costs the live session
# outright.
PROBE_DEFAULT_TIMEOUT=3600

# Create a throwaway workspace for a probe run and print its path on stdout.
# The workspace always carries a `capture/` directory, which is where the hook
# script and its payloads live.
probe_workspace_create() {
	local probe_name="$1"
	local ws
	ws=$(mktemp -d "${TMPDIR:-/tmp}/probe-${probe_name}.XXXXXX")
	mkdir -p "$ws/capture"
	printf '%s\n' "$ws"
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
	local timeout="${PROBE_TIMEOUT_SECONDS:-${3:-$PROBE_DEFAULT_TIMEOUT}}"
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

# Copy <src> to <dst>, substituting {{KEY}} placeholders from key=value pairs.
#
# This is what removes the hand-edited absolute path from a probe's setup: the
# runner knows its own temp workspace, so a generated hooks.json gets the real
# path filled in. An operator asked to substitute a placeholder path by hand
# eventually forgets, and the result is a hook that silently never fires.
#
# Fails rather than emitting an unsubstituted placeholder — a leftover {{KEY}}
# is exactly that silent failure.
probe_template_file() {
	local src="$1" dst="$2"
	shift 2

	[ -f "$src" ] || {
		printf 'probe: template source not found: %s\n' "$src" >&2
		exit 1
	}

	# `$(cat)` strips every trailing newline and the write below restores exactly
	# one. Fine for the config templates this handles; worth knowing before
	# pointing it at a file whose trailing whitespace matters.
	local content pair key value
	content=$(cat "$src")
	for pair in "$@"; do
		key=${pair%%=*}
		value=${pair#*=}

		# The key is interpolated into a glob pattern, so a metacharacter in it
		# would match far more than intended, and an empty key would substitute
		# every `{{}}`.
		case "$key" in
		'' | *[!A-Za-z0-9_]*)
			printf 'probe: invalid template key %s (want [A-Za-z0-9_]+)\n' "${key:-<empty>}" >&2
			exit 1
			;;
		esac

		# Deliberately not `${content//pat/$value}`. bash >= 5.2 expands `&` in
		# the replacement to the matched text while 3.2 does not, and macOS
		# ships both — so a path containing `&` would substitute correctly on
		# one machine and reinsert the placeholder on another. Escaping `&`
		# just inverts which shell breaks. Splitting on the literal placeholder
		# has the same meaning everywhere.
		local head tail out=""
		tail="$content"
		while [ -n "$tail" ]; do
			case "$tail" in
			*"{{$key}}"*)
				head=${tail%%"{{$key}}"*}
				out="${out}${head}${value}"
				tail=${tail#*"{{$key}}"}
				;;
			*)
				out="${out}${tail}"
				tail=""
				;;
			esac
		done
		content="$out"
	done

	if printf '%s' "$content" | grep -q '{{'; then
		printf 'probe: unsubstituted placeholder left in %s: %s\n' \
			"$dst" "$(printf '%s' "$content" | grep -o '{{[^}]*}}' | sort -u | tr '\n' ' ')" >&2
		exit 1
	fi

	mkdir -p "$(dirname "$dst")"
	printf '%s\n' "$content" >"$dst"
}

# --- The human-driven runner -------------------------------------------------
#
# Five packages drive a provider by hand and differ only in their fixtures, the
# instructions they print, and whether the answer is read from the capture or
# from a person. That shared shape lives here rather than being written out
# five times.

# Materialize a human-driven probe's workspace and print the path the operator
# opens the provider on.
#
# The layout separates the two halves deliberately:
#
#   <ws>/project/   provider config only — this is what the operator opens
#   <ws>/capture/   the hook script and its payloads
#
# The capture directory sits *outside* the opened project because a probe whose
# oracle is the agent's answer can otherwise be defeated by the agent reading
# the apparatus. A marker string in a capture script inside the project is
# findable with a filesystem search, which makes "the hook injected it" and
# "the agent grepped for it" indistinguishable — a control-arm failure that
# cost two live sessions before it was noticed.
#
# Nothing is written outside <ws>, which is what makes a collision with a real
# `agentspec sync` target structurally impossible rather than merely warned
# against.
probe_arrange_human_workspace() {
	local package="$1" name="$2"
	local ws project capture_script src rel dst

	ws=$(probe_workspace_create "$name")
	project="$ws/project"
	mkdir -p "$project"

	capture_script="$ws/capture/dump-hook.sh"
	probe_template_file "$package/fixtures/capture/dump-hook.sh" "$capture_script" \
		"JQ=$(command -v jq)" \
		"PAYLOADS=$ws/capture/payloads.jsonl"
	chmod +x "$capture_script"

	# `-type f` deliberately: a fixture tree is repo-controlled and flat enough
	# that a symlink would be a mistake rather than a feature. A path containing
	# a newline would mis-split, which the same constraint rules out.
	while IFS= read -r src; do
		rel=${src#"$package/fixtures/"}
		case "$rel" in
		capture/*) continue ;;
		esac
		dst="$project/$rel"
		probe_template_file "$src" "$dst" "CAPTURE_SCRIPT=$capture_script"

		# `probe_template_file` writes with `printf >`, so the mode is always
		# 0644 regardless of the source. Carry the executable bit across, or a
		# fixture the provider is meant to run becomes a hook that silently
		# never fires — the failure this whole helper exists to prevent.
		if [ -x "$src" ]; then chmod +x "$dst"; fi

		# A workspace path carrying a quote or backslash would yield config the
		# provider silently ignores. Fail here, where the cause is obvious,
		# rather than after a live session produced an empty capture.
		case "$rel" in
		*.json)
			jq -e . "$dst" >/dev/null 2>&1 || {
				printf 'probe: generated %s is not valid JSON — check the workspace path\n' "$dst" >&2
				exit 1
			}
			;;
		esac
	done < <(find "$package/fixtures" -type f)

	# The project path is what the operator opens; the workspace root is what
	# `--capture` takes, since that is where capture/ lives.
	printf '%s\n' "$ws"
}

# Poll the capture against the manifest's `wait_for` filter.
probe_wait_for_manifest() {
	local package="$1" ws="$2" timeout="${3:-$PROBE_DEFAULT_TIMEOUT}"
	local wait_for
	wait_for=$(jq -r '.wait_for // ""' "$package/probe.json")
	# A missing filter would poll the literal string "null", which never
	# matches — the run would burn its whole timeout for no reason.
	[ -n "$wait_for" ] || {
		printf 'probe: manifest declares no wait_for filter\n' >&2
		exit 1
	}
	probe_wait_for_capture "$ws" "$wait_for" "$timeout"
}

# Record from a completed capture, projecting or prompting per the manifest's
# driver. Refusing to record on an empty capture is the correct outcome: the
# failure stays loud rather than becoming a record nobody can trust.
probe_record_capture() {
	local package="$1" ws="$2"
	local payloads="$ws/capture/payloads.jsonl"

	if [ ! -d "$ws/capture" ]; then
		printf 'probe: %s is not a probe workspace (no capture/ directory)\n' "$ws" >&2
		exit 1
	fi
	if [ ! -s "$payloads" ]; then
		printf 'probe: no captured payloads at %s\n' "$payloads" >&2
		printf 'probe: the hook did not fire. Check that the provider was fully quit and reopened.\n' >&2
		exit 1
	fi

	local driver
	driver=$(jq -r '.driver // ""' "$package/probe.json")

	if [ "$driver" = "human-judge" ]; then
		# The machine has already proved the apparatus worked — polling
		# confirmed the hook fired — so the human answers only the question no
		# machine can.
		local options question selection
		options=$(jq -c '.assertion.options' "$package/probe.json")
		question=$(jq -r '.question' "$package/probe.json")
		selection=$(probe_prompt_selection "$options" "$question")
		"$PROBE_LIB_DIR/record.sh" \
			--manifest "$package/probe.json" \
			--selection "$selection" \
			--capture "$ws"
	else
		jq -s . "$payloads" >"$ws/view.json"
		"$PROBE_LIB_DIR/record.sh" \
			--manifest "$package/probe.json" \
			--view "$ws/view.json" \
			--capture "$ws"
	fi
}

# The whole human-driven flow as one blocking invocation: arrange, print the
# procedure, poll, record.
#
# Blocking is what deletes the stale-capture failure a two-invocation design
# has to guard against — a process that never exits has no last week, because
# the workspace was created by the invocation still running.
probe_human_run() {
	local package="$1" name="$2" instructions="$3" timeout="${4:-$PROBE_DEFAULT_TIMEOUT}"
	local ws

	ws=$(probe_arrange_human_workspace "$package" "$name")

	printf '\nWorkspace ready: %s\n' "$ws/project" >&2
	printf '(capture lives outside it, at %s)\n%s\n' "$ws/capture" "$instructions" >&2

	if probe_wait_for_manifest "$package" "$ws" "$timeout"; then
		probe_record_capture "$package" "$ws"
		return 0
	fi

	# The run is over: a runner is one blocking invocation with no resume, so
	# re-running the probe is the only way forward. The workspace is still never
	# deleted — discarding one the operator spent a live session on is the
	# single unrecoverable mistake a runner could make, and reading the capture
	# is how they tell a hook that never fired from a procedure that went off
	# the rails.
	printf '\nprobe: timed out waiting for the capture.\n' >&2
	printf 'This run is over; re-run the probe to try again.\n' >&2
	printf 'The workspace has been kept for inspection: %s\n\n' "$ws" >&2
	return 1
}
