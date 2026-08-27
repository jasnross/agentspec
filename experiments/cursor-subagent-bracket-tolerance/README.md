# Cursor — bracket tolerance for an unobservable option

**Question.** Does a bracket carrying an option Cursor's oracle cannot observe still apply the options it can?

**Why it matters.** This measurement precedes the feature it de-risks, which is the harness's normal order. The design it gates would give `CursorPreset` a named field for every documented Cursor model option and have the adapter compose them into one `model[k=v,k=v]` suffix. One of those options, `context`, is invisible to every oracle Cursor exposes — so agentspec would be emitting a token it cannot see land. The risk is not that `context` does nothing. It is that `context` costs the options beside it their effect: Cursor falls back to the parent conversation's model when a model specification fails to resolve, so a bracket Cursor rejected outright would silently strip `effort` too, and the generated file would look correct while the run thought at the wrong depth.

This package measures whether `context` is **harmless**, never whether it is **honored**. Those are different claims and only the first is reachable. It therefore does **not** discharge repo `TODO.md` #25, which stays open for the oracle question.

**Driver:** `manual`. Cursor exposes no offline introspection command, so the oracle is its hook system: a person drives Cursor, but the answer lands in a `subagentStart` payload and no one has to interpret it.

## Running it

```sh
experiments/cursor-subagent-bracket-tolerance/probe.sh
```

It arranges a throwaway workspace, prints the path, and blocks until the capture matches its `wait_for` filter. There is no resume path — a timeout ends the run and the next attempt is a fresh session.

## The assertion discriminates

Measured 2026-08-27 against **Cursor 3.17.19**.

| Arm | Declared `model:` | Resolved `subagent_model` | Reading |
| --- | --- | --- | --- |
| baseline, no brackets | `claude-opus-5` | `claude-opus-5-thinking-high` | the model's own default |
| **the probe arm** | `claude-opus-5[effort=low,context=300k]` | `claude-opus-5-thinking-low` | **the bracket parsed, and `effort` survived beside `context`** |

Both arms were re-evaluated against the committed manifest with `record.sh --dry-run`, so the expression validated is the expression the manifest carries:

```text
bracketed → confirmed — observed "claude-opus-5-thinking-low"
baseline  → refuted   — observed "claude-opus-5-thinking-high"
```

The refuting arm observes a **real resolved model**, not `null`. That distinction is the whole point of the check: `null` would mean the projection matched nothing, which is a wiring bug wearing a refutation's clothes.

### The baseline shares the probe arm's `subagent_type`

Both captures are of a subagent named `arm-bracket-tolerance`. The projection selects on `.subagent_type`, so a baseline captured under a _different_ name yields no match and the projection returns `null`. That would refute because **nothing matched**, not because a distinct input produced a distinct value — the "misspelled jq path returns `null` forever" failure the contract's discrimination rule exists to catch.

### The two arms ran in separate workspaces

The recorded run captured the bracketed arm. The baseline was then taken in a **second workspace**, arranged by hand from the same fixtures with the bare model substituted, and evaluated only with `record.sh --dry-run`.

Two workspaces rather than one file, because the projection ends in `| first`: a baseline appended to the recorded capture would leave `first` returning the bracketed value, and the refutation check would pass while measuring nothing. The baseline is also deliberately kept off the runner's recording path — a baseline arm is _supposed_ to refute, and a recorded `refuted` would be a permanent false finding in an append-only directory.

### `effort=low` is non-default, and that is mandatory

`claude-opus-5` defaults to `thinking-high`, so `effort=low` is positive proof rather than a possible reading of the default. The flattened `subagent_model` **hides an option whose value equals the model's default** — `experiments/cursor-subagent-model-options/` records two arms invalidated exactly that way. An arm built on `effort=high` here would record `refuted` against a Cursor that did precisely what was asked.

`context=300k` has no such guarantee, and that is the point of the package rather than a flaw in it: the arm is constructed so that the option carrying the evidence is the one that _can_ carry it.

## Limits of this oracle

- **The flattened `subagent_model` hides default-valued options.** A pass says nothing about any option whose value coincides with the model's default.
- **`context` is invisible either way.** Present-and-honored and dropped-as-unknown produce the same string. What a pass licenses is "the bracket cost `effort` nothing" — never "Cursor honors `context`". `TODO.md` #25 is where that second question lives.
- **Options are model-coupled.** `experiments/cursor-subagent-model-options/` recorded `effort` rendering on `claude-opus-5` and `grok-4.6` but not on `composer-2.5`. Tolerance measured on one model family is not tolerance measured on Cursor as a whole.
- **`status: "completed"` is not evidence of execution.** Resolution is confirmed; an outbound request carrying the resolved options is not.
- **Cursor's provider request is unobservable in principle.** All traffic, BYOK included, routes through Cursor's own backend, so the final hop is unreachable by any oracle Cursor exposes. This probe stops at `resolved-config` and cannot go further.
- **`model_params` is parent-turn-only.** Every `subagentStart` payload carries `model_params: null`, so the flattened string is the whole oracle. `.model` and `.subagent_model` are identical on that event, so the parent field is no second source either.

## Why `version_source.kind` is `capture`

The Cursor version comes from `cursor_version` in the payload, not from a CLI command. `cursor-agent --version` reports a **different artifact on a different versioning scheme** than the IDE that fired the hook, so a command-sourced version would record a number unrelated to the thing measured.

The visible consequence: `just probe-status` computes no drift for this package and says so. That is designed, not broken — there is nothing to compare an installed version against, because the version is a property of the recorded session rather than of the machine reading the record.

## Related

`experiments/cursor-subagent-model-options/` measures that Cursor parses a comma separator and applies both options. This package is the narrower follow-on: it holds the separator fixed and varies whether one of the two options is one the oracle can see at all.

`experiments/cursor-subagent-effort/` measures a single bracket option and is where the flattened-oracle limits above were first established.

`TODO.md` #25 asks whether `context` is observable at all, and is **not** discharged by this package. The two are easy to confuse: this one asserts `claude-opus-5[effort=low,context=300k]` → `claude-opus-5-thinking-low`, which is a claim about the options beside `context`, not about `context` itself.
