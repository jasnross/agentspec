# Provider probes

A **probe** measures what a provider actually does with a rendering, in that provider's own resolved view. agentspec's unit and integration tests compare emitted bytes against agentspec's belief about what each provider reads; nothing in the test suite checks that belief against the provider itself, and the belief has been wrong.

This directory is where that check lives. Each package holds one probe: its fixtures, its runner, and its results.

## The workflow: probe before you implement

When you are about to make agentspec emit a new rendering:

1. Check whether a probe already answers it: `jq -r '.question' experiments/*/probe.json`. Every package has a manifest, so this list is complete.
2. If nothing covers it, write a probe and run it.
3. Design against the measurement — and against what its `depth` licenses you to conclude.

**Absent from that list means nobody has measured it**, not that it works. OpenCode's _skill_ surface is the standing example of why that matters: `variant:`, `model:`, and `tools:` are all parsed and discarded there, while the agent surface honors them.

Probes verify the _provider's_ contract, so they use hand-authored provider config files rather than `agentspec compile` output. Nothing here requires an agentspec feature to exist first, which is why a probe can precede the change it de-risks instead of gating it afterward.

**Probe first when the outcome could change the design; defer when it can only confirm.** A rendering with no records has simply not been measured. This harness deliberately keeps no list of what ought to be — it reports measurements, and "unprobed" is a state that lasts exactly as long as it takes to write the probe.

## Authoring a new probe

1. **Pick the driver.** It names what running the package costs, which is what `probe-run` must be authorized to spend before it will execute it. `unattended` if a command answers the question with no credentials and no cost. `billed` if a command answers it but the run spends model quota, so it needs an explicit opt-in. `manual` if a person must drive the provider.
2. **Create the package.** `experiments/<probe-name>/`, where the directory name is the probe's identity. No file restates it — `probe-status` reads records by path and already knows it.
3. **Write the fixtures** under `fixtures/`. For a human-driven probe, that includes a capture hook that appends each payload as one JSON line to `capture/payloads.jsonl` — the payload as the provider sent it, with nothing added. Use `{{PLACEHOLDER}}` for anything absolute; `probe_template_file` fills them in at _Arrange_ so no operator ever hand-edits a path.
4. **Author the assertion** — a jq projection with an expected value, or an option set with an expected id.
5. **Validate that it discriminates** (see below). This is a required step, not a nicety. Do it with `record.sh --dry-run` against the manifest itself, so the expression being validated is the one the manifest actually carries rather than a copy retyped at the shell. Once a view exists on disk, `record.sh --manifest <probe.json> --view <saved-view> --dry-run` re-evaluates any candidate projection for free.
6. **Run it.** The runner writes the record.
7. **Commit the fixtures, the manifest, the README, and the record.**

**The manifest is authored first, and committed only once its assertion has been shown to discriminate.** Writing it first is what makes step 5 honest: `--dry-run` evaluates the expression the manifest actually carries, so the expression validated is the expression that will run. Hand-applying jq at the shell and transcribing the result into `probe.json` afterwards leaves two separately-typed strings, and a transcription slip that makes a projection return `[]` records `refuted` — a false finding rather than a loud failure. What the rule forbids is a _committed_ manifest whose assertion has never been shown to distinguish two inputs, not a draft one on disk.

### The two rules that replace a control arm

There is no control arm in this harness. Two rules do its work instead.

**Expected values must be non-default.** A pass must be positive proof, not a possible reading of the provider's default. Cursor's own documented example, `claude-opus-5[effort=high]`, is a default collision on that model — an arm that would pass whether or not the option was honored.

**An assertion must be shown to discriminate, once, at authoring time.** For a projection, show it returns distinct values for two different inputs; for an option set, show the options are mutually exclusive and exhaust the outcomes the probe can produce. Paste the evidence into the package README. Without this, a misspelled jq path returns `null` forever and nobody notices.

