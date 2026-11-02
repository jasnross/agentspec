# Claude Code — does `SessionStart` fire again on resume?

**Question.** Does Claude Code's `SessionStart` hook fire again when a session is resumed?

**Why it matters.** It drives shipped adapter behavior: a hook registered on `SessionStart` runs twice across a resumed session, so anything it does must be idempotent.

**Driver:** `manual`. A person must drive Claude for the probe to run at all. The answer still lands in the capture and nobody has to interpret it — that is the assertion's shape, a function of the capture, not the driver's business.

## Running it

```sh
experiments/probe run claude-session-start
```

The procedure has two halves, and the assertion depends on completing both:

1. `cd` into the printed workspace and run `claude`
2. Submit one prompt, then exit
3. Relaunch with `claude --resume` (or `-c`)
4. Submit another prompt

The script records automatically once the resume fires.

## The assertion is a pattern, scoped to after the first prompt

The assertion, `_assertion` in `probe.py`, finds the first `UserPromptSubmit` in the capture and returns the `source` of every `SessionStart` after it, in capture order. With no `UserPromptSubmit`, every line after the first counts. A line the capture hook could not parse as JSON, its `{"unparsed": …}` fallback, still holds its position in the capture, so it can shift the window but contributes no value. The manifest's `expected` is `["resume"]`.

Asserting the list of `source` values after the operator's first prompt reads as: once you were in a session, did resuming fire `SessionStart` again, and with what `source`?

**The window is what keeps a mistyped command from producing a false result.** Asserting `["startup", "resume"]` across the whole capture would break on a single extra `claude` launch before the resume — a crashed session, a typo — yielding `["startup","startup","resume"]` and recording `refuted`. That is the harness's _strong_ signal, and raising it against a finding that is true and already shipped costs a real investigation.

**`wait_for` gates on a `resume` source specifically.** The definition's `wait_for`, `_wait_for` in `probe.py`, is true once any line is a `SessionStart` whose `source` is `resume`. A `wait_for` accepting any `SessionStart` would be satisfied by the _startup_ payload the moment Claude launches — the runner would stop polling before the resume happened, assert on a capture with no resume in it, and record a false `refuted` against a finding that is true and already shipped.

## Prior context, not a current result

`["startup", "resume"]` is the answer a hand-run experiment produced before this package existed; scoped to after the first prompt, it lives here as the manifest's `expected` value, `["resume"]` — the hypothesis this probe tests, not a claim that it still holds. `results/` holds what is current, and `just probe-status` reports on it.

The finding is already shipped, which is what makes it worth re-establishing rather than dropping. It is stated under [`docs/hooks-canonical.md` § Session-start asymmetry](../../docs/hooks-canonical.md#session-start-asymmetry) — "Claude's `SessionStart` fires on both" — and encoded as `Adapter::session_start_fires_on_resume` returning `true` for Claude, which `compile_specs`'s parity gate in `src/compile.rs` reads to emit a cross-provider portability warning.

**That the two providers disagree here is the whole point.** Cursor's `sessionStart` fires only on initial conversation creation, so a single canonical `session_start` hook cannot achieve resume parity. This package measures the Claude half; `cursor-session-start` measures the other.

## The assertion discriminates

Validated against three synthetic captures:

| Capture | Assertion |
| --- | --- |
| startup, prompt, resume, prompt — the finding holds | `["resume"]` |
| startup, startup, prompt, resume, prompt — an extra launch before the first prompt | `["resume"]` |
| startup, prompt, prompt — the resume did **not** fire | `[]` |

Distinct values for distinct inputs, and the contaminating case yields the same as the clean one — the property the window was added for.

The value is non-default in the sense the contract requires: if `SessionStart` did not fire on resume the assertion yields `[]`, a different and equally observable result, so a pass is positive proof rather than a reading of an absence. The cross-provider asymmetry is the sharpest discriminator of all — Cursor's assertion counts `0` firings after the first prompt where Claude's yields one `resume`.

On 2026-10-22, against Claude Code 2.1.287, the `discriminator` fixture variant ran as a dry run. It changes one file, `.claude/settings.json`, whose `SessionStart` entry registers a second command, `{{CAPTURE_SCRIPT}} second`, beside the first. Claude runs every matching hook in parallel and runs an identical handler only once, so a command whose text differs by a trailing argument fires separately, and a real resume yields two `resume` payloads. The session finished after the runner's one-hour wait had expired, so the run kept its workspace without judging it, and a dry-run `record` re-judged that workspace once the capture had arrived: `status` `refuted`, `observed` `["resume","resume"]`.

The value differed from `expected`. This discriminator is a perturbation rather than a finding: it shows the assertion collects every `SessionStart` after the first prompt rather than stopping at one. It cannot show a resume that fires no `SessionStart`, because `wait_for` releases only on a `resume` payload.

`confirming.json` is an `assertion`-variant capture: startup, prompt, resume. `refuting.json` is a `discriminator`-variant capture, in which both registered commands fire at startup and again on resume. `before-resume.json` is the confirming capture cut with `--lines 2`, before the awaited `resume` payload, which makes it `wait_for`'s false side. The version source is the `claude --version` command, so no sample carries a capture version.

| Sample | Origin | `assertion` | `wait_for` |
| --- | --- | --- | --- |
| `before-resume.json` | `prefix` | `[]` | `false` |
| `confirming.json` | `capture` | `["resume"]` | `true` |
| `refuting.json` | `capture` | `["resume","resume"]` | `true` |
| `edited-no-prompt.json` | `edited` | `["resume"]` | `true` |
| `edited-unparsed-line.json` | `edited` | `["resume"]` | `true` |
| `edited-unparsed-no-prompt.json` | `edited` | `["startup","resume"]` | `true` |

`edited-no-prompt.json` removes every `UserPromptSubmit`, so every line after the first counts and the startup `SessionStart` is still skipped. `edited-unparsed-line.json` puts an `{"unparsed": …}` line ahead of the capture, which must keep its position for the slice to start after the prompt. `edited-unparsed-no-prompt.json` is that sample with its `UserPromptSubmit` removed: with no prompt the unparsed line is the first line, the one skipped, so the startup `SessionStart` counts. It is the sample that catches the definition's wrong version, `_drops_unparsed_lines`, which drops unparsed lines before indexing and so skips the startup instead.

The confirming sample equals `expected`, and the refuting sample differs from it. `just probe-test` re-proves every result on every run.

## Registrations

Both `SessionStart` and `UserPromptSubmit` are registered. `SessionStart` carries the finding; `UserPromptSubmit` proves hooks are live at all, which is what tells you whether a missing resume payload means "Claude didn't fire it" or "the hook was never wired up."

## Depth

`depth: null`. This is a finding about hook _firing semantics_, which is not a weaker point on the config-rendering chain — it is off that chain entirely. Claiming a position on a chain the evidence never touched would be a category error.

## Claude's `effort:` stays unprobed

This is the repository's only Claude probe. Claude's `effort:` frontmatter rendering is asserted from vendor documentation alone and is deliberately **not** probed: its rendering is an independent typed field with a documented closed enum and no string composition, so a probe there can only confirm. That is the sequencing rule applied — probe before implementation when the outcome could change the design, defer when it can only confirm — not an oversight. Reaching it would need a recording proxy, which is tracked separately.
