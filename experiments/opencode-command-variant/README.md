# OpenCode — `variant:` on commands

**Question.** Does OpenCode read a top-level `variant:` key in command frontmatter, sibling to `model:`?

**Why it matters.** This package was written against a defect: `OpenCodeCommandFrontmatter` (`src/adapters/opencode.rs`) carried `model` but had no `variant` field, so agentspec resolved a variant and then dropped it on the way to every generated OpenCode command. The measurement below established that OpenCode does read the key, and the plan `thoughts/plans/2026-08-26-agentspec-set-1-opencode-frontmatter-fidelity.md` corrected the drop — the struct now carries `variant` beside `model`.

**Driver:** `unattended`. `opencode debug config --pure` makes no model request, needs no network and no credentials, and is deterministic.

## Running it

```sh
experiments/opencode-command-variant/probe.sh
```

It materializes `fixtures/assertion/` into a throwaway workspace, runs the oracle from inside it, and hands the raw output to `experiments/lib/record.sh`, which writes a record under `results/`.

## The assertion discriminates

Measured 2026-08-24 against opencode **1.18.21**. The two arms differ only in the presence of the `variant:` line and project to distinct values:

| Arm | Frontmatter | `{model, variant}` |
| --- | --- | --- |
| `fixtures/assertion/` | `model: anthropic/claude-sonnet-4-5` + `variant: high` | `{"model":"anthropic/claude-sonnet-4-5","variant":"high"}` |
| `fixtures/discriminator/` | `model:` alone | `{"model":"anthropic/claude-sonnet-4-5","variant":null}` |

`variant: high` is a non-default value — OpenCode's default is `null`, which is exactly what the discriminator arm shows — so a match is positive proof rather than a possible reading of the default.

The command's resolved name is its filename stem, which is why the projection keys on `agentspec-probe-variant` rather than on anything in the frontmatter.

## The surface divergence is the finding

The same `variant:` key that OpenCode reads here is **parsed and discarded on the skill surface**. `experiments/opencode-skill-frontmatter-discard/` measures that, from a different oracle, against the same OpenCode version.

Neither package depends on the other and each stands alone; read together they are a measured instance of the standing argument for counting surfaces rather than providers. A rendering confirmed on one surface says nothing about another, even within one provider, even for the same key name.

### A projection copied between packages will not compare equal

`opencode debug config` resolves a command's `model` to the **bare string** `"anthropic/claude-sonnet-4-5"`. `opencode debug agent` resolves an agent's `model` to the **parsed object** `{"providerID":"anthropic","modelID":"claude-sonnet-4-5"}` — see `experiments/opencode-agent-variant/probe.json`.

That is a real provider finding, not a transcription slip in either manifest. Because `record.sh` compares structurally, a projection lifted from one package into the other refutes on the `model` half while the `variant` half agrees, and the diagnostic points at the value rather than at the copy.

## Limits of this oracle

**`resolved-config`, not `outbound-request`.** This proves OpenCode surfaced the key in its resolved config. It does not prove the variant survives request build — OpenCode collapses an unrecognized variant at request-build time, well after this oracle has printed it. Reaching that hop needs a different oracle.

**The oracle enumerates the operator's global commands, not only the workspace's.** `--pure` excludes external plugins; it does not exclude global config. The authoring run resolved 29 commands, 28 of them the operator's — so that count is machine-specific and a clean checkout will see a different one. What transfers is that the fixture must be the single match. The projection therefore keys on a name distinctive enough that a collision is implausible.

**The oracle resolves the whole config rather than addressing one command, so a missing fixture would refute rather than fail.** `.command["agentspec-probe-variant"]` on a workspace where the fixture never resolved is `null`, the projection yields `{"model":null,"variant":null}`, and `record.sh` writes `refuted` — the contract's strong "the provider changed" signal, raised by an apparatus failure, into an append-only directory. The runner guards this with a preflight asserting the command resolved, and exits before `record.sh` when it did not.

Note that this failure is distinct from the discriminator arm's, which refutes on a **present** command whose `variant` resolved to `null`. Both are `refuted`; only one is a measurement.

**`opencode debug` truncates its stdout at exactly 65536 bytes when stdout is a pipe.** `opencode debug config --pure` emits roughly 458 KB, so it is far over that cliff; the same command piped returns exactly 65536 bytes and `jq` reports `Unfinished string at EOF`. The runner redirects to a file rather than piping for this reason.
