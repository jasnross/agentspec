# Provider probes

A **probe** measures what a provider actually does with a rendering, in that provider's own resolved view. agentspec's unit and integration tests compare emitted bytes against agentspec's belief about what each provider reads; nothing in the test suite checks that belief against the provider itself, and the belief has been wrong.

This directory is where that check lives. Each package holds one probe: its fixtures, its definition, and its results.

## The workflow: probe before you implement

When you are about to make agentspec emit a new rendering:

1. Check whether a probe already answers it: `jq -r '.question' experiments/*/probe.json`. Every package has a manifest, so this list is complete.
2. If nothing covers it, write a probe and run it.
3. Design against the measurement — and against what its `depth` licenses you to conclude.

**Absent from that list means nobody has measured it**, not that it works. OpenCode's _skill_ surface is the worked example, and it is now on the list rather than absent from it: `experiments/opencode-skill-frontmatter-discard/` records that `variant:`, `model:`, and `tools:` are all parsed and discarded there, while `experiments/opencode-agent-variant/` and `experiments/opencode-command-variant/` record the agent and command surfaces reading the same `variant:` key.

That the three disagree is the lesson: one key, read on two surfaces of one provider and discarded on the third.

The skill finding also shows what "absent from that list" costs. It was a record once, in `docs/provider-verification.md`; when that document was replaced by this harness on 2026-08-15 it survived only as prose, and it stayed prose until a package re-measured it on 2026-08-24. Throughout, agentspec kept emitting the discarded fields — and a prose claim, unlike a record, is not something `probe status` can report as stale.

Probes verify the _provider's_ contract, so they use hand-authored provider config files rather than `agentspec compile` output. Nothing here requires an agentspec feature to exist first, which is why a probe can precede the change it de-risks instead of gating it afterward.

**Probe first when the outcome could change the design; defer when it can only confirm.** A rendering with no records has simply not been measured. This harness deliberately keeps no list of what ought to be — it reports measurements, and "unprobed" is a state that lasts exactly as long as it takes to write the probe.

## Authoring a new probe

