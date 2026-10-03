# claude-skill-plugin-mcp-named-tool

**Question:** Under `--permission-prompts none`, does a Claude skill whose `allowed-tools` names one tool of a plugin-bundled MCP server, `mcp__plugin_<plugin>_<server>__<tool>`, let the skill's call to that tool run, where `allowed-tools: Read` gets the same call refused for permission?

**Driver:** `billed`. A command answers the question with no human step, but every arm spends a billed model call, so `just probe-run` withholds this package by name and reason; it runs only under `just probe-run --billed`.

**Depth:** `outbound-request`. The oracle is the tool result Claude Code sends back to the model in the request after the call, captured through OpenTelemetry — the far end of the chain, not the provider's resolved view.

## Why it matters

This is the part of Key Assumption 2 of `$THOUGHTS_DIR/designs/2026-10-03-agentspec-mcp-tool-grants.md` that `experiments/claude-skill-mcp-allowed-tools/` left unmeasured. That package showed a skill's `allowed-tools` pre-approves an MCP tool named exactly, by server, by server wildcard, and by plugin-server wildcard. The Claude adapter also writes a named grant of a plugin-bundled server, `mcp__plugin_<plugin>_<server>__<tool>`, into a skill's `allowed-tools`.

- **If `plugin_named`'s call runs and `plugin_none`'s is denied**, the adapter can write named plugin-server grants into skills as it does into agents.
- **If `plugin_named`'s call is denied**, the named plugin spelling does not pre-approve on the skill surface, and the adapter cannot carry such a grant on a skill.

## The two arms

Every arm runs `claude -p "/probe-skill"` with `--plugin-dir <arm>/fxp --append-system-prompt AGENTSPEC-PROBE-MARKER-CCSKPN9 --permission-prompts none --allowedTools ToolSearch --effort low`. Each arm's project holds one skill, `probe-skill`, whose body is `Call the mcp__plugin_fxp_fx__alpha tool exactly once with no arguments. Then reply with exactly one word: done.`

| Arm            | `allowed-tools`             |
| -------------- | --------------------------- |
| `plugin_none`  | `Read`                      |
| `plugin_named` | `mcp__plugin_fxp_fx__alpha` |

Both arms connect `fx`, a local MCP stdio server (`fixtures/fx_server.py`), through the plugin `fxp` (`fixtures/plugin/`), so Claude names its tool `mcp__plugin_fxp_fx__alpha`. A call answers `AGENTSPEC-FX-REPLY-CCSKPN9`, a reply marker distinct from the sibling package's.

- **`--permission-prompts none`** denies any call that would prompt, so a call the skill did not pre-approve fails rather than waiting for an answer.
- **`--allowedTools ToolSearch`** keeps a deferred `fx` tool loadable without pre-approving any `fx` tool.
- **`--append-system-prompt`** puts the marker in `.system`. A skill's body reaches `messages[]`, never `.system` (measured at 2.1.232; see `probe_claude_gate_marker`), so without it the marker and model gates would have nothing to match.

The skill is user-invoked: `/probe-skill` in the `-p` prompt. A model-invoked skill, run through the `Skill` tool, is a separate surface this package does not measure.

## The oracle

`CLAUDE_CODE_ENABLE_TELEMETRY=1` plus `OTEL_LOG_RAW_API_BODIES=file:<dir>` writes each request body to `<dir>/<uuid>.request.json` and each response to `<dir>/<request_id>.response.json`.

On 2.1.287 a request body carries the latest user turn but no earlier assistant turn, so the model's call appears only in a response and the tool result only in the request that follows. The runner therefore appends each arm's response bodies to its array in the view, after the gates have read the request-only view.

The projection is the sibling package's `classify`, matching this package's reply marker. For each arm it collects the ids of `tool_use` blocks naming `mcp__plugin_fxp_fx__alpha`, then the `tool_result` blocks answering those ids, and classifies:

- any result's text contains `AGENTSPEC-FX-REPLY-CCSKPN9` → `"called"`
- every result has `is_error: true` and its text contains "Permission for this tool use was denied" → `"denied"`
- otherwise `{unrecognized: [texts]}`

Requiring the permission text keeps any other error — a call made before `ToolSearch` loaded the tool, an MCP server error, an input-validation error — out of `"denied"`.