**The apparatus must be unreachable from what the provider can read.** A human-driven probe generates two sibling directories: `<ws>/project/`, which the operator opens the provider on, and `<ws>/capture/`, which holds the hook script and its payloads. The separation is not tidiness. When a probe's oracle is the agent's _answer_, a marker string sitting in a capture script inside the opened project is findable by filesystem search — and "the hook injected it" becomes indistinguishable from "the agent grepped for it." That is a control-arm failure, and it cost two live sessions before it was noticed: an agent asked whether it knew a planted fact answered correctly, having found the fact in the hook script rather than in its context.

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

**What each depth licenses you to conclude**, which is the part that matters when designing against a record:

| Depth | Proves | Does **not** prove |
| --- | --- | --- |
| `resolved-config` | the field reached the provider's own resolved view | that the provider **acts** on it; the value can still be dropped at request-build time |
| `outbound-request` | the setting reached the model provider | nothing further — this is the end of the chain |
| `null` | the finding is off this chain entirely (output handling, hook firing) | any position on it |

**The enum is shorter than the chain, deliberately.** `emitted-bytes` is the hop agentspec's own tests already cover, and `provider-parses` would be an assertion that no error occurred — which the rule above forbids. Naming either as a depth would invite a record claiming evidence the contract prohibits gathering, so `record.sh` and the bats suite both reject them.

The gap between `resolved-config` and `outbound-request` is the one that bites. `opencode debug agent` prints the _declared_ variant; OpenCode collapses an unrecognized one to `{}` later, when it builds the request. So a `confirmed` at `resolved-config` says the provider **read** your field — design on that, but do not claim the model saw it.

For Cursor that gap is permanent: all traffic including BYOK terminates in Cursor's own backend, so no Cursor probe can ever exceed `resolved-config`.

## Package layout

```
experiments/
  README.md                   # this file — the probe contract
  lib/
    probe-common.sh           # Arrange helpers: workspace, polling, prompting
    manifest-contract.sh      # the manifest's three enums, shared by record.sh and the tests
    record.sh                 # Assert & record — the single record writer
    probe-state.sh            # derives a package's freshness; shared by probe-run and probe-status
    probe-status.sh           # reads records, derives the report; invokes no probe
    tests/*.bats              # bats coverage for the above
  <probe-name>/
    README.md                 # question, procedure, discriminates evidence, oracle limits
    probe.json                # the manifest — the only hand-authored contract file
    fixtures/                 # provider config the runner materializes
    probe.sh                  # the runner
    results/*.json            # records, one per run, append-only
```

**One package holds exactly one probe, and one probe answers one question.** A package needing two answers is two packages.

**A package never depends on another package.** A fixture two probes both need is copied, not shared.

**A probe package has measured something.** Every directory here except `lib/` carries a manifest, a runner, and at least one record. A probe that cannot run, or that nobody has attempted, is intent rather than evidence: its question belongs in `TODO.md`, alongside every other unmeasured provider question.

## The manifest (`probe.json`)

The only hand-authored file in the contract.

| Field | Meaning |
| --- | --- |
| `schema_version` | `1`. |
| `provider` | `claude`, `cursor`, or `opencode`. |
| `driver` | `unattended`, `billed`, or `manual` — what running the probe requires. Read by `probe-run` to decide what it can execute, and by `record.sh` for the `--capture` requirement and the options correspondence below. Never on a record. |
| `depth` | `resolved-config`, `outbound-request`, or `null`. |
| `question` | Required. What this probe answers, in one sentence. |
| `version_source` | Where the tool version comes from. See below. |
| `wait_for` | Optional jq filter the runner polls the capture against. Present on every `manual` manifest. |
| `assertion` | Either `{projection, expected}` or `{options, expected}`. Never both. An `options` assertion additionally requires `driver: "manual"` — see below. |

**`depth: null` is the right answer for a finding that is not on the config-rendering chain at all.** Output handling and hook-firing semantics are not weaker points on that chain; they are off it, and claiming a position on a chain the evidence never touched is a category error.

