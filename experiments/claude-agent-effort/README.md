# claude-agent-effort

**Question:** On which invocation paths does Claude Code apply an agent's `effort:` frontmatter to the outbound model request?

**Driver:** `billed`. A command answers the question with no human step, but every arm spends a billed model call, so `just probe-run` withholds this package by name and reason; it runs only under `just probe-run --billed`.

**Depth:** `outbound-request`. The oracle is the request body Claude Code sends, captured through OpenTelemetry — the far end of the chain, not the provider's resolved view.

## Why it matters

agentspec emits `effort:` into Claude agent files. Whether that field reaches the model decides whether the emission is load-bearing or decorative — and the answer turns out to depend on _how the agent is invoked_, which is not something agentspec's own tests can see.

## The two arms

One arm per path the same agent file can be reached by:

| Arm | Invocation | Prompt |
| --- | --- | --- |
| `session_agent` | `--agent probe-effort` — the agent file _is_ the session | `Reply with exactly one word: ok.` |
| `delegated` | `--allowedTools Task`, main thread delegates | `Use the Task tool to delegate to the probe-effort subagent. Do not answer yourself.` |

A package measuring only the inert cell could not state a positive finding, and one measuring only the honored cell could not report the cell agentspec's own emission lands in. The delegated arm's prompt deliberately omits the marker: a marked main-thread request would otherwise be read as governed by the fixture.

## The oracle

`CLAUDE_CODE_ENABLE_TELEMETRY=1` plus `OTEL_LOG_RAW_API_BODIES=file:<dir>` writes each untruncated request body to `<dir>/<uuid>.request.json`. The runner globs `*.request.json` specifically — the sink writes `*.response.json` beside them, and a bare `*.json` glob would read a response-only sink as a populated arm.

Request counts are never compared against an expected number. A single turn has been observed producing one, two, and sixteen requests.

## The assertion is relational

Each arm's level is compared against **the requests nothing governed in the same run**, not against a pinned absolute level:

- `"same-as-ungoverned"` means the arm ran at the same effort as the ungoverned requests captured in that run.
- An array such as `["low"]` means the arm ran at a level the ungoverned requests did not.

**The control set is pooled across arms, not computed per arm.** The `session_agent` arm is invoked with `--agent`, so plausibly every one of its own requests is governed and it contributes nothing; its baseline therefore comes from the other arm's separate `claude -p` process. Gate 3 is what licenses the pooling: it requires the whole ungoverned set to agree on one level, so a run where the two arms sat at different ungoverned levels is refused rather than silently averaged.

**No term in `expected` names an external fact** — not a baseline, not the model's default effort, not the operator's subscription tier. Every term compares two things this run measured.

This matters because the model's default effort is not a fact this repository controls, observes for free, or gets a drift signal about (`probe-status` compares `claude --version`, and a model's default effort is not a property the CLI version tracks). Vendor documentation records the default moving between levels, and scopes it by **subscription tier** — so an absolute gate's discriminating power would vary by whose machine ran the probe.

The collision is self-detecting rather than silent. If the model default were `low` and the fixture's pin failed to load, the ungoverned requests would read `low` too, `rel` would render the delegated arm as `"same-as-ungoverned"`, and the record would refute. The do-nothing case refutes for the same reason: a Claude that ignored frontmatter entirely would put every request at one level, so both arms would render `"same-as-ungoverned"` and `delegated` would fail to match.

### Governed vs. ungoverned is decided by `.system`

A request is **governed** by the fixture when the marker appears in its `.system` — the fixture's body becomes the system prompt of the request it governs. This is narrower than "mentions the marker anywhere", and the distinction is load-bearing rather than pedantic.

Measured on the discriminate run: when the subagent replies, its text comes back to the **main thread** inside a tool result. That main-thread request quotes the marker but is governed by the fixture not at all, and sits at the ungoverned level. Selecting on `tostring | contains(marker)` swept it into the delegated arm, which observed `["max","medium"]` instead of `["max"]` — an incidental echo baked into the asserted value. Selecting on `.system` measures what the arm means to measure.

The same definition runs through gate 2 (which would otherwise pass for an arm whose fixture never engaged, on the strength of an echo) and gate 3's control set (where an echoed request correctly counts as ungoverned — its effort was not set by the fixture).

`unique` rather than a positional selector, in both arms: the sink names files by UUID, so glob order is not write order, and `| last` would name an arbitrary element while silently discarding disagreement among the rest. Both arms are spelled identically, including the arm where nothing could go wrong, so a reviewer who has checked one has checked them both.

## The three gates

Every gate asserts on a positive signal in the captured bodies. None of them names an effort level.

1. **Every arm captured at least one `*.request.json`.** An empty sink means the arm never ran.
2. **Every arm holds at least one request the fixture governs.** Without it, "the fixture never engaged" and "the fixture engaged and Claude discarded its effort" are indistinguishable — which would make the most decision-relevant finding the least trustworthy one. **Gate 2 is what makes the inert arm assertable.**
3. **At least one ungoverned request declares `output_config.effort`, and they all agree on one level.** The filter is an explicit exclusion of requests carrying no `effort` key: Claude emits an intermittent title-generation sidecar — observed in this package's own runs, dispatched to `claude-haiku-4-5` rather than to the pinned model — whose `output_config` holds a `format` object and no `effort`, and a control stated over all unmarked requests would fail on it every time it appeared. Written as an exclusion, the other direction stays loud — a Claude that stopped populating `effort` on governed requests empties the control set and trips the existence clause.

