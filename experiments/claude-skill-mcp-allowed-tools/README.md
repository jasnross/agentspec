# claude-skill-mcp-allowed-tools

**Question:** Under `--permission-prompts none`, does a Claude skill whose `allowed-tools` names an MCP tool exactly, by server, by server wildcard, or by plugin-server wildcard let the skill's call to that tool run, where `allowed-tools: Read` gets the same call refused for permission, for a user-configured and a plugin-bundled server alike?

**Driver:** `billed`. A command answers the question with no human step, but every arm spends a billed model call, so `just probe-run` withholds this package by name and reason; it runs only under `just probe-run --billed`.

**Depth:** `outbound-request`. The oracle is the tool result Claude Code sends back to the model in the request after the call, captured through OpenTelemetry — the far end of the chain, not the provider's resolved view.

## Why it matters

This is Key Assumption 2 of `$THOUGHTS_DIR/designs/2026-10-03-agentspec-mcp-tool-grants.md`. A skill's `allowed-tools` does not restrict what the model is offered; it pre-approves tools for the turn that invokes the skill, so they run without a permission prompt. The design has the Claude adapter write a spec's MCP grants into a skill's `allowed-tools` in the same spellings it writes into an agent's `tools`: `mcp__<server>__<tool>`, `mcp__<server>`, `mcp__<server>__*`, and the `mcp__plugin_<plugin>_<server>__*` form for a plugin-bundled server.

- **If every spelling's call runs and `Read`'s is denied**, the adapter can reuse the agent spellings for skills.
- **If a spelling's call is denied**, that spelling does not pre-approve on the skill surface, and the adapter needs a different one there or must report the gap.

## The six arms

Every arm runs `claude -p "/probe-skill"` with `--append-system-prompt AGENTSPEC-PROBE-MARKER-CCSKILL7 --permission-prompts none --allowedTools ToolSearch --effort low`. Each arm's project holds one skill, `probe-skill`, whose body is `Call the <tool> tool exactly once with no arguments. Then reply with exactly one word: done.`

| Arm | Server source | `allowed-tools` | `<tool>` |
| --- | --- | --- | --- |
| `none` | `--mcp-config` | `Read` | `mcp__fx__alpha` |
| `exact` | `--mcp-config` | `mcp__fx__alpha` | `mcp__fx__alpha` |
| `server` | `--mcp-config` | `mcp__fx` | `mcp__fx__alpha` |
| `server_glob` | `--mcp-config` | `mcp__fx__*` | `mcp__fx__alpha` |
| `plugin_none` | `--plugin-dir` | `Read` | `mcp__plugin_fxp_fx__alpha` |
| `plugin_glob` | `--plugin-dir` | `mcp__plugin_fxp_fx__*` | `mcp__plugin_fxp_fx__alpha` |

The four user-server arms connect `fx`, a local MCP stdio server (`fixtures/fx_server.py`), through a per-arm `--mcp-config` with `--strict-mcp-config`. `plugin_none` and `plugin_glob` connect the same server through the plugin `fxp` (`fixtures/plugin/`), so Claude names its tool `mcp__plugin_fxp_fx__alpha`. Each server source has its own `Read` arm, so every `"called"` is compared against a refusal from the same setup. A call to either answers `AGENTSPEC-FX-REPLY-CCSKILL7`.

- **`--permission-prompts none`** denies any call that would prompt, so a call the skill did not pre-approve fails rather than waiting for an answer.
- **`--allowedTools ToolSearch`** keeps a deferred `fx` tool loadable without pre-approving any `fx` tool. In the dry run every arm's model called `ToolSearch` beside its `fx` call.
- **`--append-system-prompt`** puts the marker in `.system`. A skill's body reaches `messages[]`, never `.system` (measured at 2.1.232; see `probe_claude_gate_marker`), so without it the marker and model gates would have nothing to match.

The skill is user-invoked: `/probe-skill` in the `-p` prompt. A model-invoked skill, run through the `Skill` tool, is a separate surface this package does not measure.

