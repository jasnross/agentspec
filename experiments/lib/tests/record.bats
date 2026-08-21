#!/usr/bin/env bats
# Coverage for `record.sh`: comparison, freshness refusal, selection handling,
# and record well-formedness. No provider CLI is involved — every oracle output
# here is hand-written, so this suite runs anywhere `bats` and `jq` do.

setup() {
	EXPERIMENTS="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
	RECORD="$EXPERIMENTS/lib/record.sh"
	PKG="$BATS_TEST_TMPDIR/pkg"
	mkdir -p "$PKG"
	load record_contract
}

write_manifest() {
	cat >"$PKG/probe.json"
}

write_view() {
	cat >"$BATS_TEST_TMPDIR/view.json"
}

script_manifest() {
	write_manifest <<-'JSON'
		{
		  "schema_version": 1,
		  "provider": "opencode",
		  "driver": "unattended",
		  "depth": "resolved-config",
		  "question": "Test manifest.",
		  "version_source": { "kind": "none" },
		  "assertion": {
		    "projection": "{model, variant}",
		    "expected": { "model": "m", "variant": "high" }
		  }
		}
	JSON
}

judge_manifest() {
	write_manifest <<-'JSON'
		{
		  "schema_version": 1,
		  "provider": "cursor",
		  "driver": "manual",
		  "depth": null,
		  "question": "Which markers appeared?",
		  "version_source": { "kind": "none" },
		  "assertion": {
		    "options": [
		      { "id": "both-markers", "text": "Both markers were visible" },
		      { "id": "neither", "text": "Neither marker was visible" },
		      { "id": "couldnt-tell", "text": "Could not determine", "status": "inconclusive" }
		    ],
		    "expected": "neither"
		  }
		}
	JSON
}

# A workspace shaped the way `probe_workspace_create` shapes one, carrying the
# payload a Cursor hook would have appended.
fresh_capture() {
	local ws="$BATS_TEST_TMPDIR/ws"
	mkdir -p "$ws/capture"
	printf '{"cursor_version":"3.16.17","model":"m","variant":"high"}\n' \
		>"$ws/capture/payloads.jsonl"
	printf '%s' "$ws"
}

# The newest record. Filenames sort lexicographically in run order, which is
# what the filename's time component is for. `head` would return the oldest,
# which is indistinguishable from the newest only while a test writes one.
the_record() {
	cat "$(ls "$PKG"/results/*.json | tail -n 1)"
}

@test "a matching observed value records confirmed" {
	script_manifest
	write_view <<<'{"model":"m","variant":"high"}'

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -eq 0 ]
	[ "$(the_record | jq -r .status)" = "confirmed" ]
}

@test "a mismatching observed value records refuted" {
	script_manifest
	write_view <<<'{"model":"m","variant":"low"}'

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -eq 0 ]
	[ "$(the_record | jq -r .status)" = "refuted" ]
	[ "$(the_record | jq -r .assertion.observed.variant)" = "low" ]
}

@test "comparison is structural, so key order does not matter" {
	# This is the property JSON was chosen for and the one a string comparison
	# would have broken.
	script_manifest
	write_view <<<'{"variant":"high","model":"m"}'

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -eq 0 ]
	[ "$(the_record | jq -r .status)" = "confirmed" ]
}

@test "a manual projection manifest without --capture is refused" {
	write_manifest <<-'JSON'
		{
		  "schema_version": 1, "provider": "cursor", "driver": "manual", "depth": null,
		  "question": "q", "version_source": { "kind": "none" },
		  "assertion": { "projection": ".", "expected": {} }
		}
	JSON
	write_view <<<'{}'

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -ne 0 ]
	[[ "$output" == *"requires --capture"* ]]
}

@test "a manual options manifest without --capture is refused" {
	judge_manifest

	run "$RECORD" --manifest "$PKG/probe.json" --selection neither
	[ "$status" -ne 0 ]
	[[ "$output" == *"requires --capture"* ]]
}