**`question` appears only at the top level.** A human-judged probe shows this same string to the operator rather than carrying a second copy inside `assertion`, and no assertion restates it.

**There is no timeout field.** The poll timeout is a runner constant with a `PROBE_TIMEOUT_SECONDS` override, because it is a property of the operator's session rather than of the probe.

### `version_source`

| `kind`    | Extra field | Behavior                                     |
| --------- | ----------- | -------------------------------------------- |
| `command` | `command`   | Run it; take the first line.                 |
| `capture` | `jq`        | Apply the expression to the capture payload. |

**`kind: "none"` is deliberately absent from that table, but still accepted.** `record.sh` records a `null` version for it, and an omitted `version_source` falls back to it; the harness's own test fixtures declare it, which is how they avoid running a version command. It is unlisted rather than rejected because no committed manifest uses it, and adding a gate for a value nothing uses would be machinery earning nothing. The reason not to reach for it: a result carrying no tool version cannot drift, so `probe-status` has nothing to compare and the result is never reconfirmed against anything. If a version is genuinely unobtainable, say so in the package README.

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

**An options assertion requires `driver: "manual"`, and `record.sh` refuses a manifest that declares one without it.** An option set is a person choosing from a list, so running the probe requires a person present — which is exactly what `manual` declares and what neither other value can supply. An `unattended` or `billed` manifest declaring one describes a runner that would have to prompt an operator no scheduler put there. The implication runs one way only — a `manual` probe may still be machine-answered, which is what `claude-session-start` is, and its projection over a capture is perfectly legitimate.

This is what makes the runner's read safe: `probe_record_capture` decides whether to prompt from the assertion's shape rather than from `driver`, because the shape is the fact and the driver was a second copy of it. `manifest-contract.sh` holds the gate so `record.sh` and the bats suite cannot drift apart on it.

**An option may declare `status`, and `inconclusive` is the only legitimate value.** A declared status replaces the comparison against `expected` — that is how the couldn't-tell option yields `inconclusive` rather than `refuted`. Any other value would let a manifest fix the verdict in advance, which is the caller-supplied status the record contract exists to prevent; it would simply arrive through the manifest instead of the command line. Both `record.sh` and the bats suite reject it, so a committed manifest cannot carry one.

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

**`blocked` is not a status.** A probe that cannot run makes its runner exit nonzero with a diagnostic, which produces no record at all. Recording "this could not run" would mean writing a record for a run that never happened. Nor is there anywhere for such a status to live: a probe that cannot run has no package at all, so its question sits in `TODO.md` until someone can measure it.

The filename carries a UTC time component because date plus version does not disambiguate a same-day re-run at the same provider version — and re-running to confirm a `refuted` result is the first thing anyone would do. The full name also sorts lexicographically in run order, which is how `probe-status` finds the newest.

### `record.sh --dry-run`

`--dry-run` evaluates a manifest end to end — every gate, the projection, the structural comparison, version resolution, record assembly — and prints what a record would contain instead of writing one. No `results/` directory is created and no record is written. Version resolution still runs, so a manifest declaring `version_source.kind: "command"` still executes that command; what dry-run guarantees is that `record.sh` itself writes nothing, not that the run is side-effect-free.

**A runner takes no arguments, so `--dry-run` is not one of them.** Five of the six committed runners reject any argument outright. A runner that supports a dry run reads `PROBE_DRY_RUN=1` from the environment and passes the flag on to `record.sh` itself — the same shape as `PROBE_FIXTURE`. Where a runner does not read it, invoke `record.sh` directly against a view the runner already produced.

It exits **0 whatever status the comparison computes**, including `refuted`. Validating that an assertion discriminates means running it against an input it is supposed to refute, so a nonzero exit would fail the primary use case every time; it would also make dry-run disagree with real-run semantics, where `refuted` is a finding rather than a failure.

Every gate still fires. A manifest failing a manifest check, a `--capture` whose payloads are absent or empty, a projection yielding several values — all still refuse, because those are the wiring being verified.

