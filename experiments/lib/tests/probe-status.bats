#!/usr/bin/env bats
# Coverage for `probe-status.sh`, driven against fixture package trees rather
# than the repository's own records, so the assertions do not shift as real
# probes are added.

setup() {
	STATUS="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)/lib/probe-status.sh"
	TREE="$BATS_TEST_TMPDIR/experiments"
	mkdir -p "$TREE"
	export PROBE_EXPERIMENTS_DIR="$TREE"
}

# Write a record into <package>/results/<stamp>-<provider>-<version>.json.
put_record() {
	local package="$1" stamp="$2" provider="$3" record_status="$4" version="$5"
	mkdir -p "$TREE/$package/results"
	jq -n \
		--arg provider "$provider" --arg s "$record_status" \
		--arg date "${stamp%%T*}" --arg v "$version" \
		'{schema_version: 1, provider: $provider, status: $s, depth: null,
		  date: $date, tool_version: $v,
		  assertion: {projection: ".", expected: 1, observed: 1}}' \
		>"$TREE/$package/results/${stamp}-${provider}-${version}.json"
}

put_manifest() {
	local package="$1"
	mkdir -p "$TREE/$package"
	cat >"$TREE/$package/probe.json"
}

# A manifest whose version command is a stub binary printing <version>.
# The command is exec'd as argv rather than through a shell, so a version
# fixture must be a real executable rather than a quoted `printf`.
put_version_stub() {
	local package="$1" version="$2" driver="${3:-unattended}"
	mkdir -p "$TREE/$package"
	local stub="$TREE/$package/version-stub"
	printf '#!/usr/bin/env bash\nprintf "%%s\\n" "%s"\n' "$version" >"$stub"
	chmod +x "$stub"
	jq -n --arg cmd "$stub" --arg d "$driver" \
		'{schema_version: 1, provider: "opencode", driver: $d, depth: null,
		  question: "q", version_source: {kind: "command", command: $cmd},
		  assertion: {projection: ".", expected: 1}}' \
		>"$TREE/$package/probe.json"
}

# A manifest with no records — the state a billed package spends most of its
# life in, and the one the record loop passes over entirely.
put_billed_manifest() {
	local package="$1"
	mkdir -p "$TREE/$package"
	jq -n '{schema_version: 1, provider: "claude", driver: "billed",
		  depth: "outbound-request", question: "q",
		  version_source: {kind: "none"},
		  assertion: {projection: ".", expected: 1}}' \
		>"$TREE/$package/probe.json"
}

@test "the newest record for a probe is the one reported" {
	put_record alpha 2026-01-01T000000 opencode refuted 1.0.0
	put_record alpha 2026-06-01T000000 opencode confirmed 2.0.0

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"2026-06-01"* ]]
	[[ "$output" != *"2026-01-01"* ]]
	[[ "$output" == *"confirmed"* ]]
	[[ "$output" != *"REFUTED"* ]]
}

@test "the provider set is derived from the records, inventing no rows" {
	put_record alpha 2026-06-01T000000 opencode confirmed 2.0.0

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"OPENCODE"* ]]
	[[ "$output" != *"CURSOR"* ]]
	[[ "$output" != *"CLAUDE"* ]]
}

@test "a probe's reported name comes from its directory" {
	# Two otherwise-identical records under different package names must
	# produce two rows, each under its own name.
	put_record alpha 2026-06-01T000000 opencode confirmed 2.0.0
	put_record beta 2026-06-01T000000 opencode confirmed 2.0.0

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"alpha"* ]]
	[[ "$output" == *"beta"* ]]
	[[ "$output" == *"2 recorded"* ]]
}

@test "an inconclusive record renders visibly distinct from a confirmed one" {
	# Reading "couldn't tell" as a pass would defeat the option's purpose.
	put_record alpha 2026-06-01T000000 cursor confirmed 3.0.0
	put_record beta 2026-06-01T000000 cursor inconclusive 3.0.0

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"INCONCLUSIVE"* ]]
	[[ "$output" == *"1 inconclusive"* ]]
}

@test "a refuted record is surfaced as recorded assertion drift" {
	put_record alpha 2026-06-01T000000 opencode refuted 2.0.0

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"REFUTED"* ]]
	[[ "$output" == *"1 refuted"* ]]
}

