# claude-skill-effort

**Question:** When a skill is model-invoked mid-session, supplied as the session's entry prompt, or forked, does Claude Code apply its `effort:` frontmatter to the outbound model requests it governs?

**Driver:** `billed`. A command answers the question with no human step, but every arm spends a billed model call, so `just probe-run` skips this package by name and reason; it runs only under `just probe-run --live`.

**Depth:** `outbound-request`. The oracle is the request body Claude Code sends, captured through OpenTelemetry — the far end of the chain, not the provider's resolved view.

## Why it matters

agentspec emits `effort:` into Claude skill files. Whether that field reaches the model decides whether the emission is load-bearing or decorative — and the answer turns out to depend on _how the skill is invoked_, which is not something agentspec's own tests can see.

The question is not academic for this repository. A large share of agentspec-emitted skills are invoked **by name** rather than chosen by the model, and that path is measured honored here — so the decision the effort design inherits is "keep emitting `effort:` into skill files", not "consider dropping it".

## The three arms

| Arm | Invocation | Prompt | Fixture tree |
| --- | --- | --- | --- |
| `inline` | `--allowedTools Skill`; Claude chooses the skill itself, mid-session, through the Skill tool | `Run the agentspec effort probe check.` | `fixtures/<fixture>` |
| `slash_entry` | the slash command **is** the session's `-p` entry prompt | `/probe-effort run the check` | `fixtures/<fixture>` |
| `fork` | `--allowedTools Skill` against a tree whose skill declares `context: fork` | `Run the agentspec effort probe check.` | `fixtures/<fixture>-fork` |

The prompts above are the ones that actually activated the skill, not idealized versions of them. The skill's `description` is deliberately a narrow, unambiguous trigger (`Use when the user asks to run the agentspec effort probe check.`) so the `inline` arm's natural-language prompt selects it reliably.

`fork` gets a separate fixture **tree** rather than a second skill directory beside the first: a skill's directory name is its identity, so two directories in one workspace would be two differently-named skills, and the `inline` arm could activate the wrong one.

### The skill surface has three paths; this package measures two of them

| Path | How reached | Coverage |
| --- | --- | --- |
| Model-invoked mid-session | Claude chooses it | `inline` — measured |
| `/name` as the `-p` prompt | non-interactive session entry | `slash_entry` — measured |
| `/name` typed mid-session | interactive, the common case | **unmeasured** — see `TODO.md` |

The arm is named `slash_entry` rather than `slash` for exactly this reason. It measures a slash command supplied as the **session entry**, not one typed into a running session. Claude Code's own harness text instructs the model to invoke a typed `/<skill-name>` **through the Skill tool**, which would make it mechanically the `inline` path — and therefore inert. That is instruction text rather than a measurement, which is precisely the kind of belief this repository exists to stop people designing against.

The third cell cannot become a fourth arm here: reaching it needs a multi-turn interactive session, and `claude -p` is one turn. It is a `manual` driver and a separate apparatus.

## The oracle

`CLAUDE_CODE_ENABLE_TELEMETRY=1` plus `OTEL_LOG_RAW_API_BODIES=file:<dir>` writes each untruncated request body to `<dir>/<uuid>.request.json`. The runner globs `*.request.json` specifically — the sink writes `*.response.json` beside them, and a bare `*.json` glob would read a response-only sink as a populated arm.

Request counts are never compared against an expected number. Across this package's own runs the three arms produced one, three, and four requests.

## The assertion is relational

Each arm's level is compared against **the requests nothing governed in the same run**, not against a pinned absolute level:

- `"same-as-ungoverned"` means the arm ran at the same effort as the ungoverned requests captured in that run.
- An array such as `["low"]` means the arm ran at a level the ungoverned requests did not.

The control set is pooled across arms, and gate 3 licenses the pooling by requiring the whole ungoverned set to agree on one level — a run whose arms sat at different ungoverned levels is refused rather than silently averaged. This matters more here than in `claude-agent-effort`: the `slash_entry` arm produced a single request, and that request is governed, so it contributes nothing to its own baseline.