**Its stdout is not a record.** It is what a record would contain, and it must never be redirected into `results/`: every record in this repository is produced by a runner, and that invariant is what makes hand-editing one a defect rather than a workflow. `--dry-run` exists to reduce the pressure to violate that rule, not to work around it.

## Running probes and reading the report

```sh
just probe-run                   # the free probes; costlier ones are listed with the flag that frees them
just probe-run --billed          # + the probes that spend model quota
just probe-run --manual          # + the probes that block on a live provider session
just probe-run --all             # every driver
just probe-run --stale --all     # every probe owed a run
just probe-status                # report on the committed records; invokes no probe
just bats-test      # the harness test suite
just shellcheck     # lint the probe shell
```

`just check` ends with a one-line probe summary. That line can never fail the build.

### Selection and authorization

`probe-run` decides each package with two independent questions, in this order:

|  | question | flags |
| --- | --- | --- |
| **selection** | is this package interesting to this run? | `--stale` |
| **authorization** | may this run pay what the package costs? | `--billed`, `--manual`, `--all` |

**Every probe is runnable by `probe-run`, including the manual ones.** All five route through `probe_human_run`, which arranges the workspace, prints the procedure, blocks until the capture lands, and records. So what separates the drivers is not capability — it is cost. An `unattended` probe costs nothing, a `billed` one spends model quota, and a `manual` one spends an afternoon of a person's attention. A run is therefore defined by what it is willing to spend, and the default spends nothing: a batch run that costs money or blocks for hours on every invocation is a batch run people stop invoking.

**Authorization is a set, not a level.** Each flag adds its own driver; `--billed` never implies `--manual` and neither implies the other. They stack in any order, repeat harmlessly, and `--all` is the union. `PROBE_AUTHORIZE_DRIVERS="billed manual"` seeds the same set for a caller that cannot pass arguments, and composes with flags rather than overriding them. The set is built from `MANIFEST_DRIVERS` in `manifest-contract.sh`, so a fourth driver costs one entry there plus its flag — not a new combination in the run loop, and not a hand-maintained line in the summary breakdown.

A withheld package is printed with the flag that would free it, and the summary counts by driver for the same reason: the only useful thing to tell a reader looking at a skip is what to type next.

**`--stale` narrows the run to the packages actually owed one.** A package is owed a run when it has never produced a readable record, or when its recorded `tool_version` is comparable against the installed one and differs. Everything else is passed over as `fresh`, with the reason printed, so the filter is auditable without running anything.

Selection is evaluated before authorization, so a fresh package is passed over whatever it would have cost — a `--stale` run does not ask you to authorize a probe whose answer cannot have changed. The two are otherwise independent: being owed a run is not permission to pay for one, so `--stale` alone reports a drifted `billed` package as needing `--billed`, and `--stale --billed` runs it.

**"Cannot compare" counts as fresh, not as owed.** A `capture`-sourced version — every Cursor package — is knowable only by running the probe, and a `none`-sourced one only by a human. Treating unknowable as owed would put those packages permanently in the run set, which is the same as having no filter at all. `probe-status` still shows them, annotated with why no comparison was possible. The two commands share one implementation of this judgement (`lib/probe-state.sh`) precisely so the report and the runner cannot come to disagree about what stale means.

`probe-run` accepts no argument beyond those four, and exits 2 on one — as does an unknown driver in `PROBE_AUTHORIZE_DRIVERS` — rather than silently ignoring a token that was meant to authorize spending.

### What the summary line can and cannot see

```
probes: 8 recorded · 0 refuted · 0 inconclusive · 1 version drift · 2 billed (drift not tracked)
```

The drift count is **not** a claim that everything else is current. Every Cursor probe declares `version_source.kind: "capture"`, and a captured version is knowable only by running the probe — which `probe-status` never does. So no drift is computable for those packages, and they contribute nothing to that count. Read `0 version drift` as "nothing computable drifted," not as "everything is current."