@test "a selection matching expected records confirmed" {
	judge_manifest
	ws=$(fresh_capture)

	run "$RECORD" --manifest "$PKG/probe.json" --selection neither --capture "$ws"
	[ "$status" -eq 0 ]
	[ "$(the_record | jq -r .status)" = "confirmed" ]
	[ "$(the_record | jq -r .assertion.observed)" = "neither" ]
}

@test "a selection not matching expected records refuted" {
	judge_manifest
	ws=$(fresh_capture)

	run "$RECORD" --manifest "$PKG/probe.json" --selection both-markers --capture "$ws"
	[ "$status" -eq 0 ]
	[ "$(the_record | jq -r .status)" = "refuted" ]
	[ "$(the_record | jq -r .assertion.observed)" = "both-markers" ]
}

@test "the couldnt-tell option records inconclusive" {
	# Without this an operator who could not tell would produce a false pass.
	judge_manifest
	ws=$(fresh_capture)

	run "$RECORD" --manifest "$PKG/probe.json" --selection couldnt-tell --capture "$ws"
	[ "$status" -eq 0 ]
	[ "$(the_record | jq -r .status)" = "inconclusive" ]
}

@test "an option declaring a status other than inconclusive is refused" {
	# A declared status replaces the comparison, so any value but `inconclusive`
	# is the caller-supplied verdict the no-`--status` rule exists to prevent —
	# smuggled in through the manifest instead of the command line.
	write_manifest <<-'JSON'
		{
		  "schema_version": 1, "provider": "cursor", "driver": "manual", "depth": null,
		  "question": "q", "version_source": { "kind": "none" },
		  "assertion": {
		    "options": [
		      { "id": "yes", "text": "yes", "status": "confirmed" },
		      { "id": "couldnt-tell", "text": "Could not determine", "status": "inconclusive" }
		    ],
		    "expected": "yes"
		  }
		}
	JSON
	ws=$(fresh_capture)

	run "$RECORD" --manifest "$PKG/probe.json" --selection yes --capture "$ws"
	[ "$status" -ne 0 ]
	[[ "$output" == *"only status \"inconclusive\""* ]]
}

# A manifest whose option set declares `inconclusive` more than once. The
# narrowing rejects a *value*, not the presence of a status, so this must be
# accepted — a gate written as "at most one option declares a status" would
# refuse it, and both tests below would fail at the manifest check.
multi_inconclusive_manifest() {
	write_manifest <<-'JSON'
		{
		  "schema_version": 1, "provider": "cursor", "driver": "manual", "depth": null,
		  "question": "q", "version_source": { "kind": "none" },
		  "assertion": {
		    "options": [
		      { "id": "neither", "text": "Neither marker was visible" },
		      { "id": "couldnt-tell", "text": "Could not determine", "status": "inconclusive" },
		      { "id": "not-reached", "text": "The command never ran", "status": "inconclusive" }
		    ],
		    "expected": "neither"
		  }
		}
	JSON
}

@test "several inconclusive options do not stop the comparison" {
	multi_inconclusive_manifest
	ws=$(fresh_capture)

	run "$RECORD" --manifest "$PKG/probe.json" --selection neither --capture "$ws"
	[ "$status" -eq 0 ]
	[ "$(the_record | jq -r .status)" = "confirmed" ]
}

@test "an inconclusive option yields inconclusive wherever it sits in the set" {
	# `not-reached` is the second option declaring the status, so a gate reading
	# only the first would miss it.
	multi_inconclusive_manifest
	ws=$(fresh_capture)

	run "$RECORD" --manifest "$PKG/probe.json" --selection not-reached --capture "$ws"
	[ "$status" -eq 0 ]
	[ "$(the_record | jq -r .status)" = "inconclusive" ]
}

