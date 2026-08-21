# Cursor — hook output JSON alongside exit 2 (Gate #19)

**Question.** When a `preToolUse` hook exits with code 2 **and** emits JSON to stdout (`{"permission": "deny", "user_message": …, "agent_message": …}`), does Cursor consume the JSON fields or treat exit 2 as a short-circuit?

**Why it matters.** It determines whether a Claude `exit 2 + reason` hook can be losslessly translated to Cursor by emitting `permission: "deny"` JSON alongside exit 2. If Cursor honors only one or the other, the shim must pick a side per event.

**Driver:** `manual`. A person must drive Cursor for the probe to run. The oracle is also a person looking at a screen — no projection reaches it, ever — which the assertion states by declaring an option set rather than a projection.

## Running it

```sh
experiments/cursor-gate-19-output-json/probe.sh
```

One blocking invocation. It generates a workspace, prints the procedure, waits until the hook actually fires, and only then presents the option set. The "hook never fired" outcome is detected by polling rather than by asking, so you answer only the question no machine can.

## The option set discriminates

| Option | Meaning |
| --- | --- |
| `both-markers` | Cursor consumes JSON on exit 2 — the canonical schema can pair `permission: deny` with a user-facing message and have both honored |
| `user-only` | Partial consumption; `agent_message` routing is silently dropped under exit 2 |
| `agent-raw-json` | Cursor routes no field itself, but hook stdout reaches the agent unparsed |
| `neither` | Exit 2 short-circuits JSON entirely; the shim must choose exit 0 **with** deny JSON or exit 2 **without** it |

Plus the mandatory `couldnt-tell`, recording `inconclusive`.

**`agent-raw-json` was added after the first run, and how it got there is the point.** The original four options rested on an unexamined assumption: that the two markers travel by different routes, `user_message` to the UI and `agent_message` to the agent. The 2026-08-16 run against Cursor 3.16.17 produced neither of those — nothing rendered in the UI, but the agent quoted the entire JSON body back verbatim, both markers included. No option described it, so the operator selected `couldnt-tell` and the run recorded `inconclusive`.

The contract justifies the mandatory "couldn't tell" option as protection against a tired operator forcing a plausible answer. Here the hazard was an author who did not imagine the outcome — the same failure mode, caught by the same escape hatch. Without it the operator would have been pushed toward `neither`, which is plainly wrong: the markers did appear.

**The distinction `agent-raw-json` draws matters for shim design.** An agent quoting raw JSON is not Cursor parsing `agent_message` and routing it as a message; it reads as hook stdout reaching the agent unparsed. A shim cannot rely on field semantics it does not control.

## Prior context, not a current result

The 2026-05-10 run answered **`neither`**, against Cursor 3.2.21. That answer remains the manifest's `expected` value — the hypothesis this probe tests, not an assertion that it still holds.

**It is deliberately left as `neither` even though the 2026-08-16 observation differs**, so a re-run records `refuted` rather than a manufactured `confirmed`. Adjusting `expected` to match a new observation erases exactly the drift this harness exists to surface.

One caveat a reader should carry: it is not knowable whether behavior _changed_ between 3.2.21 and 3.16.17, or whether the 2026-05-10 observation was itself under-resolved because the `agent-raw-json` option did not exist to be chosen. The records distinguish what was observed; they cannot distinguish those two explanations.

## What this contradicts

The 2026-08-16 observation sits against a shipped claim. `docs/hooks-canonical.md:157` states:

> **`agent_message`** — does not surface in the agent context.

The agent quoted both markers back verbatim, so on 3.16.17 that content plainly reached its context. `src/adapters/cursor.rs:222` cites that doc section as justification for `fully_implements_canonical_output()` returning `false` — a verdict still supported by `user_message` genuinely not rendering in the UI, but supported by one stated reason rather than two.

Resolving that is out of this package's scope: a probe reports what it measured, and changing a shipped capability accessor is a separate decision needing its own reasoning.

## Marker strings

The hook emits two unique markers so neither can be confused with ordinary Cursor output:

- `AGENTSPEC_GATE19_USER_MARKER_0123456789` — the `user_message` field
- `AGENTSPEC_GATE19_AGENT_MARKER_9876543210` — the `agent_message` field

The capture hook deliberately does **not** follow the usual echo-`{}`-and-exit-0 contract. Emitting a JSON body together with exit 2 is exactly what this probe measures, so its `trap` guarantees the _deny_ shape instead — a capture failure still produces the output under test.