@test "a version_source command absent from PATH reports not installed" {
	put_record alpha 2026-06-01T000000 opencode confirmed 2.0.0
	put_manifest alpha <<-'JSON'
		{
		  "schema_version": 1, "provider": "opencode", "driver": "unattended", "depth": null,
		  "question": "q",
		  "version_source": { "kind": "command", "command": "definitely-not-a-real-tool --version" },
		  "assertion": { "projection": ".", "expected": 1 }
		}
	JSON

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"not installed"* ]]
	# An absent tool is not staleness.
	[[ "$output" == *"0 version drift"* ]]
}

@test "a capture-sourced version computes no drift" {
	put_record alpha 2026-06-01T000000 cursor confirmed 3.16.17
	put_manifest alpha <<-'JSON'
		{
		  "schema_version": 1, "provider": "cursor", "driver": "manual", "depth": null,
		  "question": "q",
		  "version_source": { "kind": "capture", "jq": "[.[] | .cursor_version] | first" },
		  "assertion": { "projection": ".", "expected": 1 }
		}
	JSON

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"from capture"* ]]
	[[ "$output" == *"0 version drift"* ]]
}

@test "a version string beginning with a sentinel word still counts as drift" {
	# The display string and the not-comparable signal must not share a
	# namespace, or a real version starting with the sentinel is silently
	# treated as incomparable and genuine staleness goes uncounted.
	put_record alpha 2026-06-01T000000 opencode confirmed 1.0.0
	put_version_stub alpha unknown-build-42

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"installed unknown-build-42"* ]]
	[[ "$output" == *"1 version drift"* ]]
}

@test "a malformed manifest is reported, not silently read as a human check" {
	put_record alpha 2026-06-01T000000 opencode confirmed 1.0.0
	put_manifest alpha <<-'JSON'
		BROKEN {{{ not json
	JSON

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"manifest unreadable"* ]]
	[[ "$output" != *"human check"* ]]
}

@test "a version command reading stdin does not wedge the report" {
	# stdin is closed for the version command, so a command that reads it
	# returns immediately instead of blocking a build nobody is watching.
	put_record alpha 2026-06-01T000000 opencode confirmed 1.0.0
	put_manifest alpha <<-'JSON'
		{
		  "schema_version": 1, "provider": "opencode", "driver": "unattended", "depth": null,
		  "question": "q",
		  "version_source": { "kind": "command", "command": "cat" },
		  "assertion": { "projection": ".", "expected": 1 }
		}
	JSON

	run "$STATUS" --summary
	[ "$status" -eq 0 ]
	[[ "$output" == *"recorded"* ]]
}

@test "an unrelated json file in results/ is not mistaken for the newest record" {
	put_record alpha 2026-06-01T000000 opencode confirmed 2.0.0
	printf '{"note":"scratch"}\n' >"$TREE/alpha/results/notes.json"

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"confirmed"* ]]
	[[ "$output" == *"2.0.0"* ]]
	[[ "$output" == *"1 recorded"* ]]
}

@test "an unknown argument exits 0 rather than failing the build" {
	put_record alpha 2026-06-01T000000 opencode confirmed 2.0.0

	run "$STATUS" --bogus
	[ "$status" -eq 0 ]
}

@test "an installed version differing from the recorded one counts as drift" {
	put_record alpha 2026-06-01T000000 opencode confirmed 1.0.0
	put_version_stub alpha 9.9.9

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"installed 9.9.9"* ]]
	[[ "$output" == *"1 version drift"* ]]
}

@test "a package with no manifest still has its records reported" {
	# Defense-in-depth: the report is derived from records, so it does not
	# depend on a manifest existing. Every probe package has one regardless.
	put_record alpha 2026-06-01T000000 opencode confirmed 2.0.0

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"alpha"* ]]
	[[ "$output" == *"1 recorded"* ]]
}

@test "malformed JSON in a records directory does not break the exit code" {
	# The property `just check` depends on.
	put_record alpha 2026-06-01T000000 opencode confirmed 2.0.0
	mkdir -p "$TREE/broken/results"
	printf 'not json at all {{{\n' >"$TREE/broken/results/2026-06-01T000000-opencode-1.json"

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"alpha"* ]]
	# The broken record is skipped rather than counted.
	[[ "$output" == *"1 recorded"* ]]
}

@test "an empty tree exits 0 and says so" {
	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"No probe records"* ]]
}