## The oracle

`CLAUDE_CODE_ENABLE_TELEMETRY=1` plus `OTEL_LOG_RAW_API_BODIES=file:<dir>` writes each request body to `<dir>/<uuid>.request.json` and each response to `<dir>/<request_id>.response.json`.

On 2.1.287 a request body carries the latest user turn but no earlier assistant turn, so the model's call appears only in a response and the tool result only in the request that follows. The runner therefore appends each arm's response bodies to its array in the view, after the gates have read the request-only view. A response has no `.system`, so it is never governed.

For each arm the projection collects the ids of `tool_use` blocks naming the arm's `<tool>`, then the `tool_result` blocks answering those ids, and classifies:

- any result's text contains `AGENTSPEC-FX-REPLY-CCSKILL7` → `"called"`
- every result has `is_error: true` and its text contains "Permission for this tool use was denied" → `"denied"`
- otherwise `{unrecognized: [texts]}`

Requiring the permission text keeps any other error — a call made before `ToolSearch` loaded the tool, an MCP server error, an input-validation error — out of `"denied"`: it lands in `{unrecognized: …}` and refutes. The text is Claude Code 2.1.287's wording; a release that rewords it refutes the same way, loudly.

It reads every request in the arm, not only governed ones; a title sidecar carries no `tool_result`, so it cannot contribute. Calls are found the way gate 9 finds them, from any entry's `.content[]`, and gate 9 guarantees at least one answered call, so no "no result" class is needed.

## Isolation

- **`--setting-sources project`** — excludes the operator's user-tier settings, permission rules included, so nothing outside the skill pre-approves an `fx` tool.
- **`--strict-mcp-config` with a per-arm `--mcp-config`** in the user-server arms — excludes every MCP server but `fx`.
- **`ENABLE_CLAUDEAI_MCP_SERVERS=false`** — on 2.1.287 `--strict-mcp-config` drops plugin-bundled servers too, so the plugin arms cannot use it; this variable keeps the operator's account-level claude.ai connectors out instead. A user-scope server in `~/.claude.json` still loads in those arms; its tools cannot carry the `mcp__plugin_fxp_fx__` prefix their calls name.
- **One fixture tree per arm**, holding only that arm's skill.
- **Unset variables**, matching the sibling runners: `ENABLE_TOOL_SEARCH`, `ANTHROPIC_BASE_URL`, `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`, `CLAUDE_CODE_MCP_STARTUP_WAIT_MS`, `CLAUDE_CODE_SUBAGENT_MODEL`, and `CLAUDE_CODE_SUBAGENT_MODEL_FORCE`.
- **`--max-budget-usd 0.50`** — a cap hit ends the turn before the call is answered, which surfaces as a gate-9 failure rather than a wrong answer.

## Model choice

Claude Sonnet 5.5 (`claude-sonnet-5-5`) at `--effort low`. This package does not delegate, so Haiku 4.5's failure to delegate in `claude-subagent-mcp-tools` does not apply to it; it uses Sonnet so that every Claude record of the MCP tool grants design describes one model.

## The gates

A gate failure is a statement about the run, not about Claude, and writes no record.

- **Stamp, per arm (runner-local).** `fx_server.py` writes `<arm>/fx.stamp` when it answers `tools/list`, so the server connected. That alone does not show the tool was loaded when the model called it; the permission text the `"denied"` class requires is what separates a refusal from an unknown tool.
- **Marker** (`probe_claude_gate_marker`). Every arm holds a request whose `.system` carries the appended marker.
- **Model** (`probe_claude_gate_model`). Every governed request ran on the pinned model. A title sidecar carrying the appended prompt on another model would fail this gate; on 2.1.287 none did, so the gate takes no exclusion for one.
- **Answered call, per arm** (`probe_claude_gate_tool_answered`). The arm's responses hold a `tool_use` naming the arm's `<tool>`, and its requests hold a `tool_result` answering it. An arm whose model never made the call, or whose turn ended before it was answered, reads as neither approved nor denied. Since the tool's name appears only in the skill body, this is also what shows the skill engaged; the marker gate shows only that `--append-system-prompt` reached `.system`.

