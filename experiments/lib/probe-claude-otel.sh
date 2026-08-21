#!/usr/bin/env bash
# Shared apparatus for the billed Claude probes: arm invocation, view assembly,
# and the three gates.
#
# `claude-agent-effort` drives `claude -p` through this, and `claude-skill-effort`
# is intended to follow: the two differ only in their fixtures, arms, and prompts —
# the same consolidation `probe-common.sh` made for the five manual packages.
# Writing the gates twice would put the safety-critical part of a billed
# apparatus in two files that can drift, which is the failure
# `manifest-contract.sh` exists to prevent, turned inward again. As library
# code the gates also get bats coverage against fabricated views at zero cost;
# duplicated in two runners, the only thing that would ever exercise them is a
# real, paid run.
#
# The contract has two requirements:
#
#   1. The sourcing runner sets `PROBE_CLAUDE_MODEL` and
#      `PROBE_CLAUDE_BUDGET_USD` before the first call. They are globals rather
#      than arguments because every arm in a package shares them, and threading
#      them through each call site would invite one arm being invoked at a
#      different model than its siblings — which would make the ungoverned
#      control set incomparable across arms.
#   2. Every helper signals failure with `return 1` and prints its own
#      diagnostic; none of them exits. The runner owns the exit and the
#      workspace-kept message, so that message is written once rather than
#      duplicated at every call site. This is uniform across all four, so a
#      reader who has checked one call site has checked them all.
#
# Note that `set -e` is in force via `probe-common.sh`, so every call site must
# be wrapped — an unwrapped `return 1` would abort the runner before its
# workspace-kept diagnostic ran.
#
# No gate here names an effort level. The assertion is relational: each arm is
# compared against the requests nothing governed in the same run, so nothing
# depends on the model's default effort or on the operator's subscription tier.
#
# This file is meant to be sourced, not executed.

# shellcheck disable=SC2154 # PROBE_CLAUDE_MODEL and PROBE_CLAUDE_BUDGET_USD are set by the sourcing runner.

# Copy the fixture tree into a per-arm project, then invoke `claude -p` against
# it with the OTEL sink pointed at a per-arm directory. Extra arguments are
# passed through to `claude` after the isolation flags.
probe_claude_arm() {
	local ws="$1" arm="$2" tree="$3" prompt="$4"
	shift 4
	local project="$ws/$arm/project" sink="$ws/$arm/sink"

	# Checked explicitly: the function is always invoked as `fn … || probe_fail`,
	# which suppresses `errexit` for its whole body, so an unchecked failure here
	# would run `claude` against an empty project — a billed call spent to reach
	# a gate-2 failure that this catches for free.
	mkdir -p "$project" "$sink" || {
		printf 'probe: could not create the %s arm workspace under %s\n' "$arm" "$ws" >&2
		return 1
	}
	cp -R "$tree/." "$project/" || {
		printf 'probe: could not copy the fixture tree %s into the %s arm\n' "$tree" "$arm" >&2
		return 1
	}

	# `CLAUDE_CODE_EFFORT_LEVEL` outranks frontmatter, so an exported one would
	# make the probe measure the operator's shell. `--setting-sources project`
	# excludes the user tier outright rather than out-ranking it. No `--effort`
	# is passed; it would outrank frontmatter too.
	(
		cd "$project" &&
			env -u CLAUDE_CODE_EFFORT_LEVEL \
				CLAUDE_CODE_ENABLE_TELEMETRY=1 \
				OTEL_LOG_RAW_API_BODIES="file:$sink" \
				claude -p "$prompt" \
				--setting-sources project \
				--model "$PROBE_CLAUDE_MODEL" \
				--max-budget-usd "$PROBE_CLAUDE_BUDGET_USD" \
				"$@"
	) >"$ws/$arm/stdout" 2>"$ws/$arm/stderr" || {
		printf 'probe: the %s arm exited nonzero; its stderr follows:\n' "$arm" >&2
		cat "$ws/$arm/stderr" >&2
		return 1
	}
}