## Isolation

- **`--setting-sources project`** — excludes the operator's user-tier settings, permission rules included, so nothing outside the skill pre-approves an `fx` tool.
- **`ENABLE_CLAUDEAI_MCP_SERVERS=false`** — on 2.1.287 `--strict-mcp-config` drops plugin-bundled servers too, so the arms cannot use it; this variable keeps the operator's account-level claude.ai connectors out instead. A user-scope server in `~/.claude.json` still loads; its tools cannot carry the `mcp__plugin_fxp_fx__` prefix the calls name.
- **One fixture tree per arm**, holding only that arm's skill.
- **Unset variables**, matching the sibling runners: `ENABLE_TOOL_SEARCH`, `ANTHROPIC_BASE_URL`, `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`, `CLAUDE_CODE_MCP_STARTUP_WAIT_MS`, `CLAUDE_CODE_SUBAGENT_MODEL`, and `CLAUDE_CODE_SUBAGENT_MODEL_FORCE`.
- **`--max-budget-usd 0.50`** — a cap hit ends the turn before the call is answered, which surfaces as a gate-9 failure rather than a wrong answer.

## Model choice

Claude Sonnet 5.5 (`claude-sonnet-5-5`) at `--effort low`, so that every Claude record of the MCP tool grants design describes one model.

## The gates

A gate failure is a statement about the run, not about Claude, and writes no record.

- **Stamp, per arm (runner-local).** `fx_server.py` writes `<arm>/fx.stamp` when it answers `tools/list`, so the server connected.
- **Marker** (`probe_claude_gate_marker`). Every arm holds a request whose `.system` carries the appended marker.
- **Model** (`probe_claude_gate_model`). Every governed request ran on the pinned model.
- **Answered call, per arm** (`probe_claude_gate_tool_answered`). The arm's responses hold a `tool_use` naming `mcp__plugin_fxp_fx__alpha`, and its requests hold a `tool_result` answering it. Since the tool's name appears only in the skill body, this is also what shows the skill engaged.

## The assertion is relational across arms

`plugin_none` expects `"denied"` and `plugin_named` expects `"called"`. The skills differ only in `allowed-tools`, so `plugin_none`'s refusal shows `--permission-prompts none` refused an unapproved call to the plugin tool, and `plugin_named`'s `"called"` is non-default against it. No separate discriminator fixture is wired (`TODO.md` #24).

## Discriminate evidence

Before the first billed run, `record.sh --manifest experiments/claude-skill-plugin-mcp-named-tool/probe.json --view <view> --dry-run` against two fabricated views printed `confirmed` for one matching `expected` and `refuted` for one where `plugin_named`'s call was denied for permission:

```
record: dry run — confirmed — observed {"plugin_none":"denied","plugin_named":"called"} (no file written)
record: dry run — refuted — observed {"plugin_none":"denied","plugin_named":"denied"} (no file written)
```

The first record, from 2026-10-03 against Claude Code 2.1.287, is `confirmed`.

## What the record licenses

At `outbound-request`, this is evidence about what reached the model after the call.

- **A named plugin-server tool pre-approves on the skill surface.** `mcp__plugin_fxp_fx__alpha` in `allowed-tools` let the skill's call run with no approval surface, where `Read` got it refused. The Claude adapter can write a named grant of a plugin-bundled server into a skill's `allowed-tools` in the same spelling it writes into an agent's `tools`.

## Oracle limits

- **One model and one CLI version** per record.
- **User-invoked skills only.** A skill the model invokes through the `Skill` tool is unmeasured.
- **Plugin-bundled servers only.** A named tool of a non-plugin server in a skill's `allowed-tools` is measured by `experiments/claude-skill-mcp-allowed-tools/` (its `exact` arm), not here.
- **`-p` with `--permission-prompts none`.** In an interactive session a call the skill did not pre-approve would prompt rather than fail.
- **Default tool search only.** Claude with tool search off is unmeasured.

## Related

- `experiments/claude-skill-mcp-allowed-tools/` — the other MCP spellings on the same skill surface, and the source of this package's fixtures and projection.
- `experiments/claude-subagent-plugin-mcp-tools/` — the plugin spelling in a subagent's `tools`.
