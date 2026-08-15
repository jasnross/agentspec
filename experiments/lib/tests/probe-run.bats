#!/usr/bin/env bats
# Coverage for `probe-run.sh`'s loop, driven with stub `probe.sh` files rather
# than real providers. The Testing Strategy exempts per-probe scripts from unit
# tests; this loop is the exception, because it is the only branching logic the
# batch path adds.

setup() {
	RUN="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)/lib/probe-run.sh"
	TREE="$BATS_TEST_TMPDIR/experiments"
	mkdir -p "$TREE"
	export PROBE_EXPERIMENTS_DIR="$TREE"
}

# A package with a manifest declaring <driver> and a stub runner exiting <code>.
put_package() {
	local name="$1" driver="$2" exit_code="${3:-0}"
	mkdir -p "$TREE/$name"
	jq -n --arg d "$driver" \
		'{schema_version: 1, provider: "opencode", driver: $d, depth: null,
		  question: "q", version_source: {kind: "none"},
		  assertion: {projection: ".", expected: 1}}' \
		>"$TREE/$name/probe.json"
	printf '#!/usr/bin/env bash\necho "ran %s"\nexit %s\n' "$name" "$exit_code" \
		>"$TREE/$name/probe.sh"
	chmod +x "$TREE/$name/probe.sh"
}

@test "a script package runs" {
	put_package alpha script

	run "$RUN"
	[ "$status" -eq 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" == *"1 ran"* ]]
}

@test "human-act and human-judge packages are listed as skipped with a README pointer" {
	put_package alpha human-act
	put_package beta human-judge

	run "$RUN"
	[ "$status" -eq 0 ]
	[[ "$output" == *"skipped"* ]]
	[[ "$output" == *"alpha/README.md"* ]]
	[[ "$output" == *"beta/README.md"* ]]
	[[ "$output" == *"2 skipped"* ]]
	# Their runners must not have been invoked.
	[[ "$output" != *"ran alpha"* ]]
	[[ "$output" != *"ran beta"* ]]
}

@test "a package with no manifest is passed over silently" {
	mkdir -p "$TREE/fixtures-only"
	printf 'A blocked gate: fixtures and a README, no manifest.\n' >"$TREE/fixtures-only/README.md"
	put_package alpha script

	run "$RUN"
	[ "$status" -eq 0 ]
	[[ "$output" != *"fixtures-only"* ]]
	[[ "$output" == *"1 ran"* ]]
	[[ "$output" == *"0 skipped"* ]]
}

@test "a failing probe does not abort the loop" {
	# The partial-failure contract: records written by probes that passed stay
	# valid, and the run names what broke. The phase success criterion expects
	# success, so it never exercises this.
	put_package alpha script 1
	put_package beta script 0

	run "$RUN"
	[ "$status" -ne 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" == *"ran beta"* ]]
	[[ "$output" == *"alpha FAILED"* ]]
	[[ "$output" == *"failed: alpha"* ]]
	[[ "$output" == *"1 ran"* ]]
}

@test "a script package with no executable runner is reported as failed" {
	put_package alpha script
	chmod -x "$TREE/alpha/probe.sh"

	run "$RUN"
	[ "$status" -ne 0 ]
	[[ "$output" == *"no executable probe.sh"* ]]
}

@test "an unknown driver is reported as failed rather than silently skipped" {
	put_package alpha bogus-driver

	run "$RUN"
	[ "$status" -ne 0 ]
	[[ "$output" == *"unknown driver"* ]]
}

@test "an empty tree exits 0" {
	run "$RUN"
	[ "$status" -eq 0 ]
	[[ "$output" == *"0 ran"* ]]
}
