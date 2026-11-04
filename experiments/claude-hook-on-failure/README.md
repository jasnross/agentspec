# claude-hook-on-failure

**Question:** Does `onFailure: block` stop a tool call when its PreToolUse hook exits 1, where the same hook without the field lets the call run?

**Driver:** `billed`. A command answers the question with no human step, but every arm spends a billed model call, so `just probe-run` withholds this package unless `--billed` is passed.

**Depth:** `null`. The finding is about what a hook's failure does to a tool call, which sits off the config-rendering chain.

## Why it matters

`fail_closed = true` on a hook renders as `"onFailure": "block"` on Claude Code. A hook that blocks an action only when it works leaves that action unguarded whenever it crashes, so the rendering is only worth having if the field changes what a failing hook does.

## The two arms

Each arm runs `claude -p` with `--allowedTools Bash --effort low`, and the prompt asks for one Bash command that lowercases the marker. Each arm's `.claude/settings.json` holds a `PreToolUse` hook on `Bash` that exits 1.

| Arm     | Hook     | `onFailure` |
| ------- | -------- | ----------- |
| `block` | `exit 1` | `block`     |
| `unset` | `exit 1` | absent      |

The marker travels in the prompt, so it sits in every request's `messages[]`, and `marker_field` is `MarkerField.MESSAGES`.

## The oracle

The assertion reads only `tool_result` blocks, because the marker in the prompt would otherwise read as output. `ran` means a result holds the lowercased marker, which only running the command produces, and `blocked` means every result has `is_error` true. Any other result text is returned under `unrecognized`, so a run neither value describes reads as `refuted` with the text visible.

`ToolAnswered` fails the run for an arm whose model never called Bash, which would otherwise have no results to read.

## Discrimination

The `discriminator` variant changes the `block` arm's hook to `exit 0`, leaving `onFailure` in place, so the assertion returns `ran` for both arms. That shows the `block` arm's `blocked` comes from the hook failing rather than from the field's presence, and the samples `confirming` and `refuting` hold both captures.

## Limits

This measures a hook exiting 1 on a Bash call. A hook that cannot start, times out, or prints invalid JSON takes the same documented route, and none of those are measured here. Nor does this package measure `onFailure` on versions before v2.1.295.