# Gate 1, plus view assembly. Glob `*.request.json` specifically — the sink also
# writes `<request_id>.response.json`. Files are counted rather than size-checked
# because a view built as `[]` is a non-empty file, and the count is never
# compared against an expected number: a single turn has been observed producing
# one, two, and sixteen requests.
probe_claude_assemble_view() {
	local ws="$1" out="$2"
	shift 2
	local view arm bodies saved_nullglob
	local -a requests

	# Restored rather than unconditionally cleared: this is sourced library
	# code, and `shopt -u` would silently disable a `nullglob` the caller set.
	saved_nullglob=$(shopt -p nullglob)

	view=$(jq -n '{}')
	for arm in "$@"; do
		shopt -s nullglob
		requests=("$ws/$arm/sink"/*.request.json)
		eval "$saved_nullglob"
		if [ "${#requests[@]}" -eq 0 ]; then
			printf 'probe: the %s arm captured no request files in %s\n' "$arm" "$ws/$arm/sink" >&2
			return 1
		fi
		bodies=$(jq -s '.' "${requests[@]}")
		view=$(jq --arg a "$arm" --argjson v "$bodies" '.[$a] = $v' <<<"$view")
	done
	printf '%s\n' "$view" >"$out"
}

# Gate 2: every arm holds at least one request the fixture *governs*. Without
# it, "the fixture never engaged" and "the fixture engaged and Claude discarded
# its effort" are indistinguishable — which would make the most
# decision-relevant finding the least trustworthy one.
#
# "Governs" is the marker appearing in `.system`, not anywhere in the body, and
# the distinction is load-bearing rather than pedantic. A fixture's text becomes
# the system prompt of the request it governs; when a subagent replies, that
# same text comes back to the *main thread* inside a tool result, on a request
# the fixture governs not at all and which sits at the ungoverned level. Matched
# on `tostring`, that echo would satisfy this gate for an arm whose fixture
# never engaged, and would drag an ungoverned level into the arm's own value.
probe_claude_gate_marker() {
	local view="$1" marker="$2"
	jq -e --arg m "$marker" 'all(.[]; any(.[]; .system | tostring | contains($m)))' "$view" >/dev/null && return 0

	printf 'probe: an arm captured no request whose system prompt carries the fixture marker.\n' >&2
	printf 'probe: that arm never engaged the fixture, so its value describes nothing.\n' >&2
	return 1
}

# Gate 3: at least one ungoverned request declares `output_config.effort`, and
# they all agree on one level. "Ungoverned" is the complement of gate 2's
# definition — the marker is not in `.system` — so a request that merely quotes
# the marker back in a tool result counts as ungoverned, which it is: its effort
# was not set by the fixture. The filter is an explicit exclusion of requests
# carrying no `effort` key: Claude emits an intermittent title-generation
# sidecar whose `output_config` holds a `format` object and no `effort`, and a
# control stated over all unmarked requests would fail on it every time it
# appeared. Writing it as an exclusion also keeps the other direction loud — a
# Claude that stopped populating `effort` on governed requests empties the
# control set and trips the existence clause.
probe_claude_gate_control() {
	local view="$1" marker="$2"
	jq -e --arg m "$marker" '
		[ .[][]
		  | select((.system | tostring | contains($m)) | not)
		  | select(.output_config | has("effort"))
		  | .output_config.effort ] as $c
		| ($c | length) > 0 and (($c | unique) | length) == 1
	' "$view" >/dev/null && return 0

	printf 'probe: the ungoverned control set is empty or internally inconsistent.\n' >&2
	printf 'probe: the assertion compares each arm against that set, so it cannot be read.\n' >&2
	printf 'probe: this is a statement about the run, not about Claude. No record written.\n' >&2
	return 1
}
