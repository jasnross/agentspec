# claude-subagent-plugin-mcp-tools

**Question:** For each spelling of a grant for a plugin-bundled MCP server in a Claude subagent's `tools`, with the agent inside the plugin or in the project, which of that server's tools does the subagent's model request carry?

**Driver:** `billed`. A command answers the question with no human step, but every arm spends a billed model call, so `just probe-run` withholds this package by name and reason; it runs only under `just probe-run --billed`.

**Depth:** `outbound-request`. The oracle is the request body Claude Code sends, captured through OpenTelemetry — the far end of the chain, not the provider's resolved view.

## Why it matters

This is Key Assumption 1 of `$THOUGHTS_DIR/designs/2026-10-03-agentspec-mcp-tool-grants.md`. Claude Code names a plugin-bundled server's tools `mcp__plugin_<plugin>_<server>__<tool>`, where a user- or project-configured server's are `mcp__<server>__<tool>`. The design has the Claude adapter compose the plugin spelling when an `[mcp.<name>.claude]` block names the plugin that bundles the server, and the plain spelling otherwise. That is right only if a subagent's `tools` honors the plugin spelling — wherever the agent lives — and the plain spelling does not match a plugin server.

- **If the plugin spellings grant exactly what they name**, the adapter's composition is correct in all three sync modes: plugin mode puts the agent inside the plugin, and user and project mode put it beside a plugin-installed server.
- **If a plugin spelling grants nothing from one location**, agentspec cannot grant a plugin server's tools from that mode.
- **If `project_unprefixed` is offered `mcp__plugin_fxp_fx__alpha`**, Claude resolves the plain spelling against plugin servers too, and the design's `plugin` field is unnecessary.

`experiments/claude-subagent-mcp-tools/` measured user- and project-configured servers only.

## The six arms

Every arm runs `claude -p` with `--plugin-dir <arm>/fxp --allowedTools Agent --effort low`. The plugin `fxp` (`fixtures/plugin/`) bundles `fx`, a local MCP stdio server (`fixtures/fx_server.py`) with two tools, `alpha` and `beta`, so Claude names them `mcp__plugin_fxp_fx__alpha` and `mcp__plugin_fxp_fx__beta`. Every arm installs the plugin, so every arm connects `fx` the same way. Two tools are what lets "only the granted tool is visible" be told apart from "every tool is visible".

| Arm | Agent location | Delegation name | `tools` |
| --- | --- | --- | --- |
| `read` | project | `probe-plug-read` | `Read` |
| `plugin_exact` | plugin `agents/` | `fxp:probe-plug-exact` | `Read, mcp__plugin_fxp_fx__alpha` |
| `plugin_glob` | plugin `agents/` | `fxp:probe-plug-glob` | `Read, mcp__plugin_fxp_fx__*` |
| `project_exact` | project | `probe-plug-project-exact` | `Read, mcp__plugin_fxp_fx__alpha` |
| `project_glob` | project | `probe-plug-project-glob` | `Read, mcp__plugin_fxp_fx__*` |
| `project_unprefixed` | project | `probe-plug-unprefixed` | `Read, mcp__fx__alpha` |

A plugin agent is addressed as `<plugin>:<name>`, where `<name>` is its `name:` field. Plugin-arm agents live in `fixtures/plugin-agents/`, and the runner copies only that arm's agent into that arm's plugin copy; their project trees are empty (`fixtures/<arm>/.gitkeep`). Project-arm agents live in `fixtures/<arm>/.claude/agents/`.

Each arm's prompt is `Use the Agent tool to delegate to the <delegation name> subagent. Do not answer yourself.` It omits the marker: a marked main-thread request would otherwise be read as governed.

## The oracle

`CLAUDE_CODE_ENABLE_TELEMETRY=1` plus `OTEL_LOG_RAW_API_BODIES=file:<dir>` writes each untruncated request body to `<dir>/<uuid>.request.json`. The projection reads only requests whose `.system` carries the agent body's marker, `AGENTSPEC-PROBE-MARKER-CCPLUG4` — the subagent's own requests.

Each arm's value is the sorted union of tool names in `tools[]` and names listed as deferred, on lines of a text block opening "The following deferred tools are now available via ToolSearch", kept to names starting `mcp__plugin_fxp_fx__` or `mcp__fx__`. The union makes the value the same whether or not tool search defers a tool, as long as the deferred listing keeps one bare name per line; see The gates for what pins that here. An arm with no governed request projects to `"arm-had-no-governed-request"`.

## Isolation

- **`--setting-sources project`** — excludes the operator's user-tier settings. It keeps `--plugin-dir` agents loaded.
- **`ENABLE_CLAUDEAI_MCP_SERVERS=false` in place of `--strict-mcp-config`.** On 2.1.287 `--strict-mcp-config` drops plugin-bundled servers too: measured 2026-10-03 at zero cost, the `init` event listed no MCP server and `fx` was never spawned. Without the flag, the operator's account-level claude.ai connectors would load; this variable excludes them. User-scope servers in the operator's `~/.claude.json` still load. The projection's name filter keeps their tools out of every value unless one is named `fx`, in which case its `mcp__fx__alpha` would make `project_unprefixed` falsely non-empty. The foreign-server gate below refuses such a run.
- **One plugin copy and one fixture tree per arm**, each holding only that arm's agent, so a delegation to the wrong name finds no agent and produces no governed request.
- **Unset variables.** The runner unsets `ENABLE_TOOL_SEARCH`, `ANTHROPIC_BASE_URL`, and `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS` (each changes whether tool search is on), `CLAUDE_CODE_MCP_STARTUP_WAIT_MS` (whether `fx` has connected when the subagent spawns), and `CLAUDE_CODE_SUBAGENT_MODEL` and `CLAUDE_CODE_SUBAGENT_MODEL_FORCE` (which model the subagent runs).
- **The pinned model**, plus `model: inherit` in each agent, which outranks `CLAUDE_CODE_SUBAGENT_MODEL`, so the subagent runs on the session's model.
- **`--max-budget-usd 0.50`** — counts subagent spend. A cap hit stops subagent spawns, which surfaces as a marker-gate failure rather than a wrong answer.

