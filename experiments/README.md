# Provider probes

A **probe** measures what a provider actually does with a rendering, in that provider's own resolved view. agentspec's unit and integration tests compare emitted bytes against agentspec's belief about what each provider reads; nothing in the test suite checks that belief against the provider itself, and the belief has been wrong.

This directory is where that check lives. Each package holds one probe: its fixtures, its runner, and its results.

## The workflow: probe before you implement

When you are about to make agentspec emit a new rendering:

1. Check whether a probe already answers whether the provider honors it. Grep the `question` field of every manifest: `jq -r '.question' experiments/*/probe.json`.
2. If none does, write one and run it.
3. Design against the measurement.

Probes verify the _provider's_ contract, so they use hand-authored provider config files rather than `agentspec compile` output. Nothing here requires an agentspec feature to exist first, which is why a probe can precede the change it de-risks instead of gating it afterward.

**Probe first when the outcome could change the design; defer when it can only confirm.** A rendering with no records has simply not been measured. This harness deliberately keeps no list of what ought to be — it reports measurements, and "unprobed" is a state that lasts exactly as long as it takes to write the probe.

## Authoring a new probe

1. **Pick the driver.** `script` if a command answers the question offline. `human-act` if a human must drive the provider but the answer lands in a file. `human-judge` if a human must drive the provider _and_ answer the question.
2. **Create the package.** `experiments/<probe-name>/`, where the directory name is the probe's identity. No file restates it — `probe-status` reads records by path and already knows it.
3. **Write the fixtures** under `fixtures/`. For a human-driven probe, that includes a capture hook that appends each payload as one JSON line to `capture/payloads.jsonl`. Use `{{PLACEHOLDER}}` for anything absolute; `probe_template_file` fills them in at _Arrange_ so no operator ever hand-edits a path.
4. **Author the assertion** — a jq projection with an expected value, or an option set with an expected id.
5. **Validate that it discriminates** (see below). This is a required step, not a nicety.
6. **Run it.** The runner writes the record.
7. **Commit the fixtures, the manifest, the README, and the record.**

A manifest is written only **once its assertion has been validated to discriminate**. No manifest is authored speculatively for a probe that has never run. A package with fixtures but no runnable assertion carries a README and its fixture tree and nothing else: `probe-run` skips any directory without a `probe.json`, and `probe-status` shows no row for a package with no records.

### The two rules that replace a control arm

There is no control arm in this harness. Two rules do its work instead.

**Expected values must be non-default.** A pass must be positive proof, not a possible reading of the provider's default. Cursor's own documented example, `claude-opus-5[effort=high]`, is a default collision on that model — an arm that would pass whether or not the option was honored.

**An assertion must be shown to discriminate, once, at authoring time.** For a projection, show it returns distinct values for two different inputs; for an option set, show the options are mutually exclusive and exhaust the outcomes the probe can produce. Paste the evidence into the package README. Without this, a misspelled jq path returns `null` forever and nobody notices.

**Every human-judged option set includes an explicit "couldn't tell" option** mapping to `status: "inconclusive"`. It is the same principle applied to a human oracle: without it, a tired operator picking the first plausible option produces a false pass. The runner also machine-checks whatever it can _before_ asking, so the human answers only the question no machine can.

## Why absence of error proves nothing

All three providers degrade silently on a rendering they do not understand:

- **OpenCode** looks an unrecognized variant up in a per-model map and collapses the miss to `{}` via `mergeOptions` — no error, no warning, no log line. _(Uncited: this behavior was established by reading provider source, but no line reference was recorded. Wants a source.)_
- **Claude** clamps an unsupported effort level down to the highest supported one at or below it, without warning.
- **Cursor** falls back to the parent conversation's model when a model specification fails to resolve, and silently ignores unrecognized model options — neither an unrecognized option key nor an unrecognized option value is rejected.

A probe must therefore assert on a **positive signal** — the setting is present and correct in the provider's own resolved view — never on the absence of an error.

Relatedly, OpenCode's `Provider.parseModel` splits on `/` only (`packages/opencode/src/provider/provider.ts:1997-2003`); no `#` parser exists anywhere in that repository, so a `#variant` suffix produces a well-formed model id that no catalog contains.

These three behaviors were established by reading provider source, so no runner can regenerate them. They are the reasoning behind the methodology rather than measurements, which is why they are prose here and not records.

## Depth

Verification depth is recorded **per result**, because the available oracles are not equivalent and calling them all "verified" would flatten a real difference. The chain is:

```
preset config → emitted bytes → provider parses file → provider's resolved config → outbound model request
```

