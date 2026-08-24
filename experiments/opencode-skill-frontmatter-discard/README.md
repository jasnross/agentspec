# OpenCode — `model`, `variant`, and `tools` on skills

**Question.** Does OpenCode discard `model`, `variant`, and `tools` from skill frontmatter, resolving the skill to `name`, `description`, `location`, and `content` alone?

**Why it matters.** `OpenCodeSkillFrontmatter` (`src/adapters/opencode.rs`) emits all three fields into every generated OpenCode skill file. `tools` is emitted unconditionally, so every skill agentspec writes carries a dead `tools:` block. If the three are discarded, agentspec is writing bytes no OpenCode stage reads.

This is the harness's first negative finding — the first probe whose **confirmed** answer is that a provider ignores a rendering, as distinct from a probe that happens to have recorded a `refuted`. It is expressible as a positive signal because the discard shows up as a **complete, enumerable key set** rather than as a missing error; the contract forbids asserting on the absence of an error, and this probe never does.

**Driver:** `unattended`. `opencode debug skill --pure` makes no model request, needs no network and no credentials, and is deterministic.

## Running it

```sh
experiments/opencode-skill-frontmatter-discard/probe.sh
```

It materializes `fixtures/assertion/` into a throwaway workspace, runs the oracle from inside it, and hands the raw output to `experiments/lib/record.sh`, which writes a record under `results/`.

## The assertion discriminates

Measured 2026-08-24 against opencode **1.18.21**.

The assertion is a conjunction and both halves are load-bearing. The **key set** carries the discard finding; the **description** proves the frontmatter under test was actually read.

| Arm | Frontmatter | `{keys, description}` |
| --- | --- | --- |
| `fixtures/assertion/` | `model` + `variant` + `tools` present | `{"keys":["content","description","location","name"],"description":"agentspec skill-frontmatter discard probe"}` |
| `fixtures/discriminator/` | none of the three present | `{"keys":["content","description","location","name"],"description":"discriminator arm - no dropped fields present"}` |
| `fixtures/corroboration/` — the **agent** surface | same three fields | `{"model":{"providerID":"anthropic","modelID":"claude-sonnet-4-5"},"variant":"high","tools":{…,"bash":false,…}}` |

### Why `keys` alone is not the assertion

A projection of the key set alone **cannot discriminate, and would record a false `confirmed`.** Every resolved skill returns the identical four-key set — the fixture, OpenCode's built-ins, and the operator's global skills alike. Against the assertion arm's 29 resolved skills:

```console
$ jq -c '[.[] | keys] | unique' view.json
[["content","description","location","name"]]
```

One entry, for every skill OpenCode can resolve. A `keys`-only projection is therefore constant across every possible input and would pass whether or not the fixture was ever read.

Adding `slash: true` — the one further field OpenCode's skill schema declares beyond `name` and `description` — does not change it either. A throwaway fixture carrying `slash: true` resolved to `{"keys":["content","description","location","name"],"description":"slash trial"}`, so `slash` is not a way to make the key set track its input.

That trial arm is **not committed**: it is a one-off observation against opencode 1.18.21 on 2026-08-24, recorded here so a reader knows the alternative was tried rather than assumed. Reproducing it is a two-line fixture and one `opencode debug skill --pure`.

The `description` half is what makes the projection track the file under test, and the discriminator arm is what shows it: same oracle, same projection, distinct value.

### Why the agent arm is evidence rather than the discriminator

The corroboration arm carries the **same three fields** on the **agent** surface, where OpenCode honors them. That is what establishes the assertion arm's absent keys are a discard rather than a malformed fixture — the fields are well-formed and read somewhere.

It is deliberately **not** the machine-checked discriminator. `opencode debug agent` emits a single object rather than a list, so this package's skill projection errors on it and `record.sh` hard-fails instead of reporting `refuted`. A hard failure is not a refutation, and the contract's discrimination rule wants a distinct value. The discriminator skill arm supplies that; the agent arm supplies this README's evidence.

## Limits of this oracle

**`resolved-config`, not `outbound-request`.** This proves OpenCode did not surface the three fields in its resolved skill record. It does not prove no later stage would have used them — only that nothing downstream can read them from here.

**The oracle enumerates the operator's global skills, not only the workspace's.** `--pure` excludes external plugins; it does not exclude global config. The projection therefore selects by `name`, and the fixture name `agentspec-probe-discard` is distinctive enough that a collision is implausible. The authoring run confirmed exactly one match.

**`location` is deliberately absent from the projection.** It resolves to an absolute path inside a throwaway workspace, so no fixed `expected` could match it. It is still present in the asserted key set, which is where it carries its weight.

**The oracle is enumerating, not name-addressed, so a missing fixture would refute rather than fail.** `opencode debug skill --pure` lists every resolvable skill; it does not take a name. A workspace where the fixture never resolved therefore exits 0 with a valid view of all the _other_ skills, the projection yields `null`, and `record.sh` compares `null` against `expected` and writes `refuted` — the contract's strong "the provider changed" signal, raised by an apparatus failure, into an append-only directory. The runner guards this with a preflight asserting the fixture resolved exactly once, and exits before `record.sh` when it did not. The sibling `opencode-agent-variant` needs no such guard because `opencode debug agent <name>` is name-addressed and exits nonzero on a missing agent.

This is not hypothetical drift: skills are discovered under both `.opencode/skill/` and `.opencode/skills/`, and which of those OpenCode honors is itself unmeasured (`TODO.md` #16). A change there is exactly what the preflight catches.

**`opencode debug` truncates its stdout at exactly 65536 bytes when stdout is a pipe.** `opencode debug skill --pure` emits roughly 338 KB, well over that cliff, at which point `jq` reports `Unfinished string at EOF`. The runner redirects to a file rather than piping for this reason; a pipe would silently hand `record.sh` a half-written JSON document.

## Related

`experiments/opencode-agent-variant/` measures the same `variant:` key on the **agent** surface, where OpenCode honors it. Surfaces diverging within one provider is the standing argument for counting surfaces rather than providers, and these two packages are a measured instance of it: the same key, read on one surface and discarded on the other.
