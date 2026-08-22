#!/usr/bin/env bats
# Coverage for `probe-run.sh`'s loop, driven with stub `probe.sh` files rather
# than real providers. The Testing Strategy exempts per-probe scripts from unit
# tests; this loop is the exception, because it is the only branching logic the
# batch path adds.
#
# The two axes the loop composes — selection (`--stale`) and authorization
# (`--billed`, `--manual`, `--all`) — are exercised independently and together,
# because their independence is the property the design rests on.

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

# Give an existing package a `command` version source resolving to <installed>,
# via a stub binary — the command is exec'd as argv rather than through a
# shell, so it has to be a real executable.
put_version_stub() {
	local name="$1" installed="$2"
	local stub="$TREE/$name/version-stub"
	printf '#!/usr/bin/env bash\nprintf "%%s\\n" "%s"\n' "$installed" >"$stub"
	chmod +x "$stub"
	local manifest="$TREE/$name/probe.json"
	jq --arg cmd "$stub" '.version_source = {kind: "command", command: $cmd}' \
		"$manifest" >"$manifest.tmp" && mv "$manifest.tmp" "$manifest"
}

# A record pinning <version>, so freshness has something to compare against.
put_record() {
	local name="$1" version="$2"
	mkdir -p "$TREE/$name/results"
	jq -n --arg v "$version" \
		'{schema_version: 1, provider: "opencode", status: "confirmed",
		  depth: null, date: "2026-01-01", tool_version: $v,
		  assertion: {projection: ".", expected: 1, observed: 1}}' \
		>"$TREE/$name/results/2026-01-01T000000-opencode-$version.json"
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

@test "a billed package is withheld by default, naming the flag that frees it" {
	# The default has to withhold: a batch run that costs money on every
	# invocation is a batch run people stop invoking.
	put_package alpha billed

	run "$RUN"
	[ "$status" -eq 0 ]
	[[ "$output" == *"driver billed"* ]]
	[[ "$output" == *"authorize with --billed"* ]]
	[[ "$output" == *"1 skipped"* ]]
	[[ "$output" == *"1 need --billed"* ]]
	[[ "$output" == *"0 need --manual"* ]]
	# The money-spending part must not have been invoked.
	[[ "$output" != *"ran alpha"* ]]
}