agentspec's own tests cover the first hop only. Each record states how far along the chain its evidence actually reached — or `null`, when the finding is not on this chain at all.

## Package layout

```
experiments/
  README.md                   # this file — the probe contract
  lib/
    probe-common.sh           # Arrange helpers: workspace, polling, prompting
    record.sh                 # Assert & record — the single record writer
    probe-status.sh           # reads records, derives the report; invokes no probe
    tests/*.bats              # bats coverage for the three above
  <probe-name>/
    README.md                 # question, procedure, discriminates evidence, oracle limits
    probe.json                # the manifest — the only hand-authored contract file
    fixtures/                 # provider config the runner materializes
    probe.sh                  # the runner
    results/*.json            # records, one per run, append-only
```

**One package holds exactly one probe, and one probe answers one question.** A package needing two answers is two packages.

**A package never depends on another package.** A fixture two probes both need is copied, not shared.

**A package with no runnable assertion may shape its fixtures however its apparatus requires.** `fixtures/` is the layout the shared runner materializes from, so it is mandatory only for packages that have a runner. The two blocked plugin gates ship `probe-plugin/` and `manifest-variants/` at the package root instead, because a Cursor plugin has to be installed where Cursor looks for plugins rather than copied into a temp workspace. Whoever writes their runners moves those trees under `fixtures/` at that point.

## The manifest (`probe.json`)

The only hand-authored file in the contract.

| Field | Meaning |
| --- | --- |
| `schema_version` | `1`. |
| `provider` | `claude`, `cursor`, or `opencode`. |
| `driver` | `script`, `human-act`, or `human-judge`. Read only by `probe-run`, which is why it lives here and never on a record. |
| `depth` | `emitted-bytes`, `provider-parses`, `resolved-config`, `outbound-request`, or `null`. |
| `question` | Required. What this probe answers, in one sentence. |
| `version_source` | Where the tool version comes from. See below. |
| `wait_for` | Optional jq filter the runner polls the capture against. Present on every `human-act` and `human-judge` manifest. |
| `assertion` | Either `{projection, expected}` or `{options, expected}`. Never both. |

**`depth: null` is the right answer for a finding that is not on the config-rendering chain at all.** Output handling and hook-firing semantics are not weaker points on that chain; they are off it, and claiming a position on a chain the evidence never touched is a category error.

**`question` appears only at the top level.** A human-judged probe shows this same string to the operator rather than carrying a second copy inside `assertion`, and no assertion restates it.

**There is no timeout field.** The poll timeout is a runner constant with a `PROBE_TIMEOUT_SECONDS` override, because it is a property of the operator's session rather than of the probe.

### `version_source`

| `kind`    | Extra field | Behavior                                     |
| --------- | ----------- | -------------------------------------------- |
| `command` | `command`   | Run it; take the first line.                 |
| `capture` | `jq`        | Apply the expression to the capture payload. |
| `none`    | —           | Record `null`.                               |

**A `command` is split on whitespace and exec'd directly — never through a shell.** So `opencode --version` works and `sh -c "…; …"` does not: `;`, `|`, `>`, `$()`, and quoting are not interpreted. `just probe-status` runs these commands on every `just check`, and a manifest is reviewed as data rather than as code, so it must not be able to express arbitrary code. The command also runs with stdin closed and under a timeout, because a version command that blocks would wedge the build — which is worse than failing it.

**A Cursor probe declares `kind: "capture"` and never a CLI command.** The probe exercises the IDE, whose version arrives in the hook payload as `cursor_version` — the field agentspec itself trusts for host detection (`src/hooks_canonical.rs:250-256`). `cursor-agent --version` reports a different artifact on a different versioning scheme (`2026.05.28-a70ca7c`), so comparing against it would produce a drift signal about something no probe touched.

### Every jq expression reads slurped, array-shaped input

`payloads.jsonl` is JSONL, but the runner evaluates every capture-facing expression with `jq -s`. So `wait_for` filters read `any(.[]; …)` and `version_source.jq` reads `[.[] | .cursor_version] | map(select(. != null)) | first`. Writing one against a stream shape instead means it never matches, and the runner polls to timeout on a capture that already holds the answer.

A projection over a capture must reduce the array to the shape `expected` is written in. `record.sh` compares structurally, so an expression yielding a one-element array will never equal a string `expected` — and the mismatch records `refuted` after a live session rather than erroring.

### Option sets

