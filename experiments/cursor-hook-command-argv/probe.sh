#!/usr/bin/env bash
# How does Cursor turn a hooks.json `command` string into the hook process's
# argv? Cursor documents neither the interpreter that reads the string nor any
# escaping rules for it, so agentspec composes commands containing spaces and a
# leading `VAR=value` assignment on an unverified assumption.
#
# Machine-judged: the answer is counted from the capture by a projection, so a
# person drives Cursor but no person interprets the result.
set -euo pipefail

package=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=experiments/lib/probe-common.sh
. "$package/../lib/probe-common.sh"

probe_require_tools jq

# A runner takes no arguments: it is one blocking invocation, so there is no
# workspace for a second one to point at.
if [ $# -ne 0 ]; then
	printf '%s: unexpected argument: %s\n' "$(basename "${BASH_SOURCE[0]}")" "$1" >&2
	exit 2
fi

# Every workspace is `mktemp -d "${TMPDIR:-/tmp}/probe-<name>.XXXXXX"`, so
# $TMPDIR is the only component of the capture-script path that this repository
# does not control. That path is substituted into a `command` string the
# provider is expected to hand to a shell, so any character with meaning to a
# shell invalidates the measurement rather than merely inconveniencing it:
# whitespace puts an extra word before the case id, a quote re-quotes a later
# case, and `$` or a backtick expands to something that resolves to no
# executable, so nothing fires at all.
#
# Nothing downstream catches any of it. `probe_template_file` rejects only an
# unsubstituted placeholder, and `probe_arrange_human_workspace`'s `jq -e .`
# rejects only a path that breaks JSON — every character below produces
# perfectly valid JSON. Fail here, before the operator spends a live session.
case "${TMPDIR:-/tmp}" in
*[' 	'\'\"\$\`\;\&\|\(\)\<\>\*\?\!\\]* | *'
'*)
	printf 'probe: TMPDIR contains whitespace or a shell metacharacter (%s);\n' "${TMPDIR:-/tmp}" >&2
	printf 'probe: the command strings this probe registers would not measure what they mean to.\n' >&2
	exit 1
	;;
esac

probe_human_run "$package" cursor-hook-command-argv "
  1. Open Cursor on that directory.
  2. Fully quit and reopen Cursor — it reads .cursor/hooks.json at start.
  3. Start a FRESH conversation (not a resume — sessionStart does not fire
     on resume, per the cursor-session-start probe).

This script is waiting and will record the result automatically.
"