A gate failure is a statement about the run, not about Claude, and writes no record.

The gates live in `experiments/lib/probe-claude-otel.sh` rather than in this runner, shared with `claude-skill-effort` and covered by `experiments/lib/tests/probe-claude-otel.bats` against fabricated views. Duplicated in two runners, the only thing that would ever exercise them is a real, paid run — an untested control on a billed apparatus.

Both gates take the field the marker must appear in as a parameter, defaulting to the `.system` this package wants. `claude-skill-effort` passes `.messages`, because a skill's body never reaches `.system` — see that package's README.

## Isolation

The runner performs four isolations, each closing a way the measurement could become about the operator's machine rather than about Claude:

- **`env -u CLAUDE_CODE_EFFORT_LEVEL`** — that variable outranks frontmatter, so an exported one would make the probe measure the operator's shell.
- **`--setting-sources project`** — excludes the user tier outright rather than out-ranking it.
- **`--model claude-opus-4-8`** — pins the value domain. The only requirement is that it serves `low` and `max`; nothing depends on its default effort.
- **`--max-budget-usd 0.50`** — print-mode only, and counts subagent spend. A cap hit stops subagent spawns, which surfaces as a gate-2 failure rather than as a wrong answer — loud, which is why the cap is generous rather than tight.

No `--effort` flag is passed; it would outrank frontmatter too.

The fixture's own `.claude/settings.json` pin (`effortLevel: medium`) is **not** an isolation — it is a fixture choice that keeps the ungoverned level stable across runs and the record legible. Nothing in `expected` depends on its value, and if it silently stopped loading the finding would still be correct, just measured against the model's default instead. It does, however, confound one of the two findings; see "What the record licenses."

## Discriminate evidence

Both rows below come from `record.sh --dry-run` output on actual runs, not from a design document. The two fixtures differ only in the agent's `effort:` value. The `status` and `observed` fields are excerpted from the full seven-key record each run printed; the other five keys are the manifest's own values plus the date and version.

```
PROBE_FIXTURE=discriminator PROBE_DRY_RUN=1 experiments/claude-agent-effort/probe.sh
{"status":"refuted","observed":{"session_agent":"same-as-ungoverned","delegated":["max"]}}

PROBE_DRY_RUN=1 experiments/claude-agent-effort/probe.sh
{"status":"confirmed","observed":{"session_agent":"same-as-ungoverned","delegated":["low"]}}
```

The two runs **differ in `delegated` and agree in `session_agent`**. That pairing is the discrimination: it shows the projection reads each arm independently, and that the inert arm is inert rather than merely unread.

## What the record licenses

At `outbound-request`, this is evidence about what reached the model, not about what a config file contained.

- **The delegated path honors `effort:`.** A subagent's requests carry the fixture's own level, and they carry it in the presence of the same project-tier `effortLevel` the session path sees — so on this path, frontmatter wins.
- **The `--agent` path does not, in the presence of a project-tier `effortLevel`.** The session agent's requests sit at the ungoverned level despite the same frontmatter.

Do not read this as "Claude ignores agent effort" — one of the two measured paths honors it.

**The second finding is scoped deliberately, and the scope is a real limit.** The fixture pins `effortLevel: medium` at the project tier, so `session_agent == medium == ungoverned` admits two readings this apparatus cannot separate:

1. The `--agent` path drops agent-frontmatter effort, and resolution falls through to the project tier.
2. A project-tier `effortLevel` outranks agent frontmatter on the session path, while frontmatter still wins for subagents.

Both fit the measurement. The delegated arm rules out "project settings always win" as a blanket rule, but not the path-dependent version. Separating the two needs an arm with no `effortLevel` set at all, which this package does not have — see Residual gaps.

## Residual gaps

Both are stated here rather than left to be inferred.

- **Managed-tier settings.** Vendor documentation does not say whether they load regardless of `--setting-sources`. On a machine with managed policy, the ungoverned level could be set by something this runner did not exclude.
- **The project-tier `effortLevel` confound.** The fixture's `effortLevel: medium` keeps the ungoverned level stable and the record legible, but it also means the `--agent` finding cannot separate "frontmatter is dropped on this path" from "a project-tier `effortLevel` outranks frontmatter on this path." Resolving it needs an arm with no `effortLevel` set, which would trade this stability for a baseline at the model's own default — a value that is neither controlled nor drift-tracked here.
- **A project-tier-load regression is no longer detected.** The design gated that direction with an absolute baseline check; the relational assertion removes the gate. A regression moves the ungoverned level, which the projection absorbs — harmless to the _finding_, but not caught. A reader deciding whether this record still means what they think should see that stated rather than implied.

## Oracle limits

- **One model** (`claude-opus-4-8`) and **one CLI version** (recorded on each record's `tool_version`).
- **`model: inherit` is unmeasured.** This package measures the absent-`model:`-key shape — the shape agentspec emits once the effort design's null-serialization change lands. `inherit` names a value where this fixture names nothing, and `ClaudePreset.model` is a free string, so a preset naming `inherit` alongside an effort renders a shape no record covers.
- **The skill surface is out of scope here** — a skill's `effort:` and a mid-session typed slash command are separate questions, measured separately.

## This contradicts Claude Code's documentation

Claude Code documents `effort` as a subagent frontmatter field applying "when this subagent is active", and separately documents that an agent file runs both as a delegated subagent and as the main session under `--agent`. The `session_agent` arm measures the field as inert on the second path. Whether that is a bug or an incomplete document is a question for upstream; this record is the evidence either way.
