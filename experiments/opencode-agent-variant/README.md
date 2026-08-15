# OpenCode — `variant:` on agents

**Question.** Does OpenCode read a top-level `variant:` key in agent frontmatter, sibling to `model:`?

**Why it matters.** agentspec emits `variant:` as a sibling key for OpenCode agents. A design revision once asserted a `provider/model#variant` suffix instead — a rendering no parser in OpenCode would ever have accepted. This probe is the measurement that settles which of the two is real.

**Driver:** `script`. There is no human step. `opencode debug agent <name>` makes no model request, needs no network and no credentials, and is deterministic — the cheapest oracle of the three providers.

## Running it

```sh
experiments/opencode-agent-variant/probe.sh
```

It materializes `fixtures/assertion/` into a throwaway workspace, runs the oracle from inside it, and hands the raw output to `experiments/lib/record.sh`, which writes a record under `results/`.

## The assertion discriminates

Measured 2026-08-15 against opencode 1.18.18, the two fixture arms project to distinct values:

| Arm | Frontmatter | `{model, variant}` |
| --- | --- | --- |
| `fixtures/assertion/` | `model: anthropic/claude-sonnet-4-5` + `variant: high` | `{"model":{"providerID":"anthropic","modelID":"claude-sonnet-4-5"},"variant":"high"}` |
| `fixtures/discriminator/` | `model: anthropic/claude-sonnet-4-5#high` | `{"model":{"providerID":"anthropic","modelID":"claude-sonnet-4-5#high"},"variant":null}` |

`variant: high` is a non-default value — OpenCode's default is `null`, as the discriminator arm shows — so a match is positive proof rather than a possible reading of the default.

The discriminator arm exists only for this check. It is never recorded; `probe.sh` runs the assertion arm alone.

## Limits of this oracle

`opencode debug agent` prints the **declared** variant from `Agent.Info`. The variant→`{}` collapse for a model lacking the `reasoning` capability happens later, at request-build time. This oracle therefore proves OpenCode **read** the field — depth `resolved-config` — not that it will act on it. Reaching the outbound request would need a different oracle.

## Related, not covered here

OpenCode's **skill** schema declares only `name`, `description`, and `slash`; `model`, `variant`, and `tools` are parsed as frontmatter and discarded. That is a separate question about a separate surface, so under the one-package-one-question rule it belongs to a probe that does not exist yet.
