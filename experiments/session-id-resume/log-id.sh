#!/usr/bin/env bash
# Hook payload logger for session_id / conversation_id resume experiments.
# Reads one JSON object from stdin (Claude Code or Cursor hook payload),
# appends a single tab-separated line to $LOG_FILE under a flock so concurrent
# hook firings don't interleave. Exits 0 to act as a no-op hook.

set -euo pipefail

LOG_FILE="${ID_PROBE_LOG:-$HOME/.cache/id-probe.log}"
mkdir -p "$(dirname "$LOG_FILE")"

# Cap at 1MB so a malformed/streaming payload can never block forever.
payload="$(head -c 1048576)"

extract() {
  printf '%s' "$payload" | jq -r "$1 // \"-\"" 2>/dev/null || printf '%s' '-'
}

ts="$(date -u +%Y-%m-%dT%H:%M:%S%z)"
event="$(extract '.hook_event_name')"

# Claude Code fields
session_id="$(extract '.session_id')"
source_val="$(extract '.source')"
agent_id="$(extract '.agent_id')"
transcript="$(extract '.transcript_path')"

# Cursor fields (note: Cursor's sessionStart payload also has .session_id,
# defined as "same as conversation_id").
conversation_id="$(extract '.conversation_id')"
generation_id="$(extract '.generation_id')"
parent_conv="$(extract '.parent_conversation_id')"
is_bg="$(extract '.is_background_agent')"
cursor_ver="$(extract '.cursor_version')"

(
  flock -x 9
  printf '%s\tevent=%s\tsession_id=%s\tconversation_id=%s\tsource=%s\tgeneration_id=%s\tagent_id=%s\tparent_conversation_id=%s\tis_background_agent=%s\tcursor_version=%s\ttranscript=%s\n' \
    "$ts" "$event" "$session_id" "$conversation_id" "$source_val" "$generation_id" "$agent_id" "$parent_conv" "$is_bg" "$cursor_ver" "$transcript" \
    >> "$LOG_FILE"
) 9>>"$LOG_FILE.lock"

exit 0