@test "a manifest declaring a retired depth is refused" {
	# `provider-parses` would be an assertion on the absence of an error, which
	# the contract forbids. A record copies `depth` from its manifest, so a
	# value gathered nowhere would become a claim on a record.
	write_manifest <<-'JSON'
		{
		  "schema_version": 1,
		  "provider": "opencode",
		  "driver": "unattended",
		  "depth": "provider-parses",
		  "question": "Test manifest.",
		  "version_source": { "kind": "none" },
		  "assertion": {
		    "projection": "{model, variant}",
		    "expected": { "model": "m", "variant": "high" }
		  }
		}
	JSON
	write_view <<<'{"model":"m","variant":"high"}'

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -ne 0 ]
	[[ "$output" == *"depth must be"* ]]
}

@test "a selection absent from the option set is refused" {
	judge_manifest
	ws=$(fresh_capture)

	run "$RECORD" --manifest "$PKG/probe.json" --selection typo --capture "$ws"
	[ "$status" -ne 0 ]
	[[ "$output" == *"not one of the manifest's option ids"* ]]
}

@test "supplying both --view and --selection is refused" {
	script_manifest
	write_view <<<'{}'

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json" --selection neither
	[ "$status" -ne 0 ]
	[[ "$output" == *"mutually exclusive"* ]]
}

