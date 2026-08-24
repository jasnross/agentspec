# Cursor — multiple comma-separated bracket options on subagents

**Question.** Does Cursor parse multiple comma-separated bracket options in subagent frontmatter and apply both to the resolved model?

**Why it matters.** `experiments/cursor-subagent-effort/` measured a **single** bracket option. Before the Cursor adapter composes `model[k=v,k=v]`, the separator itself has to be confirmed: agentspec would be the sole writer of that string, Cursor rejects nothing, and a delimiter mistake on this surface degrades silently into a model resolved without the options anyone intended.

Option **ordering** needs no probe. agentspec is the sole writer of the bracket string and emits one fixed order, so a rejected ordering would fail this same assertion.

**Driver:** `manual`. Cursor exposes no offline introspection command, so the oracle is its hook system: a person drives Cursor, but the answer lands in a `subagentStart` payload and no one has to interpret it.

## Running it

```sh
experiments/cursor-subagent-model-options/probe.sh
```

It arranges a throwaway workspace, prints the path, and blocks until the capture matches its `wait_for` filter. There is no resume path — a timeout ends the run and the next attempt is a fresh session.

## The assertion discriminates

Measured 2026-08-24 against **Cursor 3.17.8**.

| Arm | Declared `model:` | Resolved `subagent_model` | Reading |
| --- | --- | --- | --- |
| baseline, no brackets | `claude-opus-5` | `claude-opus-5-thinking-high` | the model's own default |
| **the probe arm** | `claude-opus-5[effort=low,fast=true]` | `claude-opus-5-thinking-low-fast` | **both options parsed and applied** |
| did not discriminate | `claude-opus-5[effort=low,fast=false]` | `claude-opus-5-thinking-low` | `fast=false` is the default, so it renders as nothing |
| corroboration, different family | `grok-4.6[effort=low,fast=true]` | `cursor-grok-4.6-low-fast` | both options, on a second model family |
| partial | `composer-2.5[effort=low,fast=true]` | `composer-2.5-fast` | `fast` rendered, `effort` did not |

The `grok-4.6` row is why this finding is not a single observation. The separator parses on two unrelated model families, which is stronger than one arm could be on its own.

### The baseline shares the probe arm's `subagent_type`

Both captures are of a subagent named `arm-two-options` — the baseline was taken first, then the same file was overwritten with its bracketed form and invoked again.

This matters more than it looks. The projection selects on `.subagent_type`, so a baseline captured under a _different_ name yields no match and the projection returns `null`. That refutes because **nothing matched**, not because a distinct input produced a distinct value — the "misspelled jq path returns `null` forever" failure the contract's discrimination rule exists to catch. The refuting dry-run here observes `claude-opus-5-thinking-high`, a real resolved model, which is what makes it a discrimination check rather than a wiring check.

### Both values are non-default, and that is mandatory

`effort=low` against a `thinking-high` default, and `fast=true` against a `false` default.

The flattened `subagent_model` **hides an option whose value equals the model's default**. The `[effort=low,fast=false]` row above is that failure in miniature: the comma parsed perfectly, and the evidence evaporated because `false` renders as nothing. Two arms in the original Cursor round were invalidated this way. An arm built on default values would record `refuted` against a provider that did exactly what was asked.

## Limits of this oracle

- **The flattened `subagent_model` hides default-valued options.** An option whose value equals the model's default is indistinguishable from no option at all. This is the single most important constraint when re-running: use non-default values.
- **Options are model-coupled.** `composer-2.5[effort=low,fast=true]` rendered `fast` but not `effort`. "This option is unobservable" is therefore always a claim about a model family, never about Cursor as a whole.
- **`status: "completed"` is not evidence of execution.** Resolution is confirmed; an outbound request carrying the resolved options is not.
- **Cursor's provider request is unobservable in principle.** All traffic, BYOK included, routes through Cursor's own backend, so the final hop is unreachable by any oracle Cursor exposes. This probe stops at `resolved-config` and cannot go further.
- **`model_params`, Cursor's only structured resolved-parameter payload, is parent-turn-only.** Every `subagentStart` payload carries `model_params: null`, so the flattened string is the whole oracle. `.model` and `.subagent_model` are identical on that event, so the parent field is no second source either.

## Why `version_source.kind` is `capture`

The Cursor version comes from `cursor_version` in the payload, not from a CLI command. `cursor-agent --version` reports a **different artifact on a different versioning scheme** than the IDE that fired the hook, so a command-sourced version would record a number unrelated to the thing measured.

The visible consequence: `just probe-status` computes no drift for this package and says so. That is designed, not broken — there is nothing to compare an installed version against, because the version is a property of the recorded session rather than of the machine reading the record.

## Related

`experiments/cursor-subagent-effort/` measures a single bracket option and is where the flattened-oracle limits above were first established. This package is the multi-option question, which could not live there: a manifest carries one `assertion` object, so a second finding needs a second package.

`TODO.md` #25 records the question this package's exploration session could **not** answer — whether Cursor's `[context=…]` option is observable at all. It is not, on `claude-opus-5`, at either 300k or 1m, alone or paired with `effort`.
