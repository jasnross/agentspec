# Provider Rendering Verification

agentspec's tests compare emitted bytes against agentspec's own belief about what each provider reads. Nothing in the test suite verifies that belief against the provider itself — and the belief has been wrong. This document records what has actually been confirmed against a running provider, what is still inferred from vendor documentation, and how each confirmation was obtained.

Its purpose is narrow: **a contributor with only a clone should be able to tell "confirmed against the provider" from "inferred from vendor docs."** Without that distinction, a documented guess is indistinguishable from a verified fact, which is precisely how a design revision once proposed an OpenCode `provider/model#variant` model-reference suffix that no parser in OpenCode would ever have accepted.

## Why absence of error proves nothing

All three providers degrade silently on a rendering they don't understand:

- **OpenCode** looks an unrecognized variant up in a per-model map and collapses the miss to `{}` via `mergeOptions` — no error, no warning, no log line.
- **Claude** clamps an unsupported effort level down to the highest supported one at or below it, without warning.
- **Cursor** falls back to the parent conversation's model when a model specification fails to resolve, and silently ignores unrecognized model options.

A probe must therefore assert on a **positive signal** — the setting is present and correct in the provider's own resolved view — never on the absence of an error.

## Depth

Verification depth is recorded per provider because the available oracles are not equivalent, and calling them all "verified" would flatten a real difference. The chain is:

```
preset config → emitted bytes → provider parses file → provider's resolved config → outbound model request
```

agentspec's unit and integration tests cover the first hop only. Each entry below records how far along the chain its evidence actually reaches.

## Status

| Provider | Rendering | Status | Depth reached |
| --- | --- | --- | --- |
| OpenCode | `variant:` frontmatter key, sibling to `model:` | **Verified** | Provider's resolved config |
| Cursor | `[effort=…]` bracket option composed into the model id | **Verified** | Provider's resolved config |
| Claude | `effort:` frontmatter key, independent of `model:` | **Inferred from documentation** | — |

Probes verify the *provider's* contract, so they use hand-authored provider config files rather than `agentspec compile` output. Nothing here requires an agentspec feature to exist first, which is why a probe can precede the change it de-risks instead of gating it afterward.

---

## OpenCode

**Verified.** opencode 1.18.15; source `anomalyco/opencode` @ `cc4b456`. Probed 2026-08-12.

### Procedure

`opencode debug agent <name>` resolves an agent and prints its resolved record. It makes no model request, needs no network and no credentials, and is deterministic — the cheapest oracle of the three.

### Results

| Probe | Frontmatter | Resolved output |
| --- | --- | --- |
| Agent, sibling key | `model: anthropic/claude-sonnet-4-5` + `variant: high` | `"model": {"providerID":"anthropic","modelID":"claude-sonnet-4-5"}, "variant": "high"` ✅ |
| Agent, `#` suffix | `model: anthropic/claude-sonnet-4-5#high` | `"modelID": "claude-sonnet-4-5#high"`, `"variant": null` ❌ |
| Skill | `model:` + `variant:` | both absent from the resolved skill record ❌ |
| Command | `model:` + `variant:` | `"model": "anthropic/claude-sonnet-4-5", "variant": "high"` ✅ |
| Agent, `variant:` with no `model:` | `variant: high` | accepted silently, inert per OpenCode's schema annotation |

All five probes wrote zero bytes to stderr.

### What this establishes

- agentspec's top-level `variant:` emission for **agents** is correct.
- OpenCode's **skill** schema declares only `name`, `description`, `slash` (`packages/core/src/skill.ts:33-38`); `model` and `variant` are parsed as frontmatter and discarded.
- OpenCode's **command** schema accepts `variant`.
- `Provider.parseModel` splits on `/` only (`packages/opencode/src/provider/provider.ts:1997-2003`); no `#` parser exists anywhere in the repository, so a `#variant` suffix produces a well-formed model id that no catalog contains.

### Limits

`opencode debug agent` prints the *declared* variant from `Agent.Info`. The variant→`{}` collapse for a model lacking the `reasoning` capability happens later, at request-build time. This oracle proves OpenCode **read** the field, not that it will act on it.

---

## Cursor

**Verified.** Cursor 3.15.19 and 3.16.17. Probed 2026-08-14 and 2026-08-15.

### Procedure

Cursor exposes no offline introspection command. The oracle is its hook system: a hook that dumps raw stdin, plus hand-authored subagent definitions in a throwaway workspace.

`.cursor/hooks.json`:

```json
{
  "version": 1,
  "hooks": {
    "beforeSubmitPrompt": [{ "type": "command", "command": "/abs/path/dump-hook.sh" }],
    "subagentStart":      [{ "type": "command", "command": "/abs/path/dump-hook.sh" }],
    "subagentStop":       [{ "type": "command", "command": "/abs/path/dump-hook.sh" }],
    "preToolUse":         [{ "type": "command", "command": "/abs/path/dump-hook.sh" }]
  }
}
```