@test "a record with depth null is well-formed" {
	# An off-chain finding must be representable.
	judge_manifest
	ws=$(fresh_capture)

	run "$RECORD" --manifest "$PKG/probe.json" --selection neither --capture "$ws"
	[ "$status" -eq 0 ]

	record_file=$(ls "$PKG"/results/*.json | head -n 1)
	[ "$(jq -r '.depth | type' "$record_file")" = "null" ]
	run assert_record_wellformed "$record_file"
	[ "$status" -eq 0 ]
}

@test "a capture with no payloads file is refused" {
	script_manifest
	write_view <<<'{"model":"m","variant":"high"}'
	ws="$BATS_TEST_TMPDIR/nopayloads"
	mkdir -p "$ws/capture"

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json" --capture "$ws"
	[ "$status" -ne 0 ]
	[[ "$output" == *"absent or empty"* ]]
}

@test "a capture with an empty payloads file is refused" {
	script_manifest
	write_view <<<'{"model":"m","variant":"high"}'
	ws="$BATS_TEST_TMPDIR/emptypayloads"
	mkdir -p "$ws/capture"
	: >"$ws/capture/payloads.jsonl"

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json" --capture "$ws"
	[ "$status" -ne 0 ]
	[[ "$output" == *"absent or empty"* ]]
}

@test "a fresh capture records successfully and stores nothing about the capture" {
	write_manifest <<-'JSON'
		{
		  "schema_version": 1, "provider": "cursor", "driver": "manual", "depth": null,
		  "question": "q",
		  "version_source": { "kind": "capture", "jq": "[.[] | .cursor_version] | map(select(. != null)) | first" },
		  "assertion": { "projection": "[.[] | .variant] | first", "expected": "high" }
		}
	JSON
	ws=$(fresh_capture)
	jq -s . "$ws/capture/payloads.jsonl" >"$BATS_TEST_TMPDIR/view.json"

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json" --capture "$ws"
	[ "$status" -eq 0 ]
	[ "$(the_record | jq -r .status)" = "confirmed" ]
	# The version came out of the capture, slurped.
	[ "$(the_record | jq -r .tool_version)" = "3.16.17" ]
	[ "$(the_record | jq 'has("capture")')" = "false" ]
}

@test "the emitted record parses and carries the full key set" {
	script_manifest
	write_view <<<'{"model":"m","variant":"high"}'

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -eq 0 ]

	record_file=$(ls "$PKG"/results/*.json | head -n 1)
	run assert_record_wellformed "$record_file"
	[ "$status" -eq 0 ]
}

@test "date is stamped in UTC" {
	script_manifest
	write_view <<<'{"model":"m","variant":"high"}'

	# Run under a timezone 14 hours ahead of UTC, so for most of the day a
	# local-clock stamp would produce tomorrow's date and fail this assertion.
	# The clock is read either side of the run so a genuine UTC-midnight
	# crossing tolerates the boundary instead of flaking.
	before=$(date -u +%F)
	TZ=Pacific/Kiritimati run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	after=$(date -u +%F)
	[ "$status" -eq 0 ]

	recorded=$(the_record | jq -r .date)
	[ "$recorded" = "$before" ] || [ "$recorded" = "$after" ]
}

@test "a record carries no key outside the seven-key contract" {
	script_manifest
	write_view <<<'{"model":"m","variant":"high"}'

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -eq 0 ]

	record_file=$(ls "$PKG"/results/*.json | head -n 1)
	for key in probe kind driver capture finding blocked_reason; do
		[ "$(jq --arg k "$key" 'has($k)' "$record_file")" = "false" ]
	done
}

@test "a path-hostile version string is sanitized in the record filename" {
	# The version command is exec'd as argv, not through a shell, so the
	# hostile string comes from a stub binary rather than shell quoting.
	stub="$BATS_TEST_TMPDIR/hostile-version"
	printf '#!/usr/bin/env bash\nprintf "1.0/../etc x\\n"\n' >"$stub"
	chmod +x "$stub"

	jq -n --arg cmd "$stub" \
		'{schema_version: 1, provider: "opencode", driver: "unattended", depth: null,
		  question: "q", version_source: {kind: "command", command: $cmd},
		  assertion: {projection: ".", expected: {}}}' >"$PKG/probe.json"
	write_view <<<'{}'

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -eq 0 ]

	name=$(basename "$(ls "$PKG"/results/*.json | head -n 1)")
	[[ "$name" != *"/../"* ]]
	[[ "$name" != *" "* ]]
	[[ "$name" == *"1.0_.._etc_x.json" ]]
}

@test "a manifest declaring no expected value is refused" {
	# Otherwise `jq '.assertion.expected'` yields null, a misspelled projection
	# also yields null, and `null == null` records a vacuous confirmed — the
	# exact failure the contract claims to prevent.
	write_manifest <<-'JSON'
		{
		  "schema_version": 1, "provider": "opencode", "driver": "unattended", "depth": null,
		  "question": "q", "version_source": { "kind": "none" },
		  "assertion": { "projection": ".no.such.path" }
		}
	JSON
	write_view <<<'{"real":"data"}'

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -ne 0 ]
	[[ "$output" == *"declares no expected value"* ]]
	[ ! -d "$PKG/results" ]
}

@test "a manifest declaring both projection and options is refused" {
	write_manifest <<-'JSON'
		{
		  "schema_version": 1, "provider": "cursor", "driver": "manual", "depth": null,
		  "question": "q", "version_source": { "kind": "none" },
		  "assertion": {
		    "projection": ".", "options": [{ "id": "a", "text": "a" }], "expected": "a"
		  }
		}
	JSON
	write_view <<<'{}'

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -ne 0 ]
	[[ "$output" == *"exactly one of projection or options"* ]]
}

@test "a manifest declaring options under any other driver is refused" {
	# The correspondence `probe-common.sh` depends on: an options assertion is
	# a person choosing from a list, so running the probe requires a person
	# present. Both other members of the enum are covered rather than just one,
	# because `billed` is scriptable and so is the value a future edit is most
	# likely to loosen the gate for.
	for driver in unattended billed; do
		rm -rf "$PKG/results"
		jq -n --arg d "$driver" \
			'{schema_version: 1, provider: "cursor", driver: $d, depth: null,
			  question: "q", version_source: {kind: "none"},
			  assertion: {
			    options: [
			      {id: "yes", text: "yes"},
			      {id: "couldnt-tell", text: "Could not determine", status: "inconclusive"}
			    ],
			    expected: "yes"
			  }}' >"$PKG/probe.json"
		ws=$(fresh_capture)

		run "$RECORD" --manifest "$PKG/probe.json" --selection yes --capture "$ws"
		[ "$status" -ne 0 ]
		[[ "$output" == *"requires a manual driver"* ]]
		[ ! -d "$PKG/results" ]
	done
}

@test "a manifest declaring neither projection nor options is refused" {
	write_manifest <<-'JSON'
		{
		  "schema_version": 1, "provider": "cursor", "driver": "unattended", "depth": null,
		  "question": "q", "version_source": { "kind": "none" },
		  "assertion": { "expected": "a" }
		}
	JSON
	write_view <<<'{}'

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -ne 0 ]
	[[ "$output" == *"exactly one of projection or options"* ]]
}

@test "an options manifest invoked with --view is refused, not recorded as refuted" {
	# A wiring error must not manufacture the strong signal.
	judge_manifest
	write_view <<<'{}'

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -ne 0 ]
	[[ "$output" == *"use --selection"* ]]
	[ ! -d "$PKG/results" ]
}

@test "a projection manifest invoked with --selection is refused" {
	script_manifest
	ws=$(fresh_capture)

	run "$RECORD" --manifest "$PKG/probe.json" --selection anything --capture "$ws"
	[ "$status" -ne 0 ]
	[[ "$output" == *"use --view"* ]]
}

@test "a manifest with an unknown provider is refused" {
	# Also keeps `provider` out of the record path, which is built from it.
	write_manifest <<-'JSON'
		{
		  "schema_version": 1, "provider": "../../etc", "driver": "unattended", "depth": null,
		  "question": "q", "version_source": { "kind": "none" },
		  "assertion": { "projection": ".", "expected": {} }
		}
	JSON
	write_view <<<'{}'

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -ne 0 ]
	[[ "$output" == *"provider must be"* ]]
}

@test "a manifest with an unknown schema_version is refused" {
	write_manifest <<-'JSON'
		{
		  "schema_version": 99, "provider": "opencode", "driver": "unattended", "depth": null,
		  "question": "q", "version_source": { "kind": "none" },
		  "assertion": { "projection": ".", "expected": {} }
		}
	JSON
	write_view <<<'{}'

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -ne 0 ]
	[[ "$output" == *"schema_version must be 1"* ]]
}

@test "a manifest with an unknown driver is refused" {
	# `script` rather than an invented string: a retired value is what a stale
	# manifest actually carries, and it is the one the enum has to reject.
	write_manifest <<-'JSON'
		{
		  "schema_version": 1, "provider": "opencode", "driver": "script", "depth": null,
		  "question": "q", "version_source": { "kind": "none" },
		  "assertion": { "projection": ".", "expected": {} }
		}
	JSON
	write_view <<<'{}'

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -ne 0 ]
	[[ "$output" == *"driver must be unattended, billed, or manual"* ]]
	[ ! -d "$PKG/results" ]
}

@test "a projection yielding several values is refused with a clear diagnostic" {
	write_manifest <<-'JSON'
		{
		  "schema_version": 1, "provider": "opencode", "driver": "unattended", "depth": null,
		  "question": "q", "version_source": { "kind": "none" },
		  "assertion": { "projection": ".[]", "expected": 1 }
		}
	JSON
	write_view <<<'[1,2,3]'

	run "$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -ne 0 ]
	[[ "$output" == *"exactly one JSON value"* ]]
}

@test "a duplicated option id is refused" {
	write_manifest <<-'JSON'
		{
		  "schema_version": 1, "provider": "cursor", "driver": "manual", "depth": null,
		  "question": "q", "version_source": { "kind": "none" },
		  "assertion": {
		    "options": [
		      { "id": "dup", "text": "one" },
		      { "id": "dup", "text": "two", "status": "inconclusive" }
		    ],
		    "expected": "dup"
		  }
		}
	JSON
	ws=$(fresh_capture)

	run "$RECORD" --manifest "$PKG/probe.json" --selection dup --capture "$ws"
	[ "$status" -ne 0 ]
	[[ "$output" == *"2 times"* ]]
}

@test "two runs on the same day at the same version produce two records" {
	# The append-only property the filename's time component exists to guarantee:
	# re-running a refuted probe to confirm it must not overwrite the evidence.
	script_manifest
	write_view <<<'{"model":"m","variant":"high"}'

	"$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json" >/dev/null
	sleep 1
	"$RECORD" --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json" >/dev/null

	[ "$(ls "$PKG"/results/*.json | wc -l | tr -d ' ')" -eq 2 ]
}

# `--dry-run` prints the would-be record on stdout and its diagnostic on stderr.
# Splitting them at the redirect rather than through `run --separate-stderr`
# keeps this file free of a bats version floor nothing else in it needs: with
# the two streams merged, the diagnostic line would make the captured output
# unparseable as JSON.
dry_run() {
	"$RECORD" --dry-run "$@" >"$BATS_TEST_TMPDIR/dry.json"
}

the_dry_record() {
	cat "$BATS_TEST_TMPDIR/dry.json"
}

@test "--dry-run writes no record" {
	script_manifest
	write_view <<<'{"model":"m","variant":"high"}'

	run dry_run --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -eq 0 ]
	[[ "$output" == *"no file written"* ]]
	[ ! -d "$PKG/results" ]
}

@test "--dry-run prints a well-formed record to stdout" {
	# The point of the flag: what it prints is what a record would contain, so
	# a projection validated here is the one the real run will evaluate.
	script_manifest
	write_view <<<'{"model":"m","variant":"high"}'

	run dry_run --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -eq 0 ]

	run assert_record_wellformed "$BATS_TEST_TMPDIR/dry.json"
	[ "$status" -eq 0 ]
}

@test "--dry-run exits 0 when the comparison computes refuted" {
	# Validating that an assertion discriminates means running it against an
	# input it is supposed to refute, so a nonzero exit here would fail the
	# flag's primary use case every time.
	script_manifest
	write_view <<<'{"model":"m","variant":"low"}'

	run dry_run --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -eq 0 ]
	[ "$(the_dry_record | jq -r .status)" = "refuted" ]
	[ ! -d "$PKG/results" ]
}

@test "--dry-run still refuses a manifest failing a gate" {
	# Every gate is part of the wiring being verified, so dry-run enforces all
	# of them; only the write is skipped.
	write_manifest <<-'JSON'
		{
		  "schema_version": 1, "provider": "opencode", "driver": "unattended",
		  "depth": "provider-parses",
		  "question": "q", "version_source": { "kind": "none" },
		  "assertion": { "projection": ".", "expected": {} }
		}
	JSON
	write_view <<<'{}'

	run dry_run --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json"
	[ "$status" -ne 0 ]
	[[ "$output" == *"depth must be"* ]]
	# A refused dry run emits no would-be record, so a scripted `--dry-run >file`
	# cannot leave a partial artifact behind.
	[ ! -s "$BATS_TEST_TMPDIR/dry.json" ]
}

@test "--dry-run still enforces the capture non-empty check" {
	script_manifest
	write_view <<<'{"model":"m","variant":"high"}'
	ws="$BATS_TEST_TMPDIR/emptypayloads"
	mkdir -p "$ws/capture"
	: >"$ws/capture/payloads.jsonl"

	run dry_run --manifest "$PKG/probe.json" --view "$BATS_TEST_TMPDIR/view.json" --capture "$ws"
	[ "$status" -ne 0 ]
	[[ "$output" == *"absent or empty"* ]]
	[ ! -s "$BATS_TEST_TMPDIR/dry.json" ]
}

@test "--dry-run records nothing on the selection path either" {
	# The options path is the expensive one to get wrong — a human-judged probe
	# discovers a wiring error only during a live session. The flag is passed
	# last here on purpose: both usage lines show it trailing, and the helper
	# above always passes it first, so nothing else exercises that position.
	judge_manifest
	ws=$(fresh_capture)

	run "$RECORD" --manifest "$PKG/probe.json" --selection neither --capture "$ws" --dry-run
	[ "$status" -eq 0 ]
	[ ! -d "$PKG/results" ]
	[[ "$output" == *"no file written"* ]]
}
