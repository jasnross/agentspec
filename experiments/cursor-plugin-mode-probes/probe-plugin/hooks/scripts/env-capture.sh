#!/usr/bin/env bash
# Gate A — captures the env vars Cursor injects into plugin-tier hook child
# processes, plus the stdin payload, to a single log file.
#
# Procedure:
#   1. Plugin is installed at ~/.cursor/plugins/local/agentspec-probe/ via
#      rsync from probe-plugin/ (see ../../README.md "Setup").
#   2. Open a new Cursor conversation; sessionStart fires and this script
#      executes.
#   3. Inspect $HOME/.cache/agentspec-plugin-probe.log. The CURSOR_*/CLAUDE_*
#      section shows the injected env vars. The CURSOR_PLUGIN_DATA RESOLUTION
#      CHECK section confirms (a) the var is present, (b) the resolved path,
#      (c) whether the directory exists on disk.
#
# Result categories:
#   - CURSOR_PLUGIN_DATA present + path exists → usable for jq auto-bootstrap.
#   - CURSOR_PLUGIN_DATA present but path doesn't exist → lazily created on
#     first write; bootstrap script must mkdir -p before use.
#   - CURSOR_PLUGIN_DATA unset → community research was wrong; bootstrap
#     pattern must use a different anchor (XDG_CACHE_HOME, $HOME/.cache, etc.).

set -euo pipefail

LOG_FILE="${PLUGIN_PROBE_LOG:-$HOME/.cache/agentspec-plugin-probe.log}"
mkdir -p "$(dirname "$LOG_FILE")"

# Cap stdin at 1MB so a malformed/streaming payload can never block forever.
payload="$(head -c 1048576)"

ts="$(date -u +%Y-%m-%dT%H:%M:%S%z)"

(
  flock -x 9
  {
    printf '\n=== %s ===\n' "$ts"

    printf '\n--- CURSOR_* / CLAUDE_* environment ---\n'
    env | grep -E '^(CURSOR_|CLAUDE_)' | sort || printf '(no matching vars)\n'

    printf '\n--- CURSOR_PLUGIN_DATA resolution check ---\n'
    if [ -n "${CURSOR_PLUGIN_DATA:-}" ]; then
      printf 'CURSOR_PLUGIN_DATA=%s\n' "$CURSOR_PLUGIN_DATA"
      if [ -d "$CURSOR_PLUGIN_DATA" ]; then
        printf 'directory exists on disk: yes\n'
        printf 'contents (top-level):\n'
        ls -la "$CURSOR_PLUGIN_DATA" 2>&1 | head -20
      else
        printf 'directory exists on disk: no (likely lazy-created on first write)\n'
      fi
    else
      printf '(unset)\n'
    fi

    printf '\n--- CURSOR_PLUGIN_ROOT resolution check ---\n'
    if [ -n "${CURSOR_PLUGIN_ROOT:-}" ]; then
      printf 'CURSOR_PLUGIN_ROOT=%s\n' "$CURSOR_PLUGIN_ROOT"
      printf 'contents (top-level):\n'
      ls -la "$CURSOR_PLUGIN_ROOT" 2>&1 | head -20
    else
      printf '(unset)\n'
    fi

    printf '\n--- stdin payload ---\n%s\n' "$payload"
  } >>"$LOG_FILE"
) 9>>"$LOG_FILE.lock"

exit 0
