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

## The assertion has three parts, and all three must hold

```json
"expected": { "resumed": true, "fired": true, "after_resume": 0 }
```

- **`resumed`** — both `beforeSubmitPrompt` payloads carry the same `conversation_id`. This is the machine check that the operator actually resumed rather than starting a second conversation.
- **`fired`** — `sessionStart` fired at least once _for that conversation_. The positive signal: the event demonstrably works here.
- **`after_resume`** — `sessionStart` fired zero times for that conversation after the first prompt. The finding itself.

Every part was added because a run without it produced a misleading verdict.

**Why `fired` exists.** A first run recorded `confirmed` from a capture containing _zero_ `sessionStart` payloads. The absence was satisfied, but the capture could not distinguish "resume does not fire it" from "the event never fires, or is misregistered" — a typo in the `hooks.json` event name yields a byte-identical capture, since `beforeSubmitPrompt` still fires and still proves the file loaded. That is the trap the contract opens with: assert on a positive signal, never on absence of an error.

**Why `resumed` exists.** A second run recorded `refuted` — and `refuted` is the harness's _strong_ signal, meaning shipped behavior changed. It had not. The capture showed the two prompts in different conversations (`064d52e7` and `095153f3`): two fresh conversations had been created and the second prompt went into a new one. Cursor behaved exactly as documented; the probe simply never observed a resume. The evidence was in the capture all along — `conversation_id` is on every payload — so the probe now reads it instead of trusting the procedure.

**Why both `fired` and `after_resume` are scoped to that conversation.** Cursor may open a conversation of its own on relaunch, firing a legitimate `sessionStart` for a different id. Counting unscoped would read that as the resume firing and record a false `refuted`.

**`wait_for` blocks recording until the procedure is genuinely complete:** two prompts, sharing one `conversation_id`, with a `sessionStart` seen for it. A procedure slip therefore makes the runner keep polling rather than manufacture a verdict — which is the right failure, because a false `refuted` against `src/adapters/cursor.rs:229` costs a real investigation.

If the runner times out, inspect the capture before assuming anything: differing `conversation_id`s on the two prompts mean the resume did not happen. To record a partial capture deliberately, resume with `probe.sh --capture <workspace>` — `record.sh` does not consult `wait_for`.

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

## The assumption that used to be here

This package once documented an assumption — that it trusted the operator to have resumed, since the evidence was "the operator's word rather than a field in the payload." That was wrong on the facts: `conversation_id` is a field in the payload, and the `resumed` term now checks it.

What remains unverifiable is narrower: that the operator _quit_ Cursor between the two prompts rather than staying in one continuous session. Two prompts in a single unresumed session share a `conversation_id` too. But that capture also contains no second `sessionStart`, which is the answer the probe reports — so the residual risk is confirming a true finding for a slightly wrong reason, not recording a false one.

## Depth

`depth: null` — a finding about hook firing semantics, which is off the config-rendering chain rather than a weaker point on it.

## Version source

`version_source.kind: "capture"`. Cursor's version arrives in the payload as `cursor_version`, the field agentspec itself trusts for host detection (`src/hooks_canonical.rs:250-256`). `just probe-status` therefore computes no version drift for this package.
