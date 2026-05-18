# Canonical Hook Payload Schema

agentspec's hook system uses a provider-neutral wire format on stdin and stdout, regardless of whether the hook is running under Claude Code or Cursor. This document is the reference for what that wire format looks like, when it diverges from provider behavior, and how to write portable hooks against it.

## Why a canonical schema

Claude Code and Cursor each have their own hook payload shape — different field names (`session_id` vs. `conversation_id`), different output dialects (nested `hookSpecificOutput.permissionDecision` vs. flat `permission`), and divergent semantics for subagent firings. A hook script written against one provider's shape doesn't port to the other without explicit translation.

agentspec wraps every user hook script with a per-event POSIX shell shim that translates between the provider's native shape and a single canonical shape. The user's script sees canonical JSON on stdin and emits canonical JSON on stdout. Identical scripts produce semantically identical behavior under both providers, modulo a small set of documented limitations covered below.

The shim is generated at agentspec-compile time, ships as plain `#!/usr/bin/env sh` + `jq`, and has no Rust-binary runtime dependency on the user's machine. Every shim carries both Claude and Cursor jq dialects and auto-detects the host at runtime, so a plugin compiled for one provider works correctly when running inside the other (cross-host scenarios).

## Runtime prerequisites

The shim invokes [`jq`](https://jqlang.org/) three times per hook firing — once to detect the host provider, once to translate provider stdin into canonical JSON, and once to translate the script's canonical stdout into provider stdout. `jq` must be installed on the user's machine and on `PATH` at hook-fire time.

| Platform        | Install command                                         |
| --------------- | ------------------------------------------------------- |
| macOS           | `brew install jq`                                       |
| Debian / Ubuntu | `sudo apt install jq`                                   |
| Fedora / RHEL   | `sudo dnf install jq`                                   |
| Arch            | `sudo pacman -S jq`                                     |
| Alpine          | `apk add jq`                                            |
| Windows         | Not supported in v1 (POSIX shim is macOS / Linux only). |

If `jq` is not on `PATH` when the shim fires, the shim prints the following stderr message and exits 1 before the user's script runs:

```
agentspec: jq is required for canonical hook translation but was not found on PATH. Install jq (e.g., `brew install jq`, `apt install jq`) and reload the hook host.
```

The hook host (Claude Code / Cursor) sees the non-zero exit and surfaces the error.

Auto-installation of `jq` into each plugin's persistent-data directory (`${CLAUDE_PLUGIN_DATA}` / `${CURSOR_PLUGIN_DATA}`) is on the roadmap — see [Documented Limitations](#documented-limitations) below.

## Canonical input fields

Every canonical input payload contains the same **envelope** fields, plus event-specific fields. The envelope is identical across events; only the event-specific tail differs.

### Envelope (all events)

| Field | Type | Optional | Source: Claude | Source: Cursor |
| --- | --- | --- | --- | --- |
| `schema_version` | `string` | no | literal `"1.0.0"` | literal `"1.0.0"` |
| `provider` | `"claude"`/`"cursor"` | no | literal `"claude"` | literal `"cursor"` |
| `event` | snake_case enum | no | derived from event | derived from event |
| `session_id` | `string` | no | `session_id` | `parent_conversation_id` if present, else `conversation_id` |
| `agent_id` | `string` \| `null` | yes | `agent_id` (if present) | `conversation_id` (when subagent — i.e. when `parent_conversation_id` is set) |
| `cwd` | `string` (path) | no | `cwd` | `workspace_roots[0]` |
| `transcript_path` | `string` \| `null` | yes | `transcript_path` | `transcript_path` (may be absent on first prompt of a new conversation) |
| `provider_raw` | object | no | full provider stdin verbatim | full provider stdin verbatim |

**Subagent identity model.** Claude shares `session_id` across the agent hierarchy and adds an `agent_id` overlay for subagent firings. Cursor renews `conversation_id` per subagent and carries the parent link as `parent_conversation_id`. The canonical model reconstructs a stable `session_id` from the parent link (when present) and surfaces the subagent's own id as `agent_id` — so the same canonical fields work under both providers' subagent semantics.

### Per-event tail

The event discriminator `event` is set to one of the snake_case names below. Event-specific fields:

| Event | Event-specific canonical fields |
| --- | --- |
| `pre_tool_use` | `tool_name: string`, `tool_use_id: string`, `tool_input: object` |
| `post_tool_use` | `tool_name`, `tool_use_id`, `tool_input`, `tool_response` |
| `post_tool_use_failure` | `tool_name`, `tool_use_id`, `tool_input`, `tool_response` |

`tool_use_id` is always a string but may be the empty string when the provider omits the field (rare in practice — both providers normally populate it on tool events). | `session_start` | (envelope only) | | `session_end` | (envelope only) | | `stop` | (envelope only) | | `pre_compact` | (envelope only) | | `subagent_start` | (envelope only) | | `subagent_stop` | (envelope only) | | `user_prompt_submit` | `prompt: string` |

`tool_response` is provider-shaped in v1 (Claude emits an object; Cursor emits a JSON-encoded string). See [Documented Limitations](#documented-limitations).

## Canonical output fields

Hook scripts emit canonical JSON on stdout to influence the host runtime. All fields are optional — a hook with no output (empty stdout) is the "no opinion" signal.

| Field | Type | Routed to (Claude) | Routed to (Cursor) |
| --- | --- | --- | --- |
| `schema_version` | `string` | (informational; not consumed) | (informational; not consumed) |
| `permission_decision` | `"allow"` / `"deny"` / `"ask"` | `hookSpecificOutput.permissionDecision` | `permission` |
| `decision_reason` | `string` | `hookSpecificOutput.permissionDecisionReason` | `agent_message` |
| `user_facing_message` | `string` | `hookSpecificOutput.permissionDecisionReason` (fallback when `decision_reason` absent — Claude lacks the UI/model split) | `user_message` |
| `additional_context` | `string` | `hookSpecificOutput.additionalContext` | `additional_context` |
| `updated_input` | object (tool-event only) | `hookSpecificOutput.updatedInput` | `updated_input` |

**Empty-output semantics.** A script that writes nothing to stdout (or only whitespace) produces no provider-shaped output — the shim skips the output-translation jq pass entirely when stdout is empty. Scripts that want to opt out of any decision should simply not write to stdout.

**Malformed-output handling.** If a script writes non-empty stdout that isn't valid canonical JSON, the output-translation jq fails and the shim exits 1 with an `agentspec: output translation failed …` stderr message. This intentionally overrides the script's own exit code: a hook bug should be loudly visible to the user, not silently hidden as "no decision".

The shim also rejects output containing unrecognized field names or non-object types. The six recognized canonical output fields are: `schema_version`, `permission_decision`, `decision_reason`, `user_facing_message`, `additional_context`, `updated_input`. Any other field name causes validation to fail with an error naming the specific unrecognized fields:

```
agentspec: output translation failed (jq exited 5): jq: error (at <stdin>:0): unrecognized canonical output fields: hookSpecificOutput, hookEventName
```

This catches common mistakes such as emitting provider-shaped JSON (e.g., Claude's `hookSpecificOutput` wrapper) or typos in canonical field names (e.g., `permmission_decision`). The validation mirrors the Rust-side `deny_unknown_fields` attribute on `CanonicalOutput`.

## The `provider_raw` escape hatch

Every canonical input carries `provider_raw`, a verbatim copy of the original provider stdin. Use it when:

- You need a field the canonical schema doesn't expose (e.g. Claude's `permission_mode`, Cursor's `model` / `user_email`).
- You need to disambiguate provider-specific event variants (e.g. branching on Claude's `source == "resume"` for session_start).
- You want to capture provider-specific debug information.

```sh
#!/usr/bin/env sh
INPUT=$(cat)
# Canonical (portable):
SESSION_ID=$(printf '%s' "$INPUT" | jq -r '.session_id')

# Provider-specific (escape hatch):
SOURCE=$(printf '%s' "$INPUT" | jq -r '.provider_raw.source // empty')
if [ "$SOURCE" = "resume" ]; then
    # Claude-only path; Cursor doesn't fire session_start on resume.
    ...
fi
```

`provider_raw` is the only field that intentionally exposes provider-specific shape. Everything else in the canonical envelope is designed to be the same under either provider.

## The `provider` discriminator

Every canonical input carries `provider` as a top-level string: `"claude"` or `"cursor"`. This reflects the **detected host runtime**, not the provider whose plugin tree the shim was compiled into. In native scenarios (host matches plugin), the two are the same. In cross-host scenarios (e.g., a Claude Code plugin running inside Cursor), `provider` reflects the host — so a script reading `provider` always knows which provider's runtime is actually executing the hook.

Use `provider` when you need a single branch in the script for unbridgeable behavior:

```sh
#!/usr/bin/env sh
INPUT=$(cat)
PROVIDER=$(printf '%s' "$INPUT" | jq -r '.provider')

case "$PROVIDER" in
    claude) echo '{"additional_context": "claude-only context"}' ;;
    cursor) echo '{"additional_context": "cursor-only context"}' ;;
esac
```

Most hooks don't need this — the whole point of the canonical schema is to make provider-specific branches unnecessary. Reach for it when the documented limitations below force your hand.

## Cross-host detection

Every shim carries both Claude and Cursor jq dialects and auto-detects the host runtime at startup. Detection checks for the `cursor_version` field in the raw provider payload — always present on Cursor, never present on Claude:

- If `cursor_version` is present → Cursor host detected; the shim uses Cursor's input/output jq programs.
- If `cursor_version` is absent → Claude host detected; the shim uses Claude's input/output jq programs.

This means a plugin compiled for Claude Code works correctly when running inside Cursor (and vice versa). Canonical fields are extracted using the detected host's dialect, and output is translated to the detected host's format.

In native scenarios (host matches the plugin provider), behavior is unchanged from a single-dialect shim. The detection adds one lightweight `jq -e '.cursor_version'` invocation per hook firing (total: 3 jq invocations instead of 2).

Hook scripts do not need to be aware of cross-host detection — the canonical schema hides the difference. The `provider` field in canonical input reflects the detected host, so scripts that branch on `provider` automatically get the correct value regardless of which provider's plugin tree they were installed from.

## Documented limitations

### Cursor known limitations

Cursor 3.2.21 has a partial implementation of three canonical output fields:

- **`user_message`** — does not surface in the Cursor UI.
- **`agent_message`** — does not surface in the agent context.
- **`additional_context`** — partial routing; see Cursor forum threads for the latest status.

agentspec emits a sync-time warning whenever a provider with partial canonical-output routing (today: Cursor) is in the active provider list AND any hook spec exists. The warning is generic on Cursor version — version-specific status is tracked here.

When Cursor fixes the routing gap in a future release, no agentspec change is required at the user level: hook scripts emitting `user_facing_message` / `decision_reason` / `additional_context` will start surfacing automatically once agentspec updates its capability record for Cursor.

### Session-start asymmetry

Cursor's `sessionStart` fires only on initial conversation creation, not on conversation resume. Claude's `SessionStart` fires on both. A single canonical `session_start` hook cannot achieve resume parity across the two providers.

For agentspec users targeting both Claude and Cursor with a `session_start` hook, agentspec emits a sync-time warning. To trigger logic on Claude's resume firings:

```sh
INPUT=$(cat)
SOURCE=$(printf '%s' "$INPUT" | jq -r '.provider_raw.source // empty')
if [ "$SOURCE" = "resume" ]; then
    # Claude-only path; Cursor cannot reach this branch.
    ...
fi
```

### Unreachable provider-specific outputs

These output fields are not bridgeable from canonical:

- **Cursor-only**: `followup_message`, `loop_limit`, `failClosed`, `env`, `updated_mcp_tool_output`. Use the provider discriminator + Cursor's native output schema if needed; the shim does not block extra fields written to stdout, but the canonical-output translation does NOT route them.
- **Claude-only**: `defer`, `continue: false`/`stopReason`, `suppressOutput`, `systemMessage`, `sessionTitle`, `WorktreeCreate` plain-stdout, `$CLAUDE_ENV_FILE`, `retry`. Same caveat.

Cross-provider hooks should avoid these fields; provider-specific hooks can author them via direct emission, but lose the portability the canonical schema provides.

### Tool-result type coercion

The canonical `tool_response` field is provider-shaped in v1:

- Claude emits an object (or sometimes a string).
- Cursor emits a JSON-encoded string.

A script that reads `tool_response` portably must accept both forms. Use `jq 'fromjson'` (not `jq -r '.'`) to parse Cursor's string-form into an object:

```sh
# Defensive: parse the string form into an object when on Cursor.
RESPONSE=$(printf '%s' "$INPUT" | jq -c '.tool_response')
if [ "$(printf '%s' "$INPUT" | jq -r '.provider')" = "cursor" ]; then
    # `RESPONSE` is currently a JSON-encoded string; `fromjson` parses it
    # into the equivalent object/array/etc.
    RESPONSE=$(printf '%s' "$RESPONSE" | jq 'fromjson')
fi
# `RESPONSE` is now an object on both providers; access fields normally:
# printf '%s' "$RESPONSE" | jq -r '.stdout'
```

v2 may normalize `tool_response` to always be an object. The provider's raw shape remains available via `provider_raw.tool_response` / `.tool_output`.

### Automatic jq installation

v1 requires `jq` to be installed by the user. A future enhancement will auto-install `jq` into each plugin's persistent-data directory (`${CLAUDE_PLUGIN_DATA}` / `${CURSOR_PLUGIN_DATA}`) on first session, with SHA256-pinned downloads and macOS quarantine handling. Tracked in `TODO.md` at the repo root.

### Platform support

v1 supports macOS and Linux only. Windows support is a follow-up (requires a PowerShell variant of the shim).

## Schema versioning policy

The canonical schema is SemVer-tracked via the `schema_version` field emitted on every canonical payload. Today: `"1.0.0"`.

- **Minor bumps** add new optional fields. Existing scripts continue to work unchanged.
- **Major bumps** remove or rename fields. Scripts that read removed fields will see them as absent (canonical input) or as parse errors (canonical output, which has `deny_unknown_fields`); they must be updated for the new major version.
- **Deprecations** are announced one minor version before removal, with the deprecated field still emitted alongside its replacement during the deprecation window.

Scripts that want to gate on schema version can read it directly:

```sh
VERSION=$(printf '%s' "$INPUT" | jq -r '.schema_version')
case "$VERSION" in
    1.*) ... ;;  # v1.x compatible
    *)   echo "unsupported schema version: $VERSION" >&2; exit 1 ;;
esac
```

## Migration from provider-specific scripts

Most hook scripts in the wild read a small dominant set of fields: `tool_input.command`, `tool_name`, `cwd`, `session_id`, `hook_event_name`. The canonical schema is a strict superset on these fields — i.e., the canonical name of each is identical to the dominant provider's name. **No migration is needed for scripts that read only these fields.**

Scripts that read provider-specific fields fall into a few common patterns:

### Pattern: Claude PreToolUse Bash blocker

Before (Claude-native):

```sh
#!/usr/bin/env sh
INPUT=$(cat)
CMD=$(printf '%s' "$INPUT" | jq -r '.tool_input.command // empty')
if echo "$CMD" | grep -qE 'rm -rf /'; then
    cat <<'JSON'
{"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": "blocked"}}
JSON
fi
```

(The heredoc delimiter is quoted — `<<'JSON'` rather than `<<JSON` — to suppress shell variable expansion inside the body. The body here has no expansions, but quoting the delimiter is the safer default when the body is meant as a literal.)

After (canonical):

```sh
#!/usr/bin/env sh
INPUT=$(cat)
CMD=$(printf '%s' "$INPUT" | jq -r '.tool_input.command // empty')
if echo "$CMD" | grep -qE 'rm -rf /'; then
    echo '{"permission_decision": "deny", "decision_reason": "blocked"}'
fi
```

The canonical output is shorter, doesn't repeat the event name in the JSON (the shim already knows it), and works under Cursor too.

### Pattern: Cursor `beforeShellExecution` blocker

Before (Cursor-native):

```sh
#!/usr/bin/env sh
INPUT=$(cat)
CMD=$(printf '%s' "$INPUT" | jq -r '.tool_input.command // empty')
if echo "$CMD" | grep -qE 'rm -rf /'; then
    echo '{"permission": "deny", "agent_message": "blocked"}'
fi
```

After (canonical): same as the Claude migration above. The shim handles both routings.

### Pattern: reading Claude's `permission_mode`

Before (Claude-native, no Cursor analog):

```sh
MODE=$(printf '%s' "$INPUT" | jq -r '.permission_mode')
```

After (canonical with explicit escape hatch):

```sh
# Only meaningful under Claude — Cursor's payload has no permission_mode.
MODE=$(printf '%s' "$INPUT" | jq -r '.provider_raw.permission_mode // empty')
if [ -n "$MODE" ]; then
    ...
fi
```

The `provider_raw.permission_mode` works under Claude and is `null` on Cursor, so the `// empty` makes the script defensive about single-provider fields.

## Example: cross-provider deny hook

A complete worked example — block any Bash command that writes to a `.env` file, surfacing a user-facing message under both providers:

```sh
#!/usr/bin/env sh
# block-env-writes.sh — deny `pre_tool_use` Bash events that touch .env files.
set -eu

INPUT=$(cat)
TOOL=$(printf '%s' "$INPUT" | jq -r '.tool_name // empty')
CMD=$(printf '%s' "$INPUT" | jq -r '.tool_input.command // empty')

# Only act on Bash tool firings.
[ "$TOOL" = "Bash" ] || exit 0

# Detect any redirection or write to a .env file.
if echo "$CMD" | grep -qE '\.env\b'; then
    cat <<'JSON'
{
  "permission_decision": "deny",
  "decision_reason": "writing to .env files is not allowed in this project",
  "user_facing_message": "Edit .env via your editor, not Bash."
}
JSON
fi
```

`spec/hooks/hooks.toml`:

```toml
[hooks.block-env-writes]
events = ["pre_tool_use"]
matcher = "Bash"
script = "scripts/block-env-writes.sh"
description = "Block Bash commands that touch .env files"
```

**Under Claude.** The shim translates the canonical output to:

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "deny",
    "permissionDecisionReason": "writing to .env files is not allowed in this project"
  }
}
```

The user sees the deny + reason in Claude's UI, and the model sees the reason in its context.

**Under Cursor.** The shim translates to:

```json
{
  "permission": "deny",
  "agent_message": "writing to .env files is not allowed in this project",
  "user_message": "Edit .env via your editor, not Bash."
}
```

The user sees the deny in Cursor's UI. On Cursor 3.2.21 the `agent_message` and `user_message` may not surface depending on the host runtime version — see [Cursor known limitations](#cursor-known-limitations). The deny itself is honored regardless.

**Under both providers**, the identical script with identical canonical output achieves the same semantic effect — the migration friction that existed before the canonical schema is eliminated.
