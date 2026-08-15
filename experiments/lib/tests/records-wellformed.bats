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
	# Deliberately vacuous on an empty glob: a package with no records is
	# legitimate, and so is a checkout before the first probe run.
	for record in "$EXPERIMENTS"/*/results/*.json; do
		[ -e "$record" ] || continue
		run assert_record_wellformed "$record"
		if [ "$status" -ne 0 ]; then
			printf '%s\n' "$output" >&2
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
