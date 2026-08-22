#!/usr/bin/env bash
# Derive a package's freshness from its manifest and its committed records.
# Sourced, never executed.
#
# This is the data-collection half of what `probe-status.sh` used to hold
# inline. It moved here when `probe-run.sh --stale` needed the same answer:
# a report that renders staleness and a runner that filters on it must agree,
# and the only way to guarantee that is to have one implementation.
#
# Nothing here runs a probe, and nothing here prints a report. Callers own
# their own `jq` preflight, because they disagree about what its absence
# means: the report degrades to a one-liner and exits 0, the runner fails.
#
# `installed_display`, `installed_comparable` and `probe_state_reason` are
# out-parameters: set here, read by the sourcing script. The disable below is
# file-scoped rather than pinned to one assignment, because which assignment
# gets flagged shifts whenever the branches are reordered.
# shellcheck disable=SC2034

# Bound a version command so a wedged one cannot hang `just check` forever.
# A wedged build is worse than a failed one, which is the property this exists
# to guarantee. `timeout` is GNU; macOS carries it only as `gtimeout` via
# coreutils, so its absence degrades to running unbounded rather than to
# skipping the command.
PROBE_VERSION_COMMAND_TIMEOUT=10
if command -v timeout >/dev/null 2>&1; then
	probe_timeout_prefix="timeout $PROBE_VERSION_COMMAND_TIMEOUT"
elif command -v gtimeout >/dev/null 2>&1; then
	probe_timeout_prefix="gtimeout $PROBE_VERSION_COMMAND_TIMEOUT"
else
	probe_timeout_prefix=""
fi

# The newest record in <package>, or empty when there is none.
#
# The filename carries a UTC date and time, so the shell's lexical glob order
# is run order and the last match is the newest. The glob is constrained to the
# stamped shape so an unrelated .json dropped into results/ cannot sort after
# every record and be taken for the newest.
probe_newest_record() {
	local newest="" candidate
	for candidate in "$1"/results/[0-9]*T[0-9]*-*.json; do
		[ -e "$candidate" ] || continue
		newest="$candidate"
	done
	printf '%s' "$newest"
}

# Resolve the installed version declared by <manifest> into two values:
#
#   installed_display     what to show, or empty for "say nothing"
#   installed_comparable  1 when it can be compared against the recorded
#                         version, 0 when no comparison is possible
#
# The flag is separate from the string on purpose. Sniffing the display text
# for a sentinel means a real version that happens to start with the sentinel
# word is silently treated as incomparable, under-counting genuine staleness.
probe_resolve_installed_version() {
	local manifest="$1"
	installed_display=""
	installed_comparable=0

	[ -f "$manifest" ] || return 0

	local kind
	if ! kind=$(jq -er '.version_source.kind // "none"' "$manifest" 2>/dev/null); then
		# A broken manifest must not be indistinguishable from one that
		# declares no version source.
		installed_display="manifest unreadable"
		return 0
	fi

	case "$kind" in
	command)
		local command_line resolved
		local -a argv
		command_line=$(jq -r '.version_source.command // ""' "$manifest" 2>/dev/null)
		if [ -z "$command_line" ]; then
			installed_display="manifest declares no command"
			return 0
		fi
		# Split into an argv array and exec directly — never through a shell.
		# `just check` runs this on every invocation, and a manifest is a data
		# file reviewed as data; `eval` here would make it arbitrary code.
		# The cost is that a version command cannot use shell quoting or
		# metacharacters, which no real one needs.
		read -r -a argv <<<"$command_line"
		if ! command -v "${argv[0]}" >/dev/null 2>&1; then
			installed_display="not installed"
			return 0
		fi
		# stdin is closed: a command that reads it would otherwise block the
		# report on a terminal nobody is watching.
		# shellcheck disable=SC2086 # probe_timeout_prefix is a deliberate word-split command prefix
		resolved=$($probe_timeout_prefix "${argv[@]}" </dev/null 2>/dev/null | sed -n '1p' || true)
		if [ -z "$resolved" ]; then
			installed_display="version command produced nothing"
			return 0
		fi
		installed_display="$resolved"
		installed_comparable=1
		;;
	capture)
		# The common Cursor case: the IDE's version arrives only inside a
		# payload, so it is knowable by running the probe and by no other
		# means. A caller that runs no probe has nothing to compare.
		installed_display="from capture"
		;;
	none)
		installed_display="human check"
		;;
	*)
		installed_display="unrecognized version_source kind"
		;;
	esac
}

# Decide whether <package> is owed a run. Returns 0 for owed, 1 for fresh, and
# sets `probe_state_reason` to the phrase a caller can print either way.
#
# Owed means one of exactly two things: the package has never produced a
# readable record, or its recorded version is comparable against the installed
# one and differs. Everything else is fresh — including a version that cannot
# be compared at all.
#
# That last case is the load-bearing one. A `capture`-sourced version is
# unknowable without running the probe, and a `none`-sourced one is unknowable
# without a human. Treating "cannot tell" as owed would put every Cursor
# package permanently in the run set, which is the same as having no filter.
# The report still shows those packages, annotated with why no comparison was
# possible; this function only decides what a batch run reaches for.
#
# The driver is deliberately not consulted. Staleness is a property of the
# package's records; whether a stale package may actually be executed is the
# caller's gate, and fusing the two here would leave a caller unable to reach a
# drifted package whose cost it had separately authorized.
probe_needs_run() {
	local package="$1"
	probe_state_reason=""

	local record
	record=$(probe_newest_record "$package")
	if [ -z "$record" ]; then
		probe_state_reason="no record"
		return 0
	fi
	if ! jq -e . "$record" >/dev/null 2>&1; then
		probe_state_reason="newest record unreadable"
		return 0
	fi

	local recorded_version
	recorded_version=$(jq -r 'if .tool_version == null then "—" else .tool_version end' "$record")

	probe_resolve_installed_version "$package/probe.json"
	if [ "$installed_comparable" -eq 1 ]; then
		if [ "$installed_display" != "$recorded_version" ]; then
			probe_state_reason="recorded $recorded_version · installed $installed_display"
			return 0
		fi
		probe_state_reason="version $recorded_version unchanged"
		return 1
	fi

	probe_state_reason="recorded $recorded_version · ${installed_display:-no version source}"
	return 1
}