@test "a billed package runs under --billed" {
	put_package alpha billed

	run "$RUN" --billed
	[ "$status" -eq 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" == *"1 ran"* ]]
	[[ "$output" == *"0 need --billed"* ]]
}

@test "a manual package is withheld by default, naming the flag that frees it" {
	put_package alpha manual

	run "$RUN"
	[ "$status" -eq 0 ]
	[[ "$output" == *"driver manual"* ]]
	[[ "$output" == *"authorize with --manual"* ]]
	[[ "$output" == *"1 need --manual"* ]]
	[[ "$output" != *"ran alpha"* ]]
}

@test "a manual package runs under --manual" {
	# Every manual probe blocks through probe_human_run in production; the stub
	# stands in for that, so what is pinned here is the authorization decision
	# rather than the human flow.
	put_package alpha manual

	run "$RUN" --manual
	[ "$status" -eq 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" == *"1 ran"* ]]
}

@test "--billed does not authorize manual, and --manual does not authorize billed" {
	# The whole point of a set over a single escalating level: authorizing one
	# cost must not silently authorize a different one.
	put_package alpha billed
	put_package beta manual

	run "$RUN" --billed
	[ "$status" -eq 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" != *"ran beta"* ]]

	run "$RUN" --manual
	[ "$status" -eq 0 ]
	[[ "$output" != *"ran alpha"* ]]
	[[ "$output" == *"ran beta"* ]]
}

@test "authorization flags stack" {
	put_package alpha billed
	put_package beta manual
	put_package gamma unattended

	run "$RUN" --billed --manual
	[ "$status" -eq 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" == *"ran beta"* ]]
	[[ "$output" == *"ran gamma"* ]]
	[[ "$output" == *"3 ran"* ]]
	[[ "$output" == *"0 skipped"* ]]
}

@test "authorization flags are order-independent" {
	# Each flag adds to a set rather than setting a level, so no ordering can
	# make one flag overwrite another.
	put_package alpha billed
	put_package beta manual

	run "$RUN" --manual --billed
	[ "$status" -eq 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" == *"ran beta"* ]]
	[[ "$output" == *"2 ran"* ]]
}

@test "a repeated flag authorizes the same driver once" {
	put_package alpha billed

	run "$RUN" --billed --billed
	[ "$status" -eq 0 ]
	[[ "$output" == *"1 ran"* ]]
}

@test "--all authorizes every driver" {
	put_package alpha unattended
	put_package beta billed
	put_package gamma manual

	run "$RUN" --all
	[ "$status" -eq 0 ]
	[[ "$output" == *"3 ran"* ]]
	[[ "$output" == *"0 skipped"* ]]
}

@test "PROBE_AUTHORIZE_DRIVERS seeds the same set the flags add to" {
	# The environment form exists so a caller that cannot pass arguments —
	# anything invoking the script rather than the recipe — can still opt in.
	put_package alpha billed
	put_package beta manual

	PROBE_AUTHORIZE_DRIVERS="billed manual" run "$RUN"
	[ "$status" -eq 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" == *"ran beta"* ]]
	[[ "$output" == *"2 ran"* ]]
}

@test "PROBE_AUTHORIZE_DRIVERS composes with a flag rather than replacing it" {
	put_package alpha billed
	put_package beta manual

	PROBE_AUTHORIZE_DRIVERS="billed" run "$RUN" --manual
	[ "$status" -eq 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" == *"ran beta"* ]]
	[[ "$output" == *"2 ran"* ]]
}

@test "PROBE_AUTHORIZE_DRIVERS naming an unknown driver exits 2" {
	# A typo'd driver name must not silently authorize nothing and look like a
	# run that simply found nothing to do.
	put_package alpha billed

	PROBE_AUTHORIZE_DRIVERS="biled" run "$RUN"
	[ "$status" -eq 2 ]
	[[ "$output" == *"unknown driver"* ]]
	[[ "$output" != *"ran alpha"* ]]
}

@test "the summary breakdown reports each withheld driver separately" {
	# A single count would let the parenthetical claim every skip had one
	# cause, and the reader could not tell which flag to reach for.
	put_package alpha manual
	put_package beta billed
	put_package gamma billed

	run "$RUN"
	[ "$status" -eq 0 ]
	[[ "$output" == *"0 ran"* ]]
	[[ "$output" == *"3 skipped (2 need --billed · 1 need --manual)"* ]]
}

@test "an unknown argument exits 2 without running any package" {
	put_package alpha unattended

	run "$RUN" --bogus
	[ "$status" -eq 2 ]
	[[ "$output" == *"unknown argument"* ]]
	[[ "$output" == *"--billed --manual --all --stale"* ]]
	[[ "$output" != *"ran alpha"* ]]
}

@test "--stale runs a package that has never recorded" {
	put_package alpha unattended

	run "$RUN" --stale
	[ "$status" -eq 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" == *"1 ran"* ]]
	[[ "$output" == *"0 fresh"* ]]
}

@test "--stale runs a package whose installed version has moved" {
	put_package alpha unattended
	put_version_stub alpha 2.0.0
	put_record alpha 1.0.0

	run "$RUN" --stale
	[ "$status" -eq 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" == *"1 ran"* ]]
}

@test "--stale passes over a package recorded at the installed version" {
	put_package alpha unattended
	put_version_stub alpha 1.0.0
	put_record alpha 1.0.0

	run "$RUN" --stale
	[ "$status" -eq 0 ]
	[[ "$output" != *"ran alpha"* ]]
	[[ "$output" == *"fresh"* ]]
	[[ "$output" == *"1.0.0 unchanged"* ]]
	[[ "$output" == *"0 ran"* ]]
	[[ "$output" == *"1 skipped (0 need --billed · 0 need --manual · 1 fresh)"* ]]
}

@test "--stale treats an incomparable version as fresh rather than as owed" {
	# `version_source.kind: "none"` — what every put_package manifest declares.
	# Unknowable is not the same as stale: counting it as owed would put every
	# capture-sourced package permanently in the run set, filtering nothing.
	put_package alpha unattended
	put_record alpha 1.0.0

	run "$RUN" --stale
	[ "$status" -eq 0 ]
	[[ "$output" != *"ran alpha"* ]]
	[[ "$output" == *"fresh"* ]]
	[[ "$output" == *"1 fresh"* ]]
}

@test "--stale treats an unreadable newest record as never having recorded" {
	put_package alpha unattended
	mkdir -p "$TREE/alpha/results"
	printf 'not json\n' >"$TREE/alpha/results/2026-01-01T000000-opencode-1.0.0.json"

	run "$RUN" --stale
	[ "$status" -eq 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" == *"1 ran"* ]]
}

@test "--stale skips a fresh manual package instead of pointing at its README" {
	# The manual skip line exists to tell you to go drive the probe by hand.
	# For a package whose answer cannot have changed, that is a false errand.
	put_package alpha manual
	put_version_stub alpha 1.0.0
	put_record alpha 1.0.0

	run "$RUN" --stale
	[ "$status" -eq 0 ]
	[[ "$output" != *"authorize with --manual"* ]]
	[[ "$output" == *"1 fresh"* ]]
	[[ "$output" == *"0 need --manual"* ]]
}

@test "--stale still names a drifted manual package as needing a hand-run" {
	put_package alpha manual
	put_version_stub alpha 2.0.0
	put_record alpha 1.0.0

	run "$RUN" --stale
	[ "$status" -eq 0 ]
	[[ "$output" == *"authorize with --manual"* ]]
	[[ "$output" == *"1 need --manual"* ]]
	[[ "$output" == *"0 fresh"* ]]
}

@test "--stale does not on its own authorize spending on a drifted billed package" {
	# Selection and authorization are independent: being owed a run is not
	# permission to pay for one.
	put_package alpha billed
	put_version_stub alpha 2.0.0
	put_record alpha 1.0.0

	run "$RUN" --stale
	[ "$status" -eq 0 ]
	[[ "$output" != *"ran alpha"* ]]
	[[ "$output" == *"authorize with --billed"* ]]
	[[ "$output" == *"1 need --billed"* ]]
}

@test "--stale composes with --billed to reach a drifted billed package" {
	put_package alpha billed
	put_version_stub alpha 2.0.0
	put_record alpha 1.0.0
	put_package beta billed
	put_version_stub beta 1.0.0
	put_record beta 1.0.0

	run "$RUN" --stale --billed
	[ "$status" -eq 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" != *"ran beta"* ]]
	[[ "$output" == *"1 ran"* ]]
	[[ "$output" == *"1 fresh"* ]]
}

@test "a fresh package is passed over before its cost is ever considered" {
	# Selection runs first, so a fresh billed package is reported as fresh
	# rather than as needing authorization it would never be asked to spend.
	put_package alpha billed
	put_version_stub alpha 1.0.0
	put_record alpha 1.0.0

	run "$RUN" --stale
	[ "$status" -eq 0 ]
	[[ "$output" == *"fresh"* ]]
	[[ "$output" == *"1 fresh"* ]]
	[[ "$output" == *"0 need --billed"* ]]
}

@test "a default run reports no fresh bucket at all" {
	# Not `0 fresh`: on a default run freshness is never evaluated, and a zero
	# would read as having been evaluated and found nothing.
	put_package alpha unattended
	put_version_stub alpha 1.0.0
	put_record alpha 1.0.0

	run "$RUN"
	[ "$status" -eq 0 ]
	[[ "$output" == *"ran alpha"* ]]
	[[ "$output" != *"fresh"* ]]
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