**Billed packages sit in their own segment, and their drift is deliberately uncounted.** A batch run never refreshes one, so its recorded version falls behind the installed one and stays there until somebody pays for a `--billed` run. Counted, that drift would be structurally unable to reach zero — and a count that can never reach zero is a muted alarm: it hides the genuine drift of a package a free batch run could clear. The row still shows the installed version and names what would refresh it. The segment is omitted entirely when no package declares `billed`.

### The two drift signals are not equally strong

**Version drift** — the recorded `tool_version` differs from the installed one — is weak. It means only that the result has not been reconfirmed against the version you are running.

**Assertion drift** — a re-run produced a different `observed` — is strong. It means provider behavior changed. Since `probe-status` invokes no probe, it cannot produce assertion drift itself; it surfaces `refuted` records, which are assertion drift already recorded.

## A human-driven run is one invocation

A human-driven runner materializes the workspace, prints the procedure, polls, and records — one blocking invocation with no resume path. That is what makes a stale capture unreachable rather than merely discouraged: the workspace was created by the invocation still running, so there is no earlier run to point at and nothing for a freshness check to prove. `record.sh` asks only that `payloads.jsonl` exists and is non-empty, which catches the failure that actually happens — a hook that never fired.

The cost is that an interrupt or a timeout ends the run. The workspace is kept either way, because discarding one an operator spent a live session on is the single unrecoverable mistake a runner could make, and reading the capture is how they tell a hook that never fired from a procedure that went off the rails. But finishing that run is not possible: **re-run the probe.** The poll timeout is generous for the same reason — an hour, overridable with `PROBE_TIMEOUT_SECONDS`. Its job is diagnostic, not resource protection, and a premature expiry would cost a session outright.

`record.sh --capture` remains, because it is how the capture directory reaches version resolution for a manifest declaring `version_source.kind: "capture"` — which four of six manifests do. A runner itself takes no arguments.

## Re-verification

When a captured result goes stale, the thing that aged is the third-party tool, not any file in this repository — so nothing here can be checked by CI, and no probe ever runs there. Re-verification is triggered by judgment: a provider version bump, a changed rendering, or a new provider surface.

A re-run that produces a different `observed` is **assertion drift**, and it is the strong signal — it means provider behavior changed. Treat it as a finding, not a failure.

### The life of a refutation

`expected` is **agentspec's current belief about the provider**, not a historical record. That distinction decides what to do at each stage, and getting it wrong in either direction breaks the harness:

1. **A run records `refuted`.** Do not touch `expected`. Adjusting it here erases the drift the harness exists to surface, and the record is the only evidence anything changed.
2. **Investigate.** Read the capture, not just the record. Rule out apparatus artifacts — a procedure not followed, an option set that did not cover the outcome, an agent that reached the probe's own files. Most early refutations are the probe being wrong, not the provider.
3. **Act on it** if the provider really did change: correct whatever encoded the old belief — documentation, a capability accessor, an adapter's emission.
4. **Then update `expected` to the measured value**, and record the history in the package README: what the old answer was, what the new one is, and what could not be determined between them.

**Step 4 is not optional.** A refutation that has been investigated and acted on but left with a stale `expected` makes the probe report `refuted` on every future run, forever, for a question that is settled. `just check` then prints a permanent alarm, and a permanent alarm is a muted one. The record of the original refutation stays — records are append-only — so nothing is lost by moving the belief forward.

**Updating `expected` does not clear the report — the next run does.** A record stores the status computed when it was written, and records are never edited. So after step 4 the newest record still reads `refuted` until the probe runs again under the corrected belief. That is honest: the belief moved, but nothing has yet confirmed it. Re-run when convenient; a human-driven probe may reasonably wait for the next session.

The failure this guards against is the one that motivated the whole harness: a belief about a provider living in two places that can drift apart. A manifest's `expected` and an adapter's capability accessor are both assertions about provider behavior. Keep them agreeing, and cite the probe from the accessor so the next person finds the evidence.
