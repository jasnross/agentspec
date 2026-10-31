# Cursor — `[effort=…]` bracket options on subagents

**Question.** Does Cursor parse a `[effort=…]` bracket option in subagent frontmatter and apply it to the resolved model?

**Why it matters.** It establishes that `effort` is the wire key and that bracket options reach the resolved model, which is what any `CursorPreset.effort` rendering would depend on.

**Driver:** `manual`. A person must drive Cursor for the probe to run. The answer arrives in a hook payload and no person has to interpret it — the assertion, `_assertion` in `probe.py`, reads it from the capture.

## Running it

```sh
experiments/probe run cursor-subagent-effort
```

One blocking invocation. It prints a workspace path and three instructions, then polls the capture and records automatically — no second command and no keypress. If your terminal closes before the capture arrives, the workspace is kept for inspection, but the run is over: re-run the probe.

The workspace is a generated scratch directory, never a real project. `agentspec sync` owns `.cursor/hooks.json` through `_agentspec_id` tracking (`CursorAdapter::entry_to_json` and `CursorAdapter::tidy_hooks` in `src/adapters/cursor.rs`), so running this in a synced workspace could collide with that merge logic. Generating the workspace makes the collision structurally impossible rather than merely warned against.

## The oracle

Cursor exposes no offline introspection command. The oracle is its hook system: a hook that dumps raw stdin, plus hand-authored subagent definitions in a generated workspace. `subagentStart` carries `subagent_model`, the model id Cursor resolved for the subagent. Only `subagentStart` is registered — a narrower registration means less capture noise for the assertion to read past.

Each arm is a `.cursor/agents/<name>.md` differing only in its `model:` line. The resolved model per arm is read with:

```sh
jq -r 'select(.hook_event_name=="subagentStart") | "\(.subagent_type)\t\(.subagent_model)"' payloads.jsonl
```

That expression is where the two field names this package's `wait_for` and assertion both depend on — `subagent_type` and `subagent_model` — were first read off a real payload. Nothing else in this repository records where they came from.

**It is a hand-run read of a capture file, not part of the probe.** It runs once per line of `payloads.jsonl`. The probe's own expressions are Python callables in `probe.py`, each of which receives the whole capture as a sequence of parsed lines. The definition's `wait_for`, `_wait_for`, is true once any line is a `subagentStart` whose `subagent_type` is `arm-effort-low`. The assertion, `_assertion`, returns the `subagent_model` of the first such line, and `null` when there is none or that payload carries no `subagent_model`.

## Two model-bearing surfaces, at different fidelities

_Prior observation, from the same hook oracle but not from this probe: Cursor 3.15.19 and 3.16.17, 2026-08-14 and 2026-08-15. This package registers `subagentStart` only, so nothing below is measured by running it._

| Surface | Field | Fidelity |
| --- | --- | --- |
| `beforeSubmitPrompt` (parent turns only) | `model_id` + `model_params` | **Structured** — `[{"id":"effort","value":"high"}]` |
| `subagentStart` | `subagent_model` | **Flattened** — model id fused with resolved options |

`model` on a parent turn is the flattened form of `model_id` + `model_params`: `composer-2.5` + `fast=true` renders as `composer-2.5-fast`; `grok-4.6` + `effort=high,fast=true` renders as `cursor-grok-4.6-high-fast`.

**`model_params` is not available for subagents.** Subagent events expose only the flattened `subagent_model`, which is why this probe reads a fused string and must use non-default values to see anything at all.

Independent corroboration of the key name, from a parent turn on a different model family:

```json
{
  "hook_event_name": "beforeSubmitPrompt",
  "model_id": "grok-4.6",
  "model": "cursor-grok-4.6-high-fast",
  "model_params": [
    { "id": "effort", "value": "high" },
    { "id": "fast", "value": "true" }
  ]
}
```

That payload also shows multiple options composing, which is the shape a general model-options map would render to.

## The assertion discriminates

The table below was measured by hand on 2026-08-14 and 2026-08-15 against Cursor 3.15.19 and 3.16.17, before this package existed. The baseline and the first row are now the package's two fixture variants; the other rows stay as history.