@test "--summary emits exactly one line with counts matching the tree" {
	put_record alpha 2026-06-01T000000 opencode confirmed 2.0.0
	put_record beta 2026-06-01T000000 cursor refuted 3.0.0
	put_record gamma 2026-06-01T000000 cursor inconclusive 3.0.0

	run "$STATUS" --summary
	[ "$status" -eq 0 ]
	[ "${#lines[@]}" -eq 1 ]
	[[ "$output" == *"3 recorded"* ]]
	[[ "$output" == *"1 refuted"* ]]
	[[ "$output" == *"1 inconclusive"* ]]
}

@test "--summary exits 0 on an empty tree" {
	run "$STATUS" --summary
	[ "$status" -eq 0 ]
	[ "${#lines[@]}" -eq 1 ]
	[[ "$output" == *"0 recorded"* ]]
}

@test "a billed package's version drift is excluded from the count and named as billed" {
	# A batch run never refreshes a billed package, so its drift is permanent.
	# Counting it would make the total structurally unable to reach zero, which
	# hides the genuine drift of a package a free run could clear.
	put_record alpha 2026-06-01T000000 opencode confirmed 1.0.0
	put_version_stub alpha 9.9.9 billed

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"installed 9.9.9"* ]]
	[[ "$output" == *"just probe-run --live"* ]]
	[[ "$output" == *"0 version drift"* ]]
	[[ "$output" == *"1 billed (drift not tracked)"* ]]
}

@test "a billed package's bucket does not suppress an unattended package's drift" {
	# The failure the separate bucket exists to prevent: a permanent-drift
	# package must not swallow the signal from one a batch run can clear.
	put_record alpha 2026-06-01T000000 opencode confirmed 1.0.0
	put_version_stub alpha 9.9.9 billed
	put_record beta 2026-06-01T000000 opencode confirmed 1.0.0
	put_version_stub beta 8.8.8 unattended

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"1 version drift"* ]]
	[[ "$output" == *"1 billed"* ]]
}

@test "a billed package with no results directory still renders a row" {
	# The record loop skips a package with no results/, which is exactly the
	# state a billed package is most likely to be in — and no batch run will
	# ever move it out of that state.
	put_billed_manifest alpha

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"alpha"* ]]
	[[ "$output" == *"not yet run"* ]]
	[[ "$output" == *"outbound-request"* ]]
	[[ "$output" == *"just probe-run --live"* ]]
	[[ "$output" != *"No probe records"* ]]
	[[ "$output" == *"0 recorded"* ]]
	[[ "$output" == *"1 billed"* ]]
}

@test "a billed package that has recorded is not also given a not-yet-run row" {
	put_record alpha 2026-06-01T000000 opencode confirmed 1.0.0
	put_version_stub alpha 1.0.0 billed

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" != *"not yet run"* ]]
	[[ "$output" == *"1 recorded"* ]]
	[[ "$output" == *"1 billed"* ]]
}

@test "a tree with no billed packages prints the summary line without a billed segment" {
	# The segment is omitted rather than printed as zero, so a tree that has
	# never declared a billed package keeps the line it had before.
	put_record alpha 2026-06-01T000000 opencode confirmed 2.0.0

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" == *"1 recorded"* ]]
	[[ "$output" != *"billed"* ]]
}

@test "--summary works while a billed package is present" {
	# The billed manifest scan sets `billed`, which `summary_line` reads at the
	# `--summary` early exit. Under `set -u` a refactor hoisting that exit above
	# the scan would fail on an unset variable, and the four tests above all
	# take the full-report path — so nothing else pins the ordering.
	put_billed_manifest alpha

	run "$STATUS" --summary
	[ "$status" -eq 0 ]
	[ "${#lines[@]}" -eq 1 ]
	[[ "$output" == *"1 billed (drift not tracked)"* ]]
}

@test "a billed package holding an unreadable record is not labelled not yet run" {
	# It has been run. Claiming otherwise would be a false statement about the
	# package, rather than the invisibility an unparseable record earns every
	# other package in the report.
	put_billed_manifest alpha
	mkdir -p "$TREE/alpha/results"
	printf 'not json at all {{{\n' >"$TREE/alpha/results/2026-06-01T000000-claude-1.json"

	run "$STATUS"
	[ "$status" -eq 0 ]
	[[ "$output" != *"not yet run"* ]]
	[[ "$output" == *"1 billed"* ]]
}
