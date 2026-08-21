# Cursor — plain-stdout context injection on `sessionStart` (Gate #21)

**Question.** If a Cursor `sessionStart` hook emits plain text to stdout (no JSON envelope) and exits 0, does Cursor inject that text into the agent's context, the way Claude does?

**Why it matters.** A survey of real-world hook usage found plain-stdout context injection is the dominant Claude `SessionStart` pattern — roughly 60% of observed scripts. If Cursor accepted the same shape, the canonical context-injection path would simplify materially: no JSON envelope, no field-name asymmetry to bridge.

**Driver:** `manual`. A person must drive Cursor for the probe to run. Whether the agent actually knew the planted fact is also a person's call, which is why the assertion is an option set.

## Running it

```sh
experiments/cursor-gate-21-plain-stdout/probe.sh
```

The runner waits until the hook has fired before prompting. The original script named "the hook didn't fire at all" as a third failure mode; polling detects that, so the option set covers only what a person can uniquely answer.

## What the answer drives

**This is the finding behind a shipped design decision.** Canonical context injection on Cursor uses the `additional_context` JSON envelope _because_ plain stdout is rejected. That reasoning existed nowhere in this repository before this package — it lived only in a thoughts document — which is precisely the defect the harness exists to fix.

It is also why this probe is worth re-running rather than retiring. If a re-run answers `injected`, a shipped design decision is wrong and the harness has done exactly the job it exists for.

## The option set discriminates

| Option | Meaning |
| --- | --- |
| `injected` | Plain stdout reaches the agent's context; the `additional_context` envelope is unnecessary on Cursor |
| `not-injected` | Plain stdout is dropped; the JSON envelope is required |
| `couldnt-tell` | Records `inconclusive` |

The two substantive options are mutually exclusive and exhaust what the probe can produce, because the third outcome — the hook never firing — is machine-detected before the prompt appears.

## Prior context, not a current result

The 2026-05-10 run answered **`not-injected`**, against Cursor 3.2.21. That is the manifest's `expected` value: the hypothesis this probe tests, not a claim that it still holds. `results/` holds what is current.

## Marker phrase

The hook prints `AGENTSPEC_GATE21_CONTEXT_MARKER: The user owns a hamster named Quizzlebottom-2026.` The fact is deliberately absurd and unguessable, so an agent that produces it cannot have done so by chance — the same principle as the non-default-value rule, applied to a context payload.

Start a **fresh** conversation, not a resume: `sessionStart` does not fire on resume, which is what the `cursor-session-start` package measures.
