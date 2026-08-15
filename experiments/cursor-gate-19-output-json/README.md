# Cursor — hook output JSON alongside exit 2 (Gate #19)

**Question.** When a `preToolUse` hook exits with code 2 **and** emits JSON to stdout (`{"permission": "deny", "user_message": …, "agent_message": …}`), does Cursor consume the JSON fields or treat exit 2 as a short-circuit?

**Why it matters.** It determines whether a Claude `exit 2 + reason` hook can be losslessly translated to Cursor by emitting `permission: "deny"` JSON alongside exit 2. If Cursor honors only one or the other, the shim must pick a side per event.

**Driver:** `human-judge`. The oracle is a person looking at a screen; no projection reaches it, ever.

## Running it

```sh
experiments/cursor-gate-19-output-json/probe.sh
```

One blocking invocation. It generates a workspace, prints the procedure, waits until the hook actually fires, and only then presents the option set. The "hook never fired" outcome is detected by polling rather than by asking, so you answer only the question no machine can.

## The option set discriminates

The three substantive options are mutually exclusive and exhaust what this probe can produce:

| Option | Meaning |
| --- | --- |
| `both-markers` | Cursor consumes JSON on exit 2 — the canonical schema can pair `permission: deny` with a user-facing message and have both honored |
| `user-only` | Partial consumption; `agent_message` routing is silently dropped under exit 2 |
| `neither` | Exit 2 short-circuits JSON entirely; the shim must choose exit 0 **with** deny JSON or exit 2 **without** it |

Plus the mandatory `couldnt-tell`, recording `inconclusive`. Without it, a tired operator picking the first plausible option manufactures a false pass.

## Prior context, not a current result

The 2026-05-10 run answered **`neither`**, against Cursor 3.2.21. That answer lives here as the manifest's `expected` value — as the hypothesis this probe tests, not as an assertion that it still holds. What is current is whatever `results/` contains.

If a re-run answers differently, that is assertion drift and a real finding: record it and raise it rather than adjusting `expected` to match.

## Marker strings

The hook emits two unique markers so neither can be confused with ordinary Cursor output:

- `AGENTSPEC_GATE19_USER_MARKER_0123456789` — the `user_message` field
- `AGENTSPEC_GATE19_AGENT_MARKER_9876543210` — the `agent_message` field

The capture hook deliberately does **not** follow the usual echo-`{}`-and-exit-0 contract. Emitting a JSON body together with exit 2 is exactly what this probe measures, so its `trap` guarantees the _deny_ shape instead — a capture failure still produces the output under test.