`dump-hook.sh` appends stdin to a log and echoes `{}` so it never blocks the session. Each arm is a `.cursor/agents/<name>.md` differing only in its `model:` line. Read the resolved model per arm with:

```sh
jq -r 'select(.hook_event_name=="subagentStart") | "\(.subagent_type)\t\(.subagent_model)"' payloads.jsonl
```

Do not run this in a workspace synced by `agentspec sync` — sync owns `.cursor/hooks.json` through `_agentspec_id` ownership tracking, and hand-editing it can collide with the merge logic in `src/adapters/cursor.rs`.

### Two model-bearing surfaces, at different fidelities

| Surface | Field | Fidelity |
| --- | --- | --- |
| `beforeSubmitPrompt` (parent turns only) | `model_id` + `model_params` | **Structured** — `[{"id":"effort","value":"high"}]` |
| `subagentStart` | `subagent_model` | **Flattened** — model id fused with resolved options |

`model` on a parent turn is the flattened form of `model_id` + `model_params`: `composer-2.5` + `fast=true` renders as `composer-2.5-fast`; `grok-4.6` + `effort=high,fast=true` renders as `cursor-grok-4.6-high-fast`.

**`model_params` is not available for subagents.** Subagent events expose only the flattened `subagent_model`.

### Results

Baseline for `claude-opus-5` with no bracket options: `claude-opus-5-thinking-high`.

| Declared `model:` | Resolved `subagent_model` | Reading |
| --- | --- | --- |
| `claude-opus-5[effort=low]` | `claude-opus-5-thinking-low` | **bracket options are parsed and applied** |
| `claude-opus-5[thinking=low]` | `claude-opus-5-thinking-high` | `thinking` is not an input key — `effort` is |
| `claude-opus-5[effort=nonsense]` | `claude-opus-5-thinking-high` | invalid *value* degrades silently |
| `claude-opus-5[bogus=nonsense]` | `claude-opus-5-thinking-high` | invalid *key* is not rejected either |
| `claude-opus-5[effort=high]` | `claude-opus-5-thinking-high` | **uninformative** — collides with the default |
| `composer-2.5[fast=false]` | `composer-2.5` | **uninformative** — `fast=false` is the subagent default |

Independent corroboration of the key name, from a parent turn on a different model family:

```json
{
  "hook_event_name": "beforeSubmitPrompt",
  "model_id": "grok-4.6",
  "model": "cursor-grok-4.6-high-fast",
  "model_params": [{"id": "effort", "value": "high"}, {"id": "fast", "value": "true"}]
}
```

That payload also demonstrates that multiple options compose, which is the shape a general model-options map would render to.

### What this establishes

- Cursor parses bracket model options in subagent frontmatter and applies them to the resolved model.
- The wire key is **`effort`**. `thinking` appears only in the flattened display string and is not accepted as an input key.
- Neither an unrecognized option key nor an unrecognized option value is rejected; both resolve silently to the model's default.

### Limits

- **The flattened string hides default-valued options.** An option whose value equals the model's default is indistinguishable from no option at all. This invalidated two probe arms and is the single most important constraint when re-running: **use non-default values.** Cursor's own documented example, `claude-opus-5[effort=high]`, is a default collision on that model.
- **`status: "completed"` is not evidence of execution.** Every probe subagent reported `completed` with `tool_call_count: 0` and `message_count: 0`. Resolution is confirmed; an outbound request carrying the resolved option is not.
- **Cursor's provider request is unobservable in principle.** All traffic, BYOK included, routes through Cursor's own backend, so the final hop of the chain is unreachable by any oracle Cursor exposes.
- Two Cursor versions were involved and behavior was consistent across both, but neither is pinned by anything, and Cursor's SDK documentation states that legal option values vary by model and account.

---

## Claude

**Not verified.** The `effort:` frontmatter field is asserted from vendor documentation only.

### Proposed procedure

A recording proxy via `ANTHROPIC_BASE_URL`, asserting on `output_config.effort` in the outbound request body. This is the most expensive oracle of the three to stand up, and the only one that reaches the end of the chain.

Two probes are needed, not one: `effort` on **agents** and `effort` on **skills** are separate frontmatter surfaces, both currently asserted from documentation alone. OpenCode is the standing example of those two surfaces diverging — there, the skill surface turned out not to exist at all.

Claude's rendering is an independent typed field with a documented closed enum and no string composition, so a probe here can only confirm; no plausible outcome changes how it should be rendered.

---

## Scope

This document is a written record, not a test harness. Automating these probes is tracked as TODO #11. Of the three, only OpenCode's is a plausible automation candidate — a shell command with no network, credentials, or live session. Cursor's is semi-automatable (the hook capture and log analysis are reusable; invoking the subagents still requires a live session), and Claude's requires a live session plus a recording proxy.

When a captured result goes stale, the thing that aged is the third-party tool, not any file in this repository — so nothing here can be checked by CI. Re-verification is triggered by judgment: a provider version bump, a changed rendering, or a new provider surface.