## The assertion is relational across arms

`none` and `plugin_none` expect `"denied"`, every other arm `"called"`. Within each server source the skills differ only in `allowed-tools`, so each `Read` arm's refusal shows `--permission-prompts none` refused an unapproved call to that source's tool, and each `"called"` is non-default against the refusal from its own source. No separate discriminator fixture is wired (`TODO.md` #24).

## Discriminate evidence

Before the first billed run of the five-arm manifest, `record.sh --dry-run` against two fabricated views printed `confirmed` for one matching `expected` and `refuted` for one where `server_glob`'s call was denied:

```
record: dry run — confirmed — observed {"none":"denied","exact":"called","server":"called","server_glob":"called","plugin_glob":"called"} (no file written)
record: dry run — refuted — observed {"none":"denied","exact":"called","server":"called","server_glob":"denied","plugin_glob":"called"} (no file written)
```

Against the current six-arm manifest it printed `confirmed` for a matching view, and `refuted` for one where `none`'s error was a non-permission error and one where `plugin_none`'s call ran:

```
record: dry run — confirmed — observed {"none":"denied","exact":"called","server":"called","server_glob":"called","plugin_none":"denied","plugin_glob":"called"} (no file written)
record: dry run — refuted — observed {"none":{"unrecognized":["Error: No such tool available: mcp__fx__alpha"]},"exact":"called","server":"called","server_glob":"called","plugin_none":"denied","plugin_glob":"called"} (no file written)
record: dry run — refuted — observed {"none":"denied","exact":"called","server":"called","server_glob":"called","plugin_none":"called","plugin_glob":"called"} (no file written)
```

From the five-arm `PROBE_DRY_RUN=1` run on 2026-10-03 against Claude Code 2.1.287, every arm's model called `ToolSearch` and its `<tool>`. The `fx` call's result:

| Arm | Tool result, excerpted |
| --- | --- |
| `none` | `is_error: true`, "Permission for this tool use was denied. It requires approval, and this session has no approval surface …" |
| `exact`, `server`, `server_glob`, `plugin_glob` | `AGENTSPEC-FX-REPLY-CCSKILL7` |

Two records exist from 2026-10-03, both `confirmed`. The first (`T180814`) has five arms and a `"denied"` class that accepted any error; the second (`T181756`) has `plugin_none` and the permission-text requirement.

## What the record licenses

At `outbound-request`, this is evidence about what reached the model after the call.

- **Each measured MCP spelling pre-approves on the skill surface.** `mcp__fx__alpha`, `mcp__fx`, `mcp__fx__*`, and `mcp__plugin_fxp_fx__*` each let the skill's call run with no approval surface. The Claude adapter can write a skill's MCP grants in the same spellings it writes an agent's. A named plugin tool, `mcp__plugin_fxp_fx__alpha` in `allowed-tools`, is not among them and is unmeasured.
- **An `allowed-tools` without the MCP tool does not pre-approve it, for either server source.** `none`'s and `plugin_none`'s calls were refused for permission.

## Oracle limits

- **One model and one CLI version** per record.
- **User-invoked skills only.** A skill the model invokes through the `Skill` tool is unmeasured.
- **`-p` with `--permission-prompts none`.** In an interactive session a call the skill did not pre-approve would prompt rather than fail; what pre-approval changes there is the prompt, not the outcome measured here.
- **Default tool search only.** Claude with tool search off is unmeasured.
- **One tool per server.** Each arm calls `alpha`; that `mcp__fx` and `mcp__fx__*` also pre-approve `beta` follows from them naming the server, but is not measured.

## Related

- `experiments/claude-subagent-mcp-tools/` — the same spellings in a subagent's `tools`, which restricts what is offered rather than pre-approving.
- `experiments/claude-subagent-plugin-mcp-tools/` — the plugin spelling in a subagent's `tools`.
