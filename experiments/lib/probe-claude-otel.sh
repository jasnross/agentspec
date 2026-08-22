#!/usr/bin/env bash
# Shared apparatus for the billed Claude probes: arm invocation, view assembly,
# and the three gates.
#
# `claude-agent-effort` and `claude-skill-effort` both drive `claude -p` through
# this; they differ only in their fixtures, arms, prompts, and which field the
# fixture governs (see gate 2) — the same consolidation `probe-common.sh` made
# for the five manual packages.
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
# "Governs" is the marker appearing in one named field, never anywhere in the
# body, and the distinction is load-bearing rather than pedantic. When a
# subagent replies, the fixture's text comes back to the *main thread* inside a
# tool result, on a request the fixture governs not at all and which sits at the
# ungoverned level. Matched on a bare `tostring`, that echo would satisfy this
# gate for an arm whose fixture never engaged, and would drag an ungoverned
# level into the arm's own value.
#
# *Which* field carries the fixture is the caller's to name, because it differs
# by what the fixture is rather than by anything this library knows. An agent
# file becomes the system prompt of the request it governs, so `.system` — the
# default — is right for `claude-agent-effort`. A skill's body never reaches
# `.system` at all: measured at 2.1.232, it arrives in `messages[]` (a
# `tool_result` block on the model-invoked path, `messages[0]` text on the
# session-entry and forked paths), so `claude-skill-effort` passes `.messages`.
# Widening the default to cover both would reintroduce the echo this gate exists
# to exclude, and inlining each definition in its own runner would put the
# safety-critical part of a billed apparatus in two files that can drift. A
# parameter keeps one implementation and one set of bats tests.
# `[$field]` rather than a bare `$field`: the collapse costs nothing on a
# single-output expression and stops a multi-output one from re-emitting its
# request once per output. Unchecked, that is not a loud failure — gate 2 still
# passes while gate 3's `select(… | not)` admits a *governed* request whose other
# outputs lack the marker, contaminating the control set with the very arm the
# projection is about to compare against it. Measured on a two-request view with
# `.messages[].content`: the control came out `["low","med","med"]` instead of
# `["med"]`. Both live callers name single-output fields today, so this guards a
# future one.
probe_claude_gate_marker() {
	local view="$1" marker="$2" field="${3:-.system}"
	jq -e --arg m "$marker" "all(.[]; any(.[]; [$field] | tostring | contains(\$m)))" "$view" >/dev/null && return 0

	printf 'probe: an arm captured no request whose %s carries the fixture marker.\n' "$field" >&2
	printf 'probe: that arm never engaged the fixture, so its value describes nothing.\n' >&2
	return 1
}

# Gate 3: at least one ungoverned request declares `output_config.effort`, and
# they all agree on one level. "Ungoverned" is the complement of gate 2's
# definition — the marker is not in the named field — so on the `.system`
# default a request that merely quotes the marker back in a tool result counts
# as ungoverned, which it is: its effort was not set by the fixture. Pass the
# same field here as to gate 2; the two definitions partition the requests
# between them, and disagreeing lets a request fall into *both* sets — which
# puts a governed request into the control the projection then compares it
# against, the direction that contaminates rather than merely under-counts. The
# filter is an explicit exclusion of requests
# carrying no `effort` key: Claude emits an intermittent title-generation
# sidecar whose `output_config` holds a `format` object and no `effort`, and a
# control stated over all unmarked requests would fail on it every time it
# appeared. Writing it as an exclusion also keeps the other direction loud — a
# Claude that stopped populating `effort` on governed requests empties the
# control set and trips the existence clause.
probe_claude_gate_control() {
	local view="$1" marker="$2" field="${3:-.system}"
	jq -e --arg m "$marker" "
		[ .[][]
		  | select(([$field] | tostring | contains(\$m)) | not)
		  | select(.output_config | has(\"effort\"))
		  | .output_config.effort ] as \$c
		| (\$c | length) > 0 and ((\$c | unique) | length) == 1
	" "$view" >/dev/null && return 0

	printf 'probe: the ungoverned control set is empty or internally inconsistent.\n' >&2
	printf 'probe: the assertion compares each arm against that set, so it cannot be read.\n' >&2
	printf 'probe: this is a statement about the run, not about Claude. No record written.\n' >&2
	return 1
}
