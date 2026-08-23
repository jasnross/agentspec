# Claude Code — does `SessionStart` fire again on resume?

**Question.** Does Claude Code's `SessionStart` hook fire again when a session is resumed?

**Why it matters.** It drives shipped adapter behavior: a hook registered on `SessionStart` runs twice across a resumed session, so anything it does must be idempotent.

**Driver:** `manual`. A person must drive Claude for the probe to run at all. The answer still lands in the capture and nobody has to interpret it — that is the assertion's shape, a projection, not the driver's business.

## Running it

```sh
experiments/claude-session-start/probe.sh
```

The procedure has two halves, and the assertion depends on completing both:

1. `cd` into the printed workspace and run `claude`
2. Submit one prompt, then exit
3. Relaunch with `claude --resume` (or `-c`)
4. Submit another prompt

The script records automatically once the resume fires.

## The assertion is a pattern, scoped to after the first prompt

```json
"projection": "[.[] | .hook_event_name] as $n | ($n | index(\"UserPromptSubmit\")) as $i | [.[$i + 1:][] | select(.hook_event_name == \"SessionStart\") | .source]",
"expected": ["resume"]
```

Asserting the list of `source` values after the operator's first prompt reads as: once you were in a session, did resuming fire `SessionStart` again, and with what `source`?

**The window is what keeps a mistyped command from producing a false result.** Asserting `["startup", "resume"]` across the whole capture would break on a single extra `claude` launch before the resume — a crashed session, a typo — projecting `["startup","startup","resume"]` and recording `refuted`. That is the harness's _strong_ signal, and raising it against a finding that is true and already shipped costs a real investigation.

**`wait_for` gates on `source == "resume"` specifically.** A filter matching any `SessionStart` would be satisfied by the _startup_ payload the moment Claude launches — the runner would stop polling before the resume happened, project a one-element list, and record a false `refuted` against a finding that is true and already shipped.

## Prior context, not a current result

`["startup", "resume"]` is the answer a hand-run experiment produced before this package existed, and it lives here as the manifest's `expected` value — the hypothesis this probe tests, not a claim that it still holds. `results/` holds what is current; until this probe is run, `just probe-status` correctly shows no row for it.

The finding is already shipped, which is what makes it worth re-establishing rather than dropping. It is stated under [`docs/hooks-canonical.md` § Session-start asymmetry](../../docs/hooks-canonical.md#session-start-asymmetry) — "Claude's `SessionStart` fires on both" — and encoded as `Adapter::session_start_fires_on_resume` returning `true` for Claude, which `compile_specs`'s parity gate in `src/compile.rs` reads to emit a cross-provider portability warning.

**That the two providers disagree here is the whole point.** Cursor's `sessionStart` fires only on initial conversation creation, so a single canonical `session_start` hook cannot achieve resume parity. This package measures the Claude half; `cursor-session-start` measures the other.

## The assertion discriminates

Validated against three synthetic captures:

| Capture | Projection |
| --- | --- |
| startup, prompt, resume, prompt — the finding holds | `["resume"]` |
| startup, startup, prompt, resume, prompt — an extra launch before the first prompt | `["resume"]` |
| startup, prompt, prompt — the resume did **not** fire | `[]` |

Distinct values for distinct inputs, and the contaminating case projects the same as the clean one — the property the window was added for.

The value is non-default in the sense the contract requires: if `SessionStart` did not fire on resume the projection is `[]`, a different and equally observable result, so a pass is positive proof rather than a reading of an absence. The cross-provider asymmetry is the sharpest discriminator of all — Cursor's equivalent capture projects `0` firings where Claude's projects one.

## Registrations

Both `SessionStart` and `UserPromptSubmit` are registered. `SessionStart` carries the finding; `UserPromptSubmit` proves hooks are live at all, which is what tells you whether a missing resume payload means "Claude didn't fire it" or "the hook was never wired up."

## Depth

`depth: null`. This is a finding about hook _firing semantics_, which is not a weaker point on the config-rendering chain — it is off that chain entirely. Claiming a position on a chain the evidence never touched would be a category error.

## Claude's `effort:` stays unprobed

This is the repository's only Claude probe. Claude's `effort:` frontmatter rendering is asserted from vendor documentation alone and is deliberately **not** probed: its rendering is an independent typed field with a documented closed enum and no string composition, so a probe there can only confirm. That is the sequencing rule applied — probe before implementation when the outcome could change the design, defer when it can only confirm — not an oversight. Reaching it would need a recording proxy, which is tracked separately.
