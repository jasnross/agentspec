# OpenCode — `variant:` on agents

**Question.** Does OpenCode read a top-level `variant:` key in agent frontmatter, sibling to `model:`?

**Why it matters.** agentspec emits `variant:` as a sibling key for OpenCode agents. A design revision once asserted a `provider/model#variant` suffix instead — a rendering no parser in OpenCode would ever have accepted. This probe is the measurement that settles which of the two is real.

**Driver:** `unattended`. There is no human step and nothing to opt into. `opencode debug agent <name>` makes no model request, needs no network and no credentials, and is deterministic — the cheapest oracle of the three providers.

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

## The oracle's other reach

`opencode debug skill --pure` prints every resolved skill as JSON, on the same terms — no model request, no network, no credentials:

```sh
opencode debug skill --pure | jq '.[] | select(.name=="<name>") | keys'
```

## Related findings, from the same oracle but not this probe

These came out of hand-run probes on adjacent surfaces. Two have since been given packages of their own; the rest have no records, and under the one-package-one-question rule each would be its own package.

**Now measured, so no longer prose:**

- **OpenCode's skill schema declares only `name`, `description`, and `slash`** (`packages/core/src/skill.ts:33-38`). `model`, `variant`, **and `tools`** are parsed as frontmatter and discarded — a resolved skill record is exactly `{content, description, location, name}`. `tools` matters most in practice: agentspec emits it as a non-`Option` map, so every generated OpenCode skill file carries a dead `tools:` block, whereas `model`/`variant` appear only when a preset applies. → **`experiments/opencode-skill-frontmatter-discard/`**, recorded against 1.18.21.
- **OpenCode's command schema does read `variant`**, unlike the skill schema. Surfaces diverging within one provider is the standing example of why a rendering confirmed on one surface says nothing about another. → **`experiments/opencode-command-variant/`**, recorded against 1.18.21. Note the two resolve `model` differently — a bare string on commands, a `{providerID, modelID}` object on agents — so a projection copied between that package and this one will not compare equal.

**Still prose:**

**Provenance for these two:** opencode **1.18.15**, source `anomalyco/opencode` @ `cc4b456`, probed **2026-08-12** and **2026-08-15**. That is a different version from the 1.18.18 the measured content above was taken against, and neither claim has been re-checked since. Provenance is the only thing separating them from the guesses this harness exists to expose, which is why it is stated rather than assumed.

- **Skills are discovered under both `.opencode/skill/` and `.opencode/skills/`**, so agentspec's `skills` directory name resolves correctly. The singular is incidentally demonstrated — `opencode-skill-frontmatter-discard` places its fixture under `.opencode/skill/` and records `confirmed` — but nothing measures the plural, and no package asks the question. `TODO.md` #16 stays open for it.
- **A `variant:` with no `model:` is accepted silently** and is inert per OpenCode's schema annotation. This one is about the very surface this package probes, but it is a different question — "is it accepted" rather than "is it read" — so it stays prose until someone gives it a package.

The five 2026-08-12 probes wrote **zero bytes to stderr**, which is the direct evidence behind the "no error, no warning, no log line" degradation claim in the contract. The 2026-08-15 skill probe was run with stderr suppressed, so it makes no such claim — a hedge worth preserving, since the whole point of the claim is that silence is not proof.

Each remaining finding is a candidate for a probe. Until one exists, the two under **Still prose** are prose here rather than measurements — which is the distinction this harness is built to keep visible, and which the two under **Now measured** stopped being on 2026-08-24.
