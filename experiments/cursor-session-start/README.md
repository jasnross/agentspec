# Cursor — does `sessionStart` fire again on resume?

**Question.** Does Cursor's `sessionStart` hook fire again when a conversation is resumed?

**Why it matters.** It drives shipped adapter behavior — see `src/adapters/cursor.rs:229` and `src/compile.rs:498`. Cursor's answer differs from Claude's, which is why the two providers need separate handling and why this is a separate package from `claude-session-start`.

**Driver:** `human-act`. A person drives Cursor; the answer is counted from the capture.

## Running it

```sh
experiments/cursor-session-start/probe.sh
```

1. Open Cursor on the printed workspace
2. Fully quit and reopen Cursor — it reads `hooks.json` at start
3. Submit a prompt in a new conversation
4. Fully quit Cursor, reopen it, and **resume that same conversation** from the conversation list
5. Submit another prompt in the resumed conversation

## The assertion is a count, scoped to after the first prompt

```json
"projection": "[.[] | .hook_event_name] as $n | ($n | index(\"beforeSubmitPrompt\")) as $i | [$n[$i + 1:][] | select(. == \"sessionStart\")] | length",
"expected": 0
```

The finding is that **no second `sessionStart` arrives**. Counting is how an absence becomes machine-checkable — it saves a human from having to attest to a negative.

**The window matters as much as the count.** The procedure launches Cursor three times, and Cursor may open a conversation at launch on its own. Counting `sessionStart` across the whole capture would then see two legitimate firings and record `refuted` — the harness's _strong_ signal, raised against a finding that is true and shipped. Scoping to everything after the operator's first prompt asks exactly the question the probe is about: once you were in a conversation, did resuming start another one?

**`wait_for` counts prompts, not session starts.** A poll cannot wait for a payload that never comes, so waiting on `sessionStart` would hang until timeout precisely when the finding holds. Two `beforeSubmitPrompt` payloads mean the operator submitted before quitting and again after resuming, which is the procedure's completion signal.

That is also why `beforeSubmitPrompt` is registered alongside `sessionStart`: it is what makes this probe possible at all.

## Prior context, not a current result

`1` is the answer a hand-run experiment produced before this package existed. It lives here as the manifest's `expected` value — the hypothesis this probe tests, not a claim that it still holds.

The finding is shipped and load-bearing. It is stated at [`docs/hooks-canonical.md:164`](../../docs/hooks-canonical.md) — "Cursor's `sessionStart` fires only on initial conversation creation, not on conversation resume" — and encoded as `Adapter::session_start_fires_on_resume` returning `false` for Cursor (`src/adapters/cursor.rs:229`), which `src/compile.rs:498` reads to warn when a canonical `session_start` hook targets both providers. The Cursor version that comment cites is 3.2.21.

## The assertion discriminates

Validated against three synthetic captures:

| Capture | Projection |
| --- | --- |
| `sessionStart`, prompt, prompt — the finding holds | `0` |
| `sessionStart`, `sessionStart`, prompt, prompt — an extra conversation before the first prompt | `0` |
| `sessionStart`, prompt, `sessionStart`, prompt — the resume **did** fire | `1` |

Distinct values for distinct inputs, and the contaminating case projects the same as the clean one — which is the property the window was added for.

The counting form matters for a second reason: a probe asserting "no second firing" against a provider that _did_ fire sees `1`, an observable value, rather than a silence indistinguishable from a broken hook. That is what keeps this from being an assertion on absence of error.

## The one assumption

**It trusts the operator followed the quit-and-resume procedure.** Two prompts in a single unresumed session would produce an identical capture. Every human-driven probe rests on its procedure being followed; this one just depends on it more visibly, because the evidence for "a resume happened" is the operator's word rather than a field in the payload.

## Depth

`depth: null` — a finding about hook firing semantics, which is off the config-rendering chain rather than a weaker point on it.

## Version source

`version_source.kind: "capture"`. Cursor's version arrives in the payload as `cursor_version`, the field agentspec itself trusts for host detection (`src/hooks_canonical.rs:250-256`). `just probe-status` therefore computes no version drift for this package.
