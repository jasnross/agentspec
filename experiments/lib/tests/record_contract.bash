#!/usr/bin/env bash
# The record contract, in one place, so `record.bats` and
# `records-wellformed.bats` assert against the same definition.

# Exactly these seven keys. No more, no fewer.
RECORD_KEYS='["assertion","date","depth","provider","schema_version","status","tool_version"]'

# Six fields removed during planning. A schema is only as narrow as what
# rejects additions, so each is asserted absent rather than merely unused.
RECORD_FORBIDDEN_KEYS='["probe","kind","driver","capture","finding","blocked_reason"]'

# Assert one record file satisfies the contract. Prints a diagnostic and
# returns 1 on any violation.
assert_record_wellformed() {
	local file="$1"

	if ! jq -e . "$file" >/dev/null 2>&1; then
		printf 'not valid JSON: %s\n' "$file" >&2
		return 1
	fi

	if ! jq -e --argjson want "$RECORD_KEYS" '(keys | sort) == $want' "$file" >/dev/null; then
		printf 'key set mismatch in %s\n  want: %s\n  got:  %s\n' \
			"$file" "$RECORD_KEYS" "$(jq -c 'keys | sort' "$file")" >&2
		return 1
	fi

	if ! jq -e --argjson forbidden "$RECORD_FORBIDDEN_KEYS" \
		'[keys[] | select(. as $k | $forbidden | index($k))] | length == 0' "$file" >/dev/null; then
		printf 'forbidden key present in %s: %s\n' "$file" "$(jq -c 'keys' "$file")" >&2
		return 1
	fi

	if ! jq -e '.status | . == "confirmed" or . == "refuted" or . == "inconclusive"' "$file" >/dev/null; then
		printf 'status not one of confirmed/refuted/inconclusive in %s\n' "$file" >&2
		return 1
	fi

	if ! jq -e '.assertion | has("expected") and has("observed") and (has("projection") or has("options"))' \
		"$file" >/dev/null; then
		printf 'assertion missing expected/observed/(projection|options) in %s\n' "$file" >&2
		return 1
	fi

	if ! jq -e '.date | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}$")' "$file" >/dev/null; then
		printf 'date is not YYYY-MM-DD in %s\n' "$file" >&2
		return 1
	fi
}