**No term in `expected` names an external fact** — not a baseline, not the model's default effort, not the operator's subscription tier. Every term compares two things this run measured. The reasoning is the same as `claude-agent-effort`'s, and is written out in full there.

### Governed vs. ungoverned is decided by `.messages`, not `.system`

This is where the skill package diverges from `claude-agent-effort`, and the divergence is a measurement, not a preference.

An agent file **becomes the system prompt** of the request it governs, so that package defines "governed" as the marker appearing in `.system`. A skill's body never reaches `.system` at all. Measured at 2.1.232, it arrives in `messages[]`:

| Arm | Where the skill body lands |
| --- | --- |
| `inline` | a `user` message, in a `tool_result` block, on the ongoing main thread |
| `slash_entry` | `messages[0]`, as a text block |
| `fork` | the forked subagent's `messages[0]`, as a text block |

So this package passes `.messages` to both gates and selects on `.messages` in both directions of the projection. The gates take the field as a parameter for this reason — see `experiments/lib/probe-claude-otel.sh`. Widening the shared default to "`.system` or `.messages`" would reintroduce the echo `claude-agent-effort` measured and excluded, and inlining a second copy of the gates here would put the safety-critical part of a billed apparatus in two files that can drift.

The `inline` row is itself the mechanism behind that arm's finding: a model-invoked skill does not get a request of its own. Its body is injected into the ongoing main-thread conversation as a tool result, on a request whose effort was already settled — so there is no request for `effort:` to govern.

**The echo hazard is reduced here but not eliminated.** `claude-agent-effort` narrowed to `.system` because a subagent's reply carried the marker back to the main thread. This package's `fork` arm has the same shape, and its main-thread requests were measured carrying no marker — but that holds only because the skill body instructs a one-word reply. A future fixture whose skill made Claude quote its own body back would drag an ungoverned level into the `fork` arm's value. The one-word reply is load-bearing, not cosmetic.

`unique` rather than a positional selector, in all three arms: the sink names files by UUID, so glob order is not write order, and `| last` would name an arbitrary element while silently discarding disagreement among the rest. **`unique` earns its keep most in the `fork` arm**, which returns to a main thread still at the ungoverned level and is therefore the arm most likely to hold marked requests at two levels — a two-element array that `unique` surfaces and a positional selector would hide. All three arms are spelled identically, including the ones where nothing could go wrong, so a reviewer who has checked one has checked them all.

`rel` is self-guarding: it renders `no-usable-control-set` when the ungoverned set is not exactly one level, and `arm-had-no-governed-request` when an arm has none. The runner's gates block both shapes, but `record.sh --dry-run --view <saved-view>` runs the projection gate-free, and that is the workflow this README points authors at.

## The three gates

Every gate asserts on a positive signal in the captured bodies. None of them names an effort level.

1. **Every arm captured at least one `*.request.json`.** An empty sink means the arm never ran.
2. **Every arm holds at least one request the fixture governs** (marker in `.messages`). Without it, "the fixture never engaged" and "the fixture engaged and Claude discarded its effort" are indistinguishable — which would make the most decision-relevant finding the least trustworthy one. **Gate 2 is what makes the `inline` arm assertable**, and it earned that keep during authoring: the first discriminate run failed this gate against the inherited `.system` definition, which is how the `.messages` finding above was discovered rather than silently mis-measured.
3. **At least one ungoverned request declares `output_config.effort`, and they all agree on one level.** The filter is an explicit exclusion of requests carrying no `effort` key: Claude emits an intermittent title-generation sidecar whose `output_config` holds a `format` object and no `effort`, and a control stated over all unmarked requests would fail on it every time it appeared. Written as an exclusion, the other direction stays loud — a Claude that stopped populating `effort` on governed requests empties the control set and trips the existence clause.

A gate failure is a statement about the run, not about Claude, and writes no record.

The gates live in `experiments/lib/probe-claude-otel.sh` rather than in this runner, shared with `claude-agent-effort` and covered by `experiments/lib/tests/probe-claude-otel.bats` against fabricated views — including coverage that the field parameter actually scopes both gates. Duplicated in two runners, the only thing that would ever exercise them is a real, paid run: an untested control on a billed apparatus.

