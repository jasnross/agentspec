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

@test "an unattended package runs" {
	put_package alpha unattended

	run "$RUN"
	[ "$status" -eq 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" == *"1 ran"* ]]
}

@test "manual packages are listed as skipped with a README pointer" {
	put_package alpha manual
	put_package beta manual

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

@test "a billed package is skipped by default, naming the reason" {
	# The default has to be skip: a batch run that costs money on every
	# invocation is a batch run people stop invoking.
	put_package alpha billed

	run "$RUN"
	[ "$status" -eq 0 ]
	[[ "$output" == *"driver billed"* ]]
	[[ "$output" == *"run with --live"* ]]
	[[ "$output" == *"1 skipped"* ]]
	[[ "$output" == *"0 manual"* ]]
	[[ "$output" == *"1 billed"* ]]
	# The money-spending part must not have been invoked.
	[[ "$output" != *"ran alpha"* ]]
}

@test "a billed package runs under --live" {
	put_package alpha billed

	run "$RUN" --live
	[ "$status" -eq 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" == *"1 ran"* ]]
	[[ "$output" == *"0 billed"* ]]
}

@test "a billed package runs under PROBE_ALLOW_LIVE=1" {
	# The environment form exists so a caller that cannot pass arguments —
	# anything invoking the script rather than the recipe — can still opt in.
	put_package alpha billed

	PROBE_ALLOW_LIVE=1 run "$RUN"
	[ "$status" -eq 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" == *"1 ran"* ]]
}

@test "the summary breakdown reports manual and billed skips separately" {
	# A single count would let the parenthetical claim every skip had one
	# cause, which is what the old `(human-driven)` label did.
	put_package alpha manual
	put_package beta billed
	put_package gamma billed

	run "$RUN"
	[ "$status" -eq 0 ]
	[[ "$output" == *"0 ran"* ]]
	[[ "$output" == *"3 skipped (1 manual · 2 billed)"* ]]
}

@test "an unknown argument exits 2 without running any package" {
	put_package alpha unattended

	run "$RUN" --bogus
	[ "$status" -eq 2 ]
	[[ "$output" == *"unknown argument"* ]]
	[[ "$output" != *"ran alpha"* ]]
}

@test "a package with no manifest is passed over silently" {
	# Defense-in-depth, not a sanctioned state: the contract says every probe
	# package has a manifest. This pins that a half-authored one cannot break a
	# batch run.
	mkdir -p "$TREE/fixtures-only"
	printf 'A directory mid-authoring: a README, no manifest yet.\n' >"$TREE/fixtures-only/README.md"
	put_package alpha unattended

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
	put_package alpha unattended 1
	put_package beta unattended 0

	run "$RUN"
	[ "$status" -ne 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" == *"ran beta"* ]]
	[[ "$output" == *"alpha FAILED"* ]]
	[[ "$output" == *"failed: alpha"* ]]
	[[ "$output" == *"1 ran"* ]]
}

@test "an unattended package with no executable runner is reported as failed" {
	put_package alpha unattended
	chmod -x "$TREE/alpha/probe.sh"

	run "$RUN"
	[ "$status" -ne 0 ]
	[[ "$output" == *"no executable probe.sh"* ]]
}

@test "an unknown driver is reported as failed rather than silently skipped" {
	# `script` is a retired value, so it exercises the arm a stale manifest
	# would land in rather than a value nobody ever wrote.
	put_package alpha script

	run "$RUN"
	[ "$status" -ne 0 ]
	[[ "$output" == *"unknown driver"* ]]
	[[ "$output" != *"ran alpha"* ]]
}

@test "an empty tree exits 0" {
	run "$RUN"
	[ "$status" -eq 0 ]
	[[ "$output" == *"0 ran"* ]]
}