## Model choice

Claude Sonnet 5.5 (`claude-sonnet-5-5`) at `--effort low`, matching `claude-subagent-mcp-tools`. Every arm here delegates, and Claude Haiku 4.5 did not delegate on 2.1.287 in that package (its README, Model choice), so the rule's fallback applies from the start.

## The gates

A gate failure is a statement about the run, not about Claude, and writes no record.

- **Stamp, per arm (runner-local).** `fx_server.py` writes `<arm>/fx.stamp` when it answers `tools/list`. The check only tests that a file exists, so it stays in the runner.
- **Marker** (`probe_claude_gate_marker`). Every arm holds a request whose `.system` carries the marker, so the arm's subagent actually ran.
- **Model** (`probe_claude_gate_model`). Every governed request ran on the pinned model.
- **No foreign `fx` server** (`probe_claude_gate_mcp_absent`). No main-thread request, in any arm, lists a tool starting `mcp__fx__`, directly or as deferred. Every arm's only configured server is the plugin's, so such a tool can only come from a server outside the fixture, and it would reach `project_unprefixed` through the projection's `mcp__fx__` filter as a false `refuted`.
- **MCP connected before delegation** (`probe_claude_gate_mcp_connected`). In every arm, every main-thread request that offers `Agent` lists `mcp__plugin_fxp_fx__alpha` and `mcp__plugin_fxp_fx__beta`, directly or as deferred. The delegating request is one of them, so the plugin server's tools existed under that name before the subagent did. Without it, an empty `read` or `project_unprefixed` would read the same as "the plugin server had not connected yet".

`probe_claude_gate_deferred_mcp` is not used: no arm here is unrestricted, so none has an unconditional deferred listing to pin. The cost is that nothing in this run pins the deferred listing's one-name-per-line format, which the projection reads through a prefix filter. No arm grants `ToolSearch`, and `claude-subagent-mcp-tools` measured that grants then arrive directly in `tools[]`, so the listing is not expected to carry a granted tool here; but a format change that indented or re-terminated its lines would go undetected, and would read as an absent grant.

If the connection gate fails, re-run the package once by hand before treating it as anything but an apparatus failure; the runner itself does not retry. A second failure the same way is worth investigating rather than recording.

## The assertion is relational across arms

The six agents differ only in `tools` and location, so distinct observed values across arms show the projection reads each arm independently. No separate discriminator fixture is wired (`TODO.md` #24).

`expected` states the design's belief: each plugin spelling grants what it names from either location, and the plain spelling grants nothing from a plugin server.

## Discriminate evidence

Before the first billed run, `record.sh --dry-run` against two fabricated views printed `confirmed` for one matching `expected` and `refuted` for one where `project_unprefixed` carried `mcp__plugin_fxp_fx__alpha`:

```
record: dry run — confirmed — observed {"read":[],"plugin_exact":["mcp__plugin_fxp_fx__alpha"],…,"project_unprefixed":[]} (no file written)
record: dry run — refuted — observed {"read":[],"plugin_exact":["mcp__plugin_fxp_fx__alpha"],…,"project_unprefixed":["mcp__plugin_fxp_fx__alpha"]} (no file written)
```

From the `PROBE_DRY_RUN=1` run on 2026-10-03 against Claude Code 2.1.287, each arm's one governed request ran on `claude-sonnet-5-5` and listed no deferred tool. Its `tools[]`:

| Arm | `tools[].name` |
| --- | --- |
| `read`, `project_unprefixed` | `Read`, `advisor` |
| `plugin_exact`, `project_exact` | `Read`, `mcp__plugin_fxp_fx__alpha`, `advisor` |
| `plugin_glob`, `project_glob` | `Read`, `mcp__plugin_fxp_fx__alpha`, `mcp__plugin_fxp_fx__beta`, `advisor` |

Two records exist from 2026-10-03, both `confirmed` with the same `observed`. The first (`T140459`) predates the foreign-server gate; the second (`T153213`) was made with every gate above in place.

## What the record licenses

At `outbound-request`, this is evidence about what reached the model.

- **The plugin spelling grants exactly what it names, from either location.** `mcp__plugin_fxp_fx__alpha` granted `alpha` alone, and `mcp__plugin_fxp_fx__*` granted both tools, whether the agent sat inside the plugin or in the project.
- **The plain spelling does not match a plugin server.** `project_unprefixed`, granting `mcp__fx__alpha`, received no `fx` tool. The design's `[mcp.<name>.claude] plugin` field is necessary: without it, the Claude adapter would compose a spelling that grants nothing.
- **An allowlist without MCP entries grants none.** `read` received no `fx` tool.

That every grant arrived directly in `tools[]`, never deferred, comes from the dry run's table, not from the record, whose value is the union of both places.

## Oracle limits

- **One model and one CLI version** per record.
- **Default tool search only.** Claude with tool search off is unmeasured.
- **A `--plugin-dir` plugin, not a marketplace install.** Both name tools by the plugin's `name`; that a marketplace-installed plugin resolves grants the same way is not measured.
- **Delegated subagents only.** A skill's `allowed-tools`, and an agent run as the session with `--agent`, are separate surfaces.

## Related

- `experiments/claude-subagent-mcp-tools/` — the same grants for user- and project-configured servers.
