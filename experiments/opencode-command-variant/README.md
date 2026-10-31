# OpenCode — `variant:` on commands

**Question.** Does OpenCode read a top-level `variant:` key in command frontmatter, sibling to `model:`?

**Why it matters.** This package was written against a defect: `OpenCodeCommandFrontmatter` (`src/adapters/opencode.rs`) carried `model` but had no `variant` field, so agentspec resolved a variant and then dropped it on the way to every generated OpenCode command. The measurement below established that OpenCode does read the key, and the plan `thoughts/plans/2026-08-26-agentspec-set-1-opencode-frontmatter-fidelity.md` corrected the drop — the struct now carries `variant` beside `model`.

**Driver:** `unattended`. `opencode debug config --pure` makes no model request, needs no network and no credentials, and is deterministic.

## Running it

```sh
experiments/probe run opencode-command-variant
```

It materializes `fixtures/assertion/` into a fresh workspace's `project/` directory and runs the oracle from inside it, isolated: OpenCode gets its own home and XDG directories under the workspace, and its external skill scan is off, so the fixture command is the only command it resolves. The definition's assertion in `probe.py` reads the fixture command's `model` and `variant`, and the recorder writes a record under `results/`.

The discriminator arm runs the same way, as a dry run only; the harness refuses to record it:

```sh
PROBE_FIXTURE=discriminator PROBE_DRY_RUN=1 experiments/probe run opencode-command-variant
```

## The assertion discriminates

The two arms differ only in the presence of the `variant:` line. The samples under `samples/`, captured 2026-10-31 against opencode 1.18.34, show them yielding distinct values, and `just probe-test` re-proves it on every run:

| Sample | Arm | Frontmatter | Assertion |
| --- | --- | --- | --- |
| `confirming` | `fixtures/assertion/` | `model: anthropic/claude-sonnet-4-5` + `variant: high` | `{"model":"anthropic/claude-sonnet-4-5","variant":"high"}` |
| `refuting` | `fixtures/discriminator/` | `model:` alone | `{"model":"anthropic/claude-sonnet-4-5","variant":null}` |

`variant: high` is a non-default value — OpenCode's default is `null`, which is exactly what the discriminator arm shows — so a match is positive proof rather than a possible reading of the default.

The command's resolved name is its filename stem, which is why the assertion looks up `agentspec-probe-variant` rather than anything in the frontmatter.

## The surface divergence is the finding

The same `variant:` key that OpenCode reads here is **parsed and discarded on the skill surface**. `experiments/opencode-skill-frontmatter-discard/` measures that, from a different oracle, against the same OpenCode version.

Neither package depends on the other and each stands alone; read together they are a measured instance of the standing argument for counting surfaces rather than providers. A rendering confirmed on one surface says nothing about another, even within one provider, even for the same key name.

### An assertion copied between packages will not compare equal

`opencode debug config` resolves a command's `model` to the **bare string** `"anthropic/claude-sonnet-4-5"`. `opencode debug agent` resolves an agent's `model` to the **parsed object** `{"providerID":"anthropic","modelID":"claude-sonnet-4-5"}` — see `experiments/opencode-agent-variant/samples/confirming.json`.

That is a real provider finding, not a transcription slip in either package. Because the recorder compares structurally, an assertion lifted from one package into the other refutes on the `model` half while the `variant` half agrees, and the diagnostic points at the value rather than at the copy.

## Limits of this oracle

**`resolved-config`, not `outbound-request`.** This proves OpenCode surfaced the key in its resolved config. It does not prove the variant survives request build — OpenCode collapses an unrecognized variant at request-build time, well after this oracle has printed it. Reaching that hop needs a different oracle.

**The oracle resolves every command OpenCode can find, not only the workspace's.** `--pure` excludes external plugins; it does not exclude global config. The run is isolated, so OpenCode finds no global config of the operator's, and the fixture is the only command it resolves; before isolation, the authoring run resolved 29 commands, 28 of them the operator's. The assertion still looks the fixture up by a name distinctive enough that a collision is implausible.

**The oracle resolves the whole config rather than addressing one command, so a missing fixture would refute rather than fail.** On a workspace where the fixture never resolved, the assertion yields `{"model":null,"variant":null}`, and the recorder writes `refuted` — the contract's strong "the provider changed" signal, raised by an apparatus failure, into an append-only directory. The definition guards this with a judge function asserting the command resolved, which refuses the run before anything is recorded when it did not.

Note that this failure is distinct from the discriminator arm's, which refutes on a **present** command whose `variant` resolved to `null`. Both are `refuted`; only one is a measurement.

**`opencode debug` truncates its stdout at exactly 65536 bytes when stdout is a pipe.** Unisolated, `opencode debug config --pure` emitted roughly 458 KB, far over that cliff, and the same command piped returned exactly 65536 bytes, which no JSON parser accepts. Isolated, it emits about 400 bytes, but the debug family still redirects the oracle to a file rather than a pipe, so no output size can cut the document short.
