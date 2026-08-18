#!/usr/bin/env bats
# Every committed record satisfies the contract.
#
# The extra-key half matters as much as the missing-key half: six fields were
# removed during planning, and without a test rejecting them they return one
# record at a time.

setup() {
	EXPERIMENTS="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
	load record_contract
}

@test "every committed record satisfies the seven-key contract" {
	# Deliberately vacuous on an empty glob, which keeps the loop well-defined
	# while a package is being authored. The contract requires every probe
	# package to carry at least one record.
	for record in "$EXPERIMENTS"/*/results/*.json; do
		[ -e "$record" ] || continue
		run assert_record_wellformed "$record"
		if [ "$status" -ne 0 ]; then
			printf '%s\n' "$output" >&2
			return 1
		fi
	done
}

@test "every committed manifest parses and declares its required fields" {
	# Scoped to packages that have a manifest. Every probe package has one; the
	# guard covers a package mid-authoring, before its manifest is written.
	for manifest in "$EXPERIMENTS"/*/probe.json; do
		[ -e "$manifest" ] || continue

		run jq -e . "$manifest"
		if [ "$status" -ne 0 ]; then
			printf 'manifest is not valid JSON: %s\n' "$manifest" >&2
			return 1
		fi

		run jq -e '
			(.question | type == "string" and (. | length) > 0)
			and (.provider | . == "claude" or . == "cursor" or . == "opencode")
			and (.driver | . == "script" or . == "human-act" or . == "human-judge")
			and (.assertion | type == "object" and has("expected")
			     and (has("projection") != has("options")))
		' "$manifest"
		if [ "$status" -ne 0 ]; then
			printf 'manifest missing a required field: %s\n' "$manifest" >&2
			return 1
		fi
	done
}

@test "every human-judge manifest carries exactly one inconclusive option" {
	# Without a "couldn't tell" option a probe forces a binary, and a tired
	# operator picking the first plausible answer manufactures a pass.
	for manifest in "$EXPERIMENTS"/*/probe.json; do
		[ -e "$manifest" ] || continue
		[ "$(jq -r '.driver' "$manifest")" = "human-judge" ] || continue

		run jq -e '[.assertion.options[] | select(.status == "inconclusive")] | length == 1' "$manifest"
		if [ "$status" -ne 0 ]; then
			printf 'human-judge manifest lacks exactly one inconclusive option: %s\n' "$manifest" >&2
			return 1
		fi
	done
}

@test "every human-judge manifest's expected names one of its own option ids" {
	# A typo here would make the probe permanently unpassable, and it would
	# only surface during a live session — the most expensive place to find it.
	#
	# Scoped to human-judge on purpose: a machine-read manifest's `expected` is
	# an arbitrary JSON value with no `options` to name.
	for manifest in "$EXPERIMENTS"/*/probe.json; do
		[ -e "$manifest" ] || continue
		[ "$(jq -r '.driver' "$manifest")" = "human-judge" ] || continue

		run jq -e '.assertion.expected as $e | any(.assertion.options[]; .id == $e)' "$manifest"
		if [ "$status" -ne 0 ]; then
			printf 'human-judge manifest expected names no existing option id: %s\n' "$manifest" >&2
			return 1
		fi
	done
}

@test "every human-driven manifest declares a wait_for filter" {
	# The runner polls against it; without one the run burns its full timeout.
	for manifest in "$EXPERIMENTS"/*/probe.json; do
		[ -e "$manifest" ] || continue
		case "$(jq -r '.driver' "$manifest")" in
		human-act | human-judge) ;;
		*) continue ;;
		esac

		run jq -e '.wait_for | type == "string" and (. | length) > 0' "$manifest"
		if [ "$status" -ne 0 ]; then
			printf 'human-driven manifest declares no wait_for: %s\n' "$manifest" >&2
			return 1
		fi
	done
}

@test "every probe package is a measurement" {
	# The contract's discovery command is `jq -r '.question' experiments/*/probe.json`,
	# and both READMEs call that list complete. Nothing else enforces it:
	# `probe-run` and `probe-status` each pass over a manifest-less directory
	# silently, so an unbacked package would be invisible to all three at once.
	# Every other test here is scoped to packages that have a manifest; this is
	# the one that says there are no others.
	for package in "$EXPERIMENTS"/*/; do
		package="${package%/}"
		[ "$(basename "$package")" = lib ] && continue

		if [ ! -f "$package/probe.json" ]; then
			printf 'package has no manifest, so nothing discovers it: %s\n' "$package" >&2
			return 1
		fi
		if [ ! -x "$package/probe.sh" ]; then
			printf 'package has no executable runner: %s\n' "$package" >&2
			return 1
		fi

		shopt -s nullglob
		records=("$package"/results/*.json)
		shopt -u nullglob
		if [ "${#records[@]}" -eq 0 ]; then
			printf 'package has never recorded a measurement: %s\n' "$package" >&2
			return 1
		fi
	done
}

@test "every package with a manifest has an executable runner" {
	for manifest in "$EXPERIMENTS"/*/probe.json; do
		[ -e "$manifest" ] || continue
		package=$(dirname "$manifest")
		if [ ! -x "$package/probe.sh" ]; then
			printf 'package has a manifest but no executable probe.sh: %s\n' "$package" >&2
			return 1
		fi
	done
}

@test "no committed record carries a field removed during planning" {
	for record in "$EXPERIMENTS"/*/results/*.json; do
		[ -e "$record" ] || continue
		for key in probe kind driver capture finding blocked_reason; do
			if [ "$(jq --arg k "$key" 'has($k)' "$record")" != "false" ]; then
				printf 'record %s carries forbidden key %s\n' "$record" "$key" >&2
				return 1
			fi
		done
	done
}