## Isolation

The runner performs four isolations, each closing a way the measurement could become about the operator's machine rather than about Claude:

- **`env -u CLAUDE_CODE_EFFORT_LEVEL`** — that variable outranks frontmatter, so an exported one would make the probe measure the operator's shell.
- **`--setting-sources project`** — excludes the user tier outright rather than out-ranking it.
- **`--model claude-opus-4-8`** — pins the value domain. The only requirement is that it serves `low` and `max`; nothing depends on its default effort.
- **`--max-budget-usd 0.50`** — print-mode only, and counts subagent spend. A cap hit stops subagent spawns, which surfaces as a gate-2 failure rather than as a wrong answer — loud, which is why the cap is generous rather than tight.

No `--effort` flag is passed; it would outrank frontmatter too.

Each fixture's own `.claude/settings.json` pin (`effortLevel: medium`) is **not** an isolation — it is a fixture choice that keeps the ungoverned level stable across runs and the record legible. Nothing in `expected` depends on its value. It is copied into this package rather than shared with `claude-agent-effort`, per the contract's no-dependency rule; the identical content is that rule working as intended.

## Discriminate evidence

Both rows below come from `record.sh --dry-run` output on actual runs, not from a design document. The two fixture pairs differ only in the skill's `effort:` value. The `status` and `observed` fields are excerpted from the full seven-key record each run printed; the other five keys are the manifest's own values plus the date and version.

```
PROBE_FIXTURE=discriminator PROBE_DRY_RUN=1 experiments/claude-skill-effort/probe.sh
{"status":"refuted","observed":{"inline":"same-as-ungoverned","slash_entry":["max"],"fork":["max"]}}

PROBE_DRY_RUN=1 experiments/claude-skill-effort/probe.sh
{"status":"confirmed","observed":{"inline":"same-as-ungoverned","slash_entry":["low"],"fork":["low"]}}
```

The two runs **differ in `slash_entry` and `fork` while agreeing in `inline`**. That pairing is the discrimination: it shows the projection reads each arm independently, and that the inert arm is inert rather than merely unread.

## What the record licenses

At `outbound-request`, this is evidence about what reached the model, not about what a config file contained.

- **The session-entry path honors `effort:`.** A skill supplied as the `-p` prompt governs a request carrying the fixture's own level.
- **The forked path honors `effort:`.** A `context: fork` skill's subagent requests carry the fixture's own level.
- **The model-invoked path does not.** A skill Claude selects itself mid-session has its body injected into the ongoing conversation as a tool result, on a request already sitting at the ungoverned level.

**Do not read this as "Claude ignores skill effort."** Two of the three measured paths honor it. For a model-invoked skill, `effort:` reaches the file and not the model — that is the whole of what the `inline` result licenses.

Nor is the finding exhaustive over the skill surface: a slash command typed into a running interactive session is unmeasured, and the three-path table above says so.

## Residual gaps

- **Managed-tier settings.** Vendor documentation does not say whether they load regardless of `--setting-sources`. On a machine with managed policy, the ungoverned level could be set by something this runner did not exclude.
- **A project-tier-load regression is not detected.** The relational assertion absorbs a moved ungoverned level, which is harmless to the _finding_ but means the regression is not caught.
- **The `fork` arm's echo protection is the fixture's one-word reply**, not a structural narrowing — see "Governed vs. ungoverned" above.

## Oracle limits

- **One model** (`claude-opus-4-8`) and **one CLI version** (recorded on each record's `tool_version`).
- **`effort:` on `SKILL.md` is attested by search snippets rather than by a verbatim vendor documentation quote.** The skills documentation page would not yield its frontmatter table to repeated direct fetches during planning. That is a reason this package exists, and it is worth stating so a future reader does not go looking for a citation that is hard to retrieve. This record is the measurement that stands in for it.
- **The agent surface is out of scope here** — an agent's `effort:` is a separate question, measured in `claude-agent-effort`.