1. **Pick the driver.** It names what running the package costs, which is what `probe run` must be authorized to spend before it will execute it. `unattended` if a command answers the question at no cost, with no credentials beyond a login the operator already holds. `billed` if a command answers it but the run spends model quota, so it needs an explicit opt-in. `manual` if a person must drive the provider.
2. **Create the package.** `experiments/<probe-name>/`, where the directory name is the probe's identity. No file restates it — `probe status` reads records by path and already knows it.
3. **Write the fixtures and the definition.** Fixtures go under `fixtures/assertion/` (see [Fixture variants](#fixture-variants)). A human-driven probe also needs a capture hook, `fixtures/capture/dump-hook.sh`, that appends each payload as one JSON line to `capture/payloads.jsonl` — the payload as the provider sent it, with nothing added. Use `{{PLACEHOLDER}}` for anything absolute; the harness fills them in at _Arrange_ (`template_file` in `lib/probe_harness/workspace.py`) so no operator ever hand-edits a path. The definition is `probe.py`, which sets a module-level `PROBE` to one family's definition type: `ManualProbe`, `OpenCodeDebugProbe`, `OpenCodeFakeProbe`, `ClaudeOtelProbe`, or `ClaudeInitProbe`. That family's runner does the setup, the provider invocation, and the run-soundness gates, so the definition states only the arms, prompts, and checks specific to this probe. A probe new to every family writes a `CustomProbe`, supplying its own arrange, act, and judge steps.
4. **Author the assertion.** The manifest declares the `expected` value, or an option set with an expected id. The definition carries every expression the record is judged by — the assertion, a manual probe's `wait_for` and `capture_version`, and any custom checks — as private callables in `probe.py`, typed against the family's parsed input: each arm's request (and, with `requires_responses`, response) models for the arm families, the `output` model for `opencode debug`, and the capture lines its `payload` model parses for a manual probe. Every field an expression reads is declared in a model under `lib/probe_harness/models/`.
5. **Validate that it discriminates** (see below). This is a required step, not a nicety. Run two dry runs, each of which keeps its workspace: `PROBE_DRY_RUN=1 experiments/probe run <probe-name>` for the assertion fixture, and `PROBE_FIXTURE=discriminator PROBE_DRY_RUN=1 experiments/probe run <probe-name>` for the discriminator. Iterate the assertion by editing it and re-judging each kept workspace with `experiments/probe record <probe-name> <workspace> --dry-run`, which runs every gate and the edited assertion for free. Then write the two workspaces as [samples](#samples), `experiments/probe sample <probe-name> <workspace> confirming` and `… refuting`, plus any prefix or edited samples a `wait_for` or custom check needs for its false side, and run `just probe-test`, which proves every expression takes both outcomes.
6. **Run it** with `experiments/probe run <probe-name>`. The family runner writes the record.
7. **Commit the fixtures, the manifest, the definition, the README, and the record.**

**The definition is authored first, and committed only with samples that show each of its expressions discriminates.** Writing the assertion in the definition first is what makes step 5 honest: `--dry-run` evaluates the callable the definition actually carries, the same way a real record does, so the expression validated is the expression that will run. Evaluating something else at the shell and transcribing its result afterwards leaves two separately-written expressions, and a slip that makes one return `[]` records `refuted` — a false finding rather than a loud failure. The samples keep the proof alive: a later edit that makes an expression constant fails `just probe-test` rather than waiting for the next live run. What the rule forbids is a _committed_ definition whose expressions have never been shown to distinguish two inputs, not a draft one on disk.

### The two rules that replace a control arm

There is no control arm in this harness. Two rules do its work instead.

**Expected values must be non-default.** A pass must be positive proof, not a possible reading of the provider's default. Cursor's own documented example, `claude-opus-5[effort=high]`, is a default collision on that model — an arm that would pass whether or not the option was honored.

**An assertion must be shown to discriminate, once, at authoring time.** For an assertion, show it returns distinct values for two different inputs; for an option set, show the options are mutually exclusive and exhaust the outcomes the probe can produce. Without this, a misspelled field returns `null` forever and nobody notices. The evidence is a package's samples: one whose assertion equals `expected` and one whose does not, and for `wait_for` and each custom check one sample where it holds and one where it does not. `just probe-test` re-proves all of it on every run, so the proof does not go stale when the code it reads changes.

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

**The enum is shorter than the chain, deliberately.** `emitted-bytes` is the hop agentspec's own tests already cover, and `provider-parses` would be an assertion that no error occurred — which the rule above forbids. Naming either as a depth would invite a record claiming evidence the contract prohibits gathering, so the recorder and the test suite both reject them.

The gap between `resolved-config` and `outbound-request` is the one that bites. `opencode debug agent` prints the _declared_ variant; OpenCode collapses an unrecognized one to `{}` later, when it builds the request. So a `confirmed` at `resolved-config` says the provider **read** your field — design on that, but do not claim the model saw it.

For Cursor that gap is permanent: all traffic including BYOK terminates in Cursor's own backend, so no Cursor probe can ever exceed `resolved-config`.

## Package layout

```
experiments/
  README.md                   # this file — the probe contract
  probe                       # the harness CLI: run, record, status
  pyproject.toml  uv.lock     # the harness's locked environment: its runtime library (pydantic) and dev tools
  lib/
    probe_harness/            # the harness package: recorder, run loop, status, family runners, gates
    stubs/                    # the stub MCP server and stub model endpoint the arm families share
    tests/                    # pytest coverage for the above
  <probe-name>/
    README.md                 # question, procedure, discriminates evidence, oracle limits
    probe.json                # the manifest — the only hand-authored contract file
    probe.py                  # the definition: PROBE, run by its family's runner
    fixtures/                 # provider config the runner materializes
    results/*.json            # records, one per run, append-only
```

`experiments/probe` syncs `uv.lock` into `experiments/.venv` with `uv sync --locked` and re-executes itself there, so a harness feature takes a library by declaring it in `pyproject.toml`; a missing uv or a stale lock is refused with a `probe:` line before any harness code runs. The harness reads and writes JSON itself, including every record, and each family leaves a `view.json` in the workspace for inspection — for the fake-provider and billed Claude families an object mapping each arm to its request bodies (and responses when the definition requires them), and for the Claude init family each arm to its `init` and `result` events, written by the judge; the oracle's own output for `opencode debug`, which the oracle run writes; and the list of captured payloads for a manual probe, written by the judge. A `CustomProbe` writes none of its own.

Request bodies, `opencode debug` output and hook payloads are read through pydantic models in `lib/probe_harness/models/`, one module per format, and gates, assertions and custom checks read the same models. A field is required unless a provider is known to omit it; only then does it default to `MISSING`, with a comment naming the evidence. A model's source JSON is reachable only through `RawJson`'s `contains`, `strings` and `keys`, never by key, so an expression reads only fields a model declares. A body that fails its model is refused, naming its file and the field's path, and the workspace is kept. `lib/tests/test_repo_model_coverage.py` requires every `MISSING`-able field of a sampled model to be present in some committed sample; a field declared for a shape no capture shows carries `ExpectedAbsent(reason)` in its `Annotated` metadata instead.

**One package holds exactly one probe, and one probe answers one question.** A package needing two answers is two packages.

**A package never depends on another package.** Apparatus shared across a family's probes — the stub MCP server and the stub model endpoint — lives in `lib/stubs/` and is parameterized per probe; the files a provider reads stay in the package.

**A probe package has measured something.** Every directory here except `lib/` carries a manifest, a definition, and at least one record. A probe that cannot run, or that nobody has attempted, is intent rather than evidence: its question belongs in `TODO.md`, alongside every other unmeasured provider question.

## Samples

**`samples/<name>.json` holds an input and what each of the definition's expressions yields on it.** `input` is the family's sample format — `{"<arm>": {"requests": [...], "responses": [...]}}` for a billed Claude probe, `{"<arm>": {"init": {...}, "result": {...}}}` for a Claude init one, `{"<arm>": {"requests": [...]}}` for a fake-provider one, the oracle's document for `opencode debug`, and a list of capture lines for a manual probe. `results` declares exactly the expressions the definition carries: `assertion`, `wait_for`, `capture_version` (`null` for no version) and `custom_checks`, keyed by each check's `name`. `origin` is `capture`, `prefix` or `edited`, and an `edited` sample carries a `note` saying what was edited and why. Conventional names are `confirming`, `refuting`, `before-<event>` and `edited-<what>`.

**`probe sample <package> <workspace> <name>` writes one from a kept workspace.** It parses the workspace through the family, evaluates every expression over the full capture, then reduces and anonymizes it, and writes the sample only when every expression yields the same on the reduced input and the leak scan finds nothing; it never overwrites.

- **Reduction** keeps, on every model, each declared key that was set (a declared string whole), and each other key's value only when a string in it carries a search string — the definition's marker, the deferred-tools listing, or the subagent flag. Such a string, under an undeclared key or in a `RawJson` value, keeps only the line carrying the marker or the subagent flag, and the deferred-tools listing's paragraph, from its header to the first empty line. A search reading more of the string than that may yield differently on the reduced input than on the capture, which the equality check refuses, so a written sample proves no expression's result depended on a dropped line. Lines starting with a claude.ai connector tool name (`mcp__claude_ai_`) are dropped, since they are the operator's account configuration. A declared string naming one is kept whole, so the leak scan refuses that sample, and it needs a capture taken without the operator's connectors. Every other key stays, as `null`, so `raw.keys()` is unchanged, and a `RawJson` value keeps its shape with every string not carrying a search string blanked.
- **Anonymization** rewrites the home directory to `/home/probe`, macOS temp directories (and their `-`-encoded forms) to `/tmp/`, `/private/tmp/` to `/tmp/`, and the username to `probe`.
- **The leak scan** refuses a sample still carrying the home directory, the username, git's `user.email` or `user.name`, a line of 30 or more characters from the operator's `~/.claude/CLAUDE.md` or `~/.config/opencode/AGENTS.md`, the name of one of the operator's own skills or commands containing `-` or `:`, `/Users/`, `/var/folders/`, `var-folders`, `mcp__claude_ai_`, or anything email-shaped. `just probe-test` runs it over every committed sample.
- **`--lines N`** samples a manual capture's first N lines, for a `wait_for`'s false side: choose the largest N that leaves the awaited event out.
- **An edited sample** is a hand-edited copy of a reduced one, for a false side no fixture makes a provider produce. After editing, `probe sample <package> --regenerate` recomputes every sample's `results` from its `input` with the definition's expressions, printing each changed result. It rewrites only the samples whose results changed, and never changes their `origin`, `note` or `input`. An author who changes an expression regenerates its samples' results the same way, then reviews the printed changes and the diff before committing: regenerated results agree with the code by construction, so `just probe-test` then checks only that each expression still takes both outcomes.

`lib/tests/test_repo_samples.py` holds each package to the both-outcomes rule: every declared result reproduced by the definition, the assertion equal to `expected` on one sample and not on another, `wait_for` and each custom check both true and false, and a `capture_version` yielding a version on at least one. Every package must have samples.

## Fixture variants

**Every family arranges from a full-copy tree per variant, `fixtures/<variant>/`.** Everything a provider reads for one variant lives in that variant's tree, so `diff -r fixtures/assertion fixtures/discriminator` shows exactly what a discriminator varies. A manual probe's capture apparatus is the exception: it is not provider configuration, and it stays once in `fixtures/capture/`, shared by every variant and templated outside the project the operator opens. A `CustomProbe` receives the selected variant in its arrange step and reads `fixtures/<variant>/` itself.

**Every fixture a package commits is reachable through its definition.** Each entry directly under `fixtures/` must be a declared variant, or `capture/` for a manual probe. The manual, fake-provider, and debug families copy a variant tree whole. A Claude probe, of either Claude family, reads its variant tree piecemeal, so each entry inside one must be an arm's tree, a shared input an arm names (an MCP config template, a plugin source, a user-scope config template), or a name the definition lists in a module-level `FIXTURE_PATHS`, which must exist in every variant tree. A fixture no run can reach is evidence nobody can reproduce, so `lib/tests/test_repo_fixture_reachability.py` fails on one.

**A definition declares its variants, and `assertion` is the only one a run can record.** `variants` defaults to `("assertion",)`. A probe that shows its assertion discriminates with a second fixture — conventionally `discriminator`, built so the assertion must disagree with it — declares that variant too, and keeps its tree beside `assertion/`. A declared variant with no `fixtures/<variant>/` is refused before a workspace exists, and so is a Claude variant tree missing an arm's tree, MCP config template, or plugin source.

**`PROBE_FIXTURE` names a declared variant, and any other variant runs only under `PROBE_DRY_RUN=1`.** It defaults to `assertion`. An undeclared name exits 2 before a workspace exists, and so does a non-`assertion` variant without `PROBE_DRY_RUN=1`: a run of a fixture built to disagree would record `refuted`, and committing that record would be a false finding. So the discriminator run is

```sh
PROBE_FIXTURE=discriminator PROBE_DRY_RUN=1 experiments/probe run <probe-name>
```

and it prints the record it would have written, which should be `refuted`.

## The manifest (`probe.json`)

The only hand-authored file in the contract.

**A manifest carries no expression.** Its `assertion` is `{expected}` or `{options, expected}`, and a capture version source is `{"kind": "capture"}`: every expression it is judged by is Python on the package's definition. A `projection`, `wait_for` or `jq` key is refused by its path, as every unknown key is.

| Field | Meaning |
| --- | --- |
| `schema_version` | `2`. |
| `provider` | `claude`, `cursor`, or `opencode`. |
| `driver` | `unattended`, `billed`, or `manual` — what running the probe requires. Read by `probe run` to decide what it can execute, by the recorder, which refuses a `manual` probe's workspace whose capture is empty, and for the options correspondence below. Never on a record. |
| `depth` | Required: `resolved-config`, `outbound-request`, or `null`. |
| `question` | Required. What this probe answers, in one sentence. |
| `version_source` | Where the tool version comes from. See below. |
| `assertion` | `{expected}` or `{options, expected}`. An `options` assertion additionally requires `driver: "manual"` — see below. |

**`depth: null` is the right answer for a finding that is not on the config-rendering chain at all.** Output handling and hook-firing semantics are not weaker points on that chain; they are off it, and claiming a position on a chain the evidence never touched is a category error.

**`question` appears only at the top level.** A human-judged probe shows this same string to the operator rather than carrying a second copy inside `assertion`, and no assertion restates it.

**`parse_manifest` in `lib/probe_harness/contract.py` checks the type and value of every field described here.** `probe run` and `probe record` both parse through it, so they refuse a malformed manifest the same way, before any workspace exists, rather than at the step that would first read the bad value. A key not described here is refused too, and the refusal lists every invalid field by its path.

**There is no timeout field.** The poll timeout is a runner constant with a `PROBE_TIMEOUT_SECONDS` override, because it is a property of the operator's session rather than of the probe.

### `version_source`

| `kind` | Extra field | Behavior |
| --- | --- | --- |
| `command` | `command` | Run it; take the first line. |
| `capture` | — | The definition's `capture_version` reads it from the parsed capture. |

`kind` is one of `command`, `capture`, or `none`; an omitted `version_source` or `kind` means `none`, but a `null` one is refused. A `command` must be a non-empty, single-line string.

**`kind: "none"` is deliberately absent from that table, but accepted.** The recorder records a `null` version for it, and the harness's own test fixtures declare it, which is how they avoid running a version command. No committed manifest uses it, and the reason not to reach for it: a result carrying no tool version cannot drift, so `probe status` has nothing to compare and the result is never reconfirmed against anything. If a version is genuinely unobtainable, say so in the package README.

**A `command` is split on whitespace and exec'd directly — never through a shell.** So `opencode --version` works and `sh -c "…; …"` does not: `;`, `|`, `>`, `$()`, and quoting are not interpreted. `just probe-status` runs these commands on every `just check`, and a manifest is reviewed as data rather than as code, so it must not be able to express arbitrary code. The command also runs with stdin closed and under a timeout, because a version command that blocks would wedge the build — which is worse than failing it.

**A Cursor probe declares `kind: "capture"` and never a CLI command.** The probe exercises the IDE, whose version arrives in the hook payload as `cursor_version` — the field agentspec itself trusts for host detection (`detect_provider` in `src/hooks_canonical.rs`). `cursor-agent --version` reports a different artifact on a different versioning scheme (`2026.05.28-a70ca7c`), so comparing against it would produce a drift signal about something no probe touched.

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

**`options` is a non-empty list of `{id, text}` objects, each optionally carrying `status`.** Every `id` is a non-empty string, unique within the list, and every `text` is a string. Any other key on an option is refused, so a misspelled `status` cannot silently drop the couldn't-tell option's verdict.

**An options assertion requires `driver: "manual"`, and the recorder refuses a manifest that declares one without it.** An option set is a person choosing from a list, so running the probe requires a person present — which is exactly what `manual` declares and what neither other value can supply. An `unattended` or `billed` manifest declaring one describes a runner that would have to prompt an operator no scheduler put there. The implication runs one way only — a `manual` probe may still be machine-answered, which is what `claude-session-start` is, and its assertion over a capture is perfectly legitimate.

This is what makes the runner's read safe: the manual family's judge (`manual.judge`) decides whether to prompt from the assertion's shape rather than from `driver`, because the shape is the fact and the driver was a second copy of it. `lib/probe_harness/contract.py` holds the gate, and the recorder, every runner's preflight, and the test suite all read the manifest through it, so they cannot drift apart on it.

**An option may declare `status`, and `inconclusive` is the only legitimate value.** A declared status replaces the comparison against `expected` — that is how the couldn't-tell option yields `inconclusive` rather than `refuted`. Any other value would let a manifest fix the verdict in advance, which is the caller-supplied status the record contract exists to prevent; it would simply arrive through the manifest instead of the command line. Both the recorder and the test suite reject it, so a committed manifest cannot carry one.

## The record (`results/<date>T<HHMMSS>-<provider>-<version>.json`)

**Every record is produced by a run or by `probe record`, from a judged workspace.** Nothing is hand-written, so there is one record shape and no exception to the never-hand-edit rule. Records are append-only: superseding a result means adding a newer file, never editing an existing one. `experiments/*/results/` is prettier-ignored so a probe run is the last thing that touches its own record.

A schema 2 record has nine keys, `arranged` among them only when it applies. A schema 1 record, the shape every record written before the harness evaluated Python takes, has seven; the harness reads it and never writes one. `probe status` reads `provider`, `status`, `depth`, `date` and `tool_version`; `schema_version` decides which format the record is read as, and `assertion` and the source maps are what a reader recovers the finding and its code from.

| Key | Meaning |
| --- | --- |
| `schema_version` | `1` (read only) or `2`. |
| `provider` | Groups the report. |
| `status` | `confirmed`, `refuted`, or `inconclusive`. |
| `depth` | Nullable; copied from the manifest. |
| `date` | UTC date of the judge that wrote the record: the run's, or the `probe record` invocation's. |
| `tool_version` | Nullable. |
| `source` | _Schema 2._ The git blob id of every file that judged the workspace — the package's `probe.json`, `probe.py` and fixtures, and the harness — keyed by repository-relative path. |
| `arranged` | _Schema 2, optional._ The same map for the code that arranged the workspace, present only when it differs from `source`: a workspace re-judged by `probe record` after the code changed. |
| `assertion` | Schema 1: `{projection, expected, observed}` or `{options, expected, observed}`. Schema 2: `{expected, observed}` or `{options, expected, observed}`. |

Fields that would merely restate a derivable fact — the probe's name, its driver, the workspace it was judged from — are absent, because storing a derivable fact means storing a way for it to disagree with its source. `source` is not one: the package's files change after a record is written, and nothing in the package's layout names the content that judged it. An assertion is Python on the definition, so `source` is the only way back to it. A schema 1 record carries its jq projection's text inside `assertion` instead.

**`probe source <record> <dir> [--arranged]` writes a schema 2 record's files back out.** Each file the record names is written to `<dir>/<path>` from the record's repository's object store, `--arranged` choosing the arranging code over the judging code (the same files when the record carries no `arranged`). It refuses to overwrite an existing file, lists each path whose object is missing and exits 1, and refuses a schema 1 record, whose assertion text is in the record. Like `probe status`, it executes no probe-authored code.

### The clean-tree rule

**A record is written only when every file its `source` names matches `HEAD`.** A blob id names committed content only if the content was committed, so a real `probe run` checks the map in preflight, before the workspace exists, and `probe record` checks it before judging. A covered path added, deleted or modified since the last commit refuses, naming up to ten of them; commit them and re-run. The map is computed again just before the record is written, and a change since the first computation refuses too, keeping the workspace. A dry run makes neither check: it prints the record a real run would write, from the files on disk. A package outside any git work tree has no `HEAD` and its records cannot be committed, so the rule is skipped for it — which is how the harness's own tests record end to end.

### The reachability rule

**Every id a package's newest record, when it is schema 2, names in `source` must be reachable from `HEAD`.** A rebase or squash that rewrites the commit holding a probe's code before it reaches `main` leaves the record naming objects no ref reaches, and `git gc` eventually deletes them. `lib/tests/test_repo_record_sources.py` enumerates `HEAD`'s objects and fails naming each unreachable file, so `just check` run after the branch's last `git town sync` is the moment it catches a lost id. The repair is to re-judge the kept workspace with `probe record`, which writes a newer record naming the code as it now stands. `arranged` is not checked: it names code that may never have been committed.

**`blocked` is not a status.** A probe that cannot run makes its runner exit nonzero with a diagnostic, which produces no record at all. Recording "this could not run" would mean writing a record for a run that never happened. Nor is there anywhere for such a status to live: a probe that cannot run has no package at all, so its question sits in `TODO.md` until someone can measure it.

The filename carries a UTC time component because date plus version does not disambiguate a same-day re-run at the same provider version — and re-running to confirm a `refuted` result is the first thing anyone would do. The full name also sorts lexicographically in run order, which is how `probe status` finds the newest.

### `probe record`

```sh
experiments/probe record <probe-name> <workspace> [--selection <id>] [--dry-run]
```

`probe record` judges a kept workspace and records it. It loads the definition, checks the manifest's fields and the definition's consistency with it as a run does, runs the family's own judge — the same one `probe run` uses: every gate, the custom checks, `wait_for` for a manual probe, view assembly — over the workspace as it stands, and records the result. A workspace its gates refuse is never recorded. It reads only the workspace, never the provider: the version a `command` source names comes from the workspace's `tool_version`, so a real record over a workspace without one is refused. The checks a run makes only to arrange — the definition's own preflight function, the run's required tools beyond git, the other `PROBE_*` variables — are not made, since judging needs none of them. It computes the source map before judging, and a real recording applies [the clean-tree rule](#the-clean-tree-rule) to it then and again before writing. The record's `source` is that map; when the workspace's `source.json`, written by the run that arranged it, names different content, the record also carries it as `arranged`. A real recording refuses a workspace whose `source.json` is missing or is not a source map, since its record could not say which code arranged it; such a workspace can still be judged with `--dry-run`. `PROBE_DRY_RUN=1` makes it a dry run, as `--dry-run` does. It refuses a workspace whose `run.json` names another package, and writes a record only over one whose `run.json` names the `assertion` variant: a discriminator's run is dry-run only, as it is under `probe run`, and a workspace with no `run.json` can only be judged dry. The judge may write `view.json` into the workspace, as a live run's does.

A manual probe with an option set asks for a selection when stdin is a terminal; otherwise, and always under `--dry-run`, it needs `--selection <id>` naming the option, and exits 2 without one. `--selection` with a manifest declaring no options is a usage error.

### `probe record --dry-run`

`--dry-run` judges the workspace end to end — every gate, the assertion, the structural comparison, version resolution, record assembly — and prints what a record would contain instead of writing one. No `results/` directory is created and no record is written. A workspace with no recorded tool version reports `null` with a warning rather than refusing.

**Re-judging a kept workspace is `probe record --dry-run`.** It is free to run against a workspace from a billed or manual run, and it is how a gate or assertion change is checked against real captures without paying for new ones. The workspace must have the layout its family's runner creates.

**A runner takes no arguments, so `--dry-run` is not one of them.** Every runner reads `PROBE_DRY_RUN=1` from the environment and passes the flag on to the recorder itself — the same shape as `PROBE_FIXTURE`. Every run keeps its workspace.

It exits **0 whatever status the comparison computes**, including `refuted`. Validating that an assertion discriminates means running it against an input it is supposed to refute, so a nonzero exit would fail the primary use case every time; it would also make dry-run disagree with real-run semantics, where `refuted` is a finding rather than a failure.

Every gate still fires. A manifest failing a manifest check, a capture whose payloads are absent or empty, an assertion that raises or yields no JSON value — all still refuse, because those are the wiring being verified.

**Its stdout is not a record.** It is what a record would contain, and it must never be redirected into `results/`: every record in this repository is produced by a run or by `probe record`, and that invariant is what makes hand-editing one a defect rather than a workflow. `--dry-run` exists to reduce the pressure to violate that rule, not to work around it.

## Running probes and reading the report

```sh
just probe-run                   # the free probes; costlier ones are listed with the flag that frees them
just probe-run --billed          # + the probes that spend model quota
just probe-run --manual          # + the probes that block on a live provider session
just probe-run --all             # every driver
just probe-run --stale --all     # every probe owed a run
just probe-status                # report on the committed records; invokes no probe
just probe-test                  # the harness test suite
just probe-lint                  # ruff and mypy over the harness and every definition
just shellcheck                  # lint the scripts a provider runs

experiments/probe run <probe-name>   # run one package, whatever its driver
```

The harness and its dev tools need Python 3.14 or newer, and `mise` supplies it along with uv and the pinned jq the manual capture hooks and the harness's own tests run. `probe-run`, `probe-status`, and the probe summary call `experiments/probe`; `probe-test` and `probe-lint` run through `uv`.

**Messages and exit codes.** Every diagnostic line the harness writes starts with the subcommand that printed it — `probe run: …`, `probe record: …`, `probe status: …` — whichever family, gate, or recorder step it came from. A provider's own output, such as the stderr of an arm that exited nonzero, is printed unprefixed after the line that introduces it. So is the text a manual run addresses to its operator: the workspace it prepared, the procedure, and the selection prompt. The timeout that ends a run whose capture never arrived is a prefixed refusal like any other. Exit 0 is success, 1 a refusal (a missing tool included), and 2 a usage error or an invalid `PROBE_*` variable.

`just check` ends with a one-line probe summary. That line can never fail the build.

### Selection and authorization

`probe run` decides each package with two independent questions, in this order:

|  | question | flags |
| --- | --- | --- |
| **selection** | is this package interesting to this run? | `--stale` |
| **authorization** | may this run pay what the package costs? | `--billed`, `--manual`, `--all` |

**Every probe is runnable by `probe run`, including the manual ones.** Every manual package defines a `ManualProbe`, whose family runner arranges the workspace, prints the procedure, blocks until the capture lands, and records. So what separates the drivers is not capability — it is cost. An `unattended` probe costs nothing, a `billed` one spends model quota, and a `manual` one spends an afternoon of a person's attention. A run is therefore defined by what it is willing to spend, and the default spends nothing: a batch run that costs money or blocks for hours on every invocation is a batch run people stop invoking.

**Authorization is a set, not a level.** Each flag adds its own driver; `--billed` never implies `--manual` and neither implies the other. They stack in any order, repeat harmlessly, and `--all` is the union. `PROBE_AUTHORIZE_DRIVERS="billed manual"` seeds the same set for a caller that cannot pass arguments, and composes with flags rather than overriding them. The set is built from `DRIVERS` in `lib/probe_harness/contract.py`, so a fourth driver costs one entry there plus its flag — not a new combination in the run loop, and not a hand-maintained line in the summary breakdown.

A withheld package is printed with the flag that would free it, and the summary counts by driver for the same reason: the only useful thing to tell a reader looking at a skip is what to type next.

**`--stale` narrows the run to the packages actually owed one.** A package is owed a run when it has never produced a readable record, or when its recorded `tool_version` is comparable against the installed one and differs. Everything else is passed over as `fresh`, with the reason printed, so the filter is auditable without running anything.

Selection is evaluated before authorization, so a fresh package is passed over whatever it would have cost — a `--stale` run does not ask you to authorize a probe whose answer cannot have changed. The two are otherwise independent: being owed a run is not permission to pay for one, so `--stale` alone reports a drifted `billed` package as needing `--billed`, and `--stale --billed` runs it.

**"Cannot compare" counts as fresh, not as owed.** A `capture`-sourced version — every Cursor package — is knowable only by running the probe, and a `none`-sourced one only by a human. Treating unknowable as owed would put those packages permanently in the run set, which is the same as having no filter at all. `probe status` still shows them, annotated with why no comparison was possible. The two commands share one implementation of this judgement (`lib/probe_harness/state.py`) precisely so the report and the runner cannot come to disagree about what stale means.

`probe run` accepts no argument beyond those four, and exits 2 on one — as does an unknown driver in `PROBE_AUTHORIZE_DRIVERS` — rather than silently ignoring a token that was meant to authorize spending.

### What the summary line can and cannot see

```
probes: 8 recorded · 0 refuted · 0 inconclusive · 1 version drift · 2 billed (drift not tracked)
```

The drift count is **not** a claim that everything else is current. Every Cursor probe declares `version_source.kind: "capture"`, and a captured version is knowable only by running the probe — which `probe status` never does. So no drift is computable for those packages, and they contribute nothing to that count. Read `0 version drift` as "nothing computable drifted," not as "everything is current."

**Billed packages sit in their own segment, and their drift is deliberately uncounted.** A batch run never refreshes one, so its recorded version falls behind the installed one and stays there until somebody pays for a `--billed` run. Counted, that drift would be structurally unable to reach zero — and a count that can never reach zero is a muted alarm: it hides the genuine drift of a package a free batch run could clear. The row still shows the installed version and names what would refresh it. The segment is omitted entirely when no package declares `billed`.

### The two drift signals are not equally strong

**Version drift** — the recorded `tool_version` differs from the installed one — is weak. It means only that the result has not been reconfirmed against the version you are running.

**Assertion drift** — a re-run produced a different `observed` — is strong. It means provider behavior changed. Since `probe status` invokes no probe, it cannot produce assertion drift itself; it surfaces `refuted` records, which are assertion drift already recorded.

## A human-driven run is one invocation

The manual family's runner materializes the workspace, prints the procedure, polls, and records — one blocking invocation with no resume path. That is what makes a stale capture unreachable rather than merely discouraged: the workspace was created by the invocation still running, so there is no earlier run to point at and nothing for a freshness check to prove. The recorder asks only that `payloads.jsonl` exists and is non-empty, which catches the failure that actually happens — a hook that never fired.

The cost is that an interrupt or a timeout ends the run. The workspace is kept either way, because discarding one an operator spent a live session on is the single unrecoverable mistake a runner could make, and reading the capture is how they tell a hook that never fired from a procedure that went off the rails. But finishing that run is not possible: **re-run the probe.** The poll timeout is generous for the same reason — an hour, overridable with `PROBE_TIMEOUT_SECONDS`. Its job is diagnostic, not resource protection, and a premature expiry would cost a session outright.

**Every family keeps every workspace,** at `$XDG_STATE_HOME/agentspec/probe-workspaces/<package>/<YYYY-MM-DDTHHMMSS>-<variant>/` (`~/.local/state/…` when `XDG_STATE_HOME` is unset). The harness never deletes one; its owner deletes it once the branch carrying its record ships. The root must lie outside every git work tree, with no provider instruction file (`CLAUDE.md`, `AGENTS.md`, `.claude/`, `.cursor/`) in any directory above it below `$HOME`, since a provider opened on a workspace would read those.

A workspace records what produced its capture. `run.json` names the package and fixture variant it was arranged for. `source.json` holds the git blob id of every file that affects evaluation — the package's `probe.json`, `probe.py` and `fixtures/`, and the harness itself — as it was on disk when the run started. `tool_version` holds a `command` version source's output, read once just before Act; every later judge of the workspace reads the version from there rather than running the command, so a provider updated after the run cannot change the version its record carries.

`probe record <package> <workspace>` records a kept manual capture after a fix, and refuses one whose `wait_for` does not hold. A runner itself takes no arguments.

**Each capture line is parsed on its own while the run polls.** A manual definition names the `payload` model one line is parsed against, and its `wait_for` reads the parsed lines. A line that is not JSON, or fails the model, is reported once as it lands, with its line number and the field's path, and a `wait_for` that raises is reported once per distinct message; neither stops the poll, because the operator may still be mid-session. The judge then refuses any unrecognized line before an expression reads the capture, keeping the workspace — so a payload shape the model did not expect costs a model fix and a `probe record`, never the session.

**A toolchain failure cannot cost a capture.** The Python version, the locked environment, the harness's imports, the manifest's gates, the required tools — the family's (jq, for a manual probe, whose capture hook runs it), git, and a `command` version source's — the `PROBE_*` environment variables, and for a real run [the clean-tree rule](#the-clean-tree-rule) are all checked before the workspace exists, so a failure among them surfaces before the operator is asked to open a provider. A manual workspace is kept whatever happens after that.

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

The failure this guards against is the one that motivated the whole harness: a belief about a provider living in two places that can drift apart. A manifest's `expected` and an adapter's capability accessor are both assertions about provider behavior, and they must agree. The manifest is the binding one: it is the copy `probe status` can read, and a refuted record stays loud until somebody acts on it. Naming the probe from the accessor is a courtesy to whoever reads that code next, not the mechanism that keeps the two in step.