`fixtures/discriminator/` is the committed baseline arm. It differs from `fixtures/assertion/` only in `.cursor/agents/arm-effort-low.md`, whose `model:` is the bare `claude-opus-5`. It keeps the filename because `wait_for` and the assertion select on `subagent_type`, which Cursor derives from the filename when the agent has no `name:`. On 2026-10-22, against Cursor 3.26.16, it ran as a dry run, and the record the run printed had `status` `refuted` and `observed` `"claude-opus-5-thinking-high"`.

The value differed from `expected`, and it is a real resolved model rather than `null`, so the assertion matched the subagent and read a distinct value. The other arms below are reproducible by editing `model:` in a copy of `fixtures/assertion/.cursor/agents/arm-effort-low.md`.

Baseline for `claude-opus-5` with no bracket options is `claude-opus-5-thinking-high`.

| Declared `model:` | Resolved `subagent_model` | Reading |
| --- | --- | --- |
| `claude-opus-5[effort=low]` | `claude-opus-5-thinking-low` | bracket options are parsed and applied |
| `claude-opus-5[effort=nonsense]` | `claude-opus-5-thinking-high` | an invalid value degrades silently to the default |
| `claude-opus-5[effort=high]` | `claude-opus-5-thinking-high` | **uninformative** — collides with the model's default |
| `composer-2.5[fast=false]` | `composer-2.5` | **uninformative** — `fast=false` is the subagent default |

Distinct inputs produce distinct values, so the assertion discriminates.

**The arm value is deliberately non-default.** `effort=low` differs from the `claude-opus-5-thinking-high` baseline, so a match is positive proof. Cursor's own documented example, `claude-opus-5[effort=high]`, is a default collision on that model and would pass whether or not the option was honored.

Two further arms from the same session establish that `thinking` is not an input key (`claude-opus-5[thinking=low]` resolves to the default) and that an unrecognized key is not rejected either (`claude-opus-5[bogus=nonsense]` also resolves to the default).

The committed samples are one capture per fixture variant and one prefix. `confirming.json` is an `assertion`-variant capture on Cursor 3.24.12, and it equals `expected`. `refuting.json` is a `discriminator`-variant capture on Cursor 3.26.16, which reproduces the baseline and differs from `expected`. `before-subagent-start.json` is a capture cut with `--lines 0`, before the awaited `subagentStart`, which makes it `wait_for`'s false side; with no payload in it, the assertion and the capture version are both `null`.

| Sample | Origin | `assertion` | `wait_for` | `capture_version` |
| --- | --- | --- | --- | --- |
| `before-subagent-start.json` | `prefix` | `null` | `false` | `null` |
| `confirming.json` | `capture` | `"claude-opus-5-thinking-low"` | `true` | `"3.24.12"` |
| `refuting.json` | `capture` | `"claude-opus-5-thinking-high"` | `true` | `"3.26.16"` |

`just probe-test` re-proves every result on every run.

## Limits of this oracle

- **The flattened `subagent_model` hides default-valued options.** An option whose value equals the model's default is indistinguishable from no option at all. This invalidated two probe arms and is the single most important constraint when re-running: use non-default values.
- **`status: "completed"` is not evidence of execution.** Every probe subagent reported `completed` with `tool_call_count: 0` and `message_count: 0`. Resolution is confirmed; an outbound request carrying the resolved option is not.
- **Cursor's provider request is unobservable in principle.** All traffic, BYOK included, routes through Cursor's own backend, so the final hop of the chain is unreachable by any oracle Cursor exposes. This probe therefore stops at `resolved-config` depth and cannot go further.
- Behavior was consistent across two Cursor versions, but neither is pinned by anything, and Cursor's SDK documentation states that legal option values vary by model and account.

## Version source

This package declares `version_source.kind: "capture"`, and the definition's capture version, `_capture_version` in `probe.py`, reads the first `cursor_version` among the capture's payloads. The version that matters is the IDE's, which arrives in the payload as `cursor_version` — the field agentspec itself trusts for host detection (`detect_provider` in `src/hooks_canonical.rs`). `cursor-agent --version` reports a different artifact on a different scheme, so `just probe-status` computes no drift for this package.