```json
"question": "After the hook denied the command, which marker strings appeared?",
"assertion": {
  "options": [
    { "id": "both-markers", "text": "Both the user_message and agent_message markers were visible" },
    { "id": "user-only", "text": "Only the user_message marker was visible" },
    { "id": "neither", "text": "Neither marker was visible — only a generic deny" },
    { "id": "couldnt-tell", "text": "Could not determine", "status": "inconclusive" }
  ],
  "expected": "neither"
}
```

## The record (`results/<date>T<HHMMSS>-<provider>-<version>.json`)

**Every record is produced by a runner.** Nothing is hand-written, so there is one record shape and no exception to the never-hand-edit rule. Records are append-only: superseding a result means adding a newer file, never editing an existing one. `experiments/*/results/` is prettier-ignored so a probe run is the last thing that touches its own record.

Seven keys, every one consumed by `probe-status`:

| Key | Meaning |
| --- | --- |
| `schema_version` | Makes a future migration decidable. |
| `provider` | Groups the report. |
| `status` | `confirmed`, `refuted`, or `inconclusive`. |
| `depth` | Nullable; copied from the manifest. |
| `date` | UTC date of the run. |
| `tool_version` | Nullable. |
| `assertion` | `{projection, expected, observed}` or `{options, expected, observed}`. |

Fields that would merely restate a derivable fact — the probe's name, its driver, the capture's provenance — are absent, because storing a derivable fact means storing a way for it to disagree with its source.

**`blocked` is not a status.** A probe that cannot run makes its runner exit nonzero with a diagnostic, which produces no record at all. Recording "this could not run" would mean writing a record for a run that never happened. A blocked probe's state lives in its package README, where a process note belongs.

The filename carries a UTC time component because date plus version does not disambiguate a same-day re-run at the same provider version — and re-running to confirm a `refuted` result is the first thing anyone would do. The full name also sorts lexicographically in run order, which is how `probe-status` finds the newest.

## Running probes and reading the report

```sh
just probe-run      # run every script-driven probe; human-driven ones are listed as skipped
just probe-status   # report on the committed records; invokes no probe
just bats-test      # the harness test suite
just shellcheck     # lint the probe shell
```

`just check` ends with a one-line probe summary. That line can never fail the build.

**`probe-run` cannot drive a human-driven probe.** A `human-act` or `human-judge` package needs a live provider session, so `probe-run` lists it as skipped and points at its README; run its `probe.sh` directly.

### What the summary line can and cannot see

```
probes: 6 recorded · 0 refuted · 0 inconclusive · 1 version drift
```

The drift count is **not** a claim that everything else is current. Every Cursor probe declares `version_source.kind: "capture"`, and a captured version is knowable only by running the probe — which `probe-status` never does. So no drift is computable for those packages, and they contribute nothing to that count. Read `0 version drift` as "nothing computable drifted," not as "everything is current."

### The two drift signals are not equally strong

**Version drift** — the recorded `tool_version` differs from the installed one — is weak. It means only that the result has not been reconfirmed against the version you are running.

**Assertion drift** — a re-run produced a different `observed` — is strong. It means provider behavior changed. Since `probe-status` invokes no probe, it cannot produce assertion drift itself; it surfaces `refuted` records, which are assertion drift already recorded.

## What the freshness checks do and do not prove

A human-driven runner is one blocking invocation: it materializes the workspace, waits, and records. That is what makes a stale capture unreachable on the normal path — the workspace was created by the invocation still running, so there is no earlier run to point at.

`probe.sh --capture <workspace>` is the fallback for a terminal that closed mid-run, and it is the only path where an older workspace is reachable at all. There, `record.sh` requires four things: a run stamp exists, `payloads.jsonl` exists and is non-empty, the stamp's token appears in the payloads, and the payloads' mtime is at or after the stamp's epoch.

**Those checks prove the capture belongs to its own workspace and postdates it. They do not prove it came from today.** `record.sh` reads the stamp from the capture directory it is validating, so a self-consistent workspace from last week satisfies all four. What they do catch is a capture crossed with a different workspace's payloads, a truncated or empty capture, and payloads predating the workspace.

This is a deliberate limit, not an oversight: closing it would mean carrying an invocation token outside the capture, which buys nothing on the path everyone actually uses. **Resume promptly, or re-run the probe.** A re-run costs one live session; a wrong record costs more.

## Re-verification

When a captured result goes stale, the thing that aged is the third-party tool, not any file in this repository — so nothing here can be checked by CI, and no probe ever runs there. Re-verification is triggered by judgment: a provider version bump, a changed rendering, or a new provider surface.

A re-run that produces a different `observed` is **assertion drift**, and it is the strong signal — it means provider behavior changed. Treat it as a finding, not a failure: record it and raise it, rather than adjusting `expected` to match.
