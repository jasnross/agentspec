#!/usr/bin/env bash
# The manifest contract's enumerated values, in one place, so `record.sh` and
# the bats suite gate on the same definition.
#
# This file exists because a duplicated enum drifts by having one copy keep
# accepting a value the other retired — the exact failure the probe harness was
# built to surface in providers, turned inward on the harness itself. `record.sh`
# sources this directly; the bats suite reaches it through `record_contract.bash`.
#
# This file is meant to be sourced, not executed.

# shellcheck disable=SC2034 # Every constant here is consumed by a sourcing file.

# The depths a probe can actually gather evidence for. `emitted-bytes` is the
# hop agentspec's own tests already cover, and `provider-parses` would assert
# that no error occurred — which the contract forbids. Neither is a depth, so a
# record can never claim one. Applied to a `.depth` value.
MANIFEST_DEPTH_JQ='. == null or . == "resolved-config" or . == "outbound-request"'

# An option's declared status replaces the comparison against `expected`, which
# is how `couldnt-tell` yields `inconclusive`. Any other declared status would
# fix the verdict in advance — the caller-supplied status the record contract
# exists to prevent, arriving through the manifest instead of the command line.
#
# `[]?` is vacuously true wherever there is nothing to iterate, which covers a
# projection manifest; the `type` guard keeps a malformed non-object option
# falling through to the clearer refusal downstream rather than raising into
# this gate's diagnostic. Applied to a whole manifest.
MANIFEST_OPTION_STATUS_JQ='all(.assertion.options[]? | select(type == "object" and has("status")) | .status; . == "inconclusive")'
