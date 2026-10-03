# claude-fork-subagent-mcp-tools

**Question:** Under fork mode (`CLAUDE_CODE_FORK_SUBAGENT=1`, `-p`), is a Claude subagent whose `tools` is `Read` offered a connected MCP server's tools, where a subagent with no `tools` field is offered all of them?

**Driver:** `billed`. A command answers the question with no human step, but every arm spends a billed model call, so `just probe-run` withholds this package by name and reason; it runs only under `just probe-run --billed`.

**Depth:** `outbound-request`. The oracle is the request body Claude Code sends, captured through OpenTelemetry — the far end of the chain, not the provider's resolved view.

## Why it matters

This is the part of Key Assumption 7 of `$THOUGHTS_DIR/designs/2026-10-03-agentspec-mcp-tool-grants.md` that `experiments/claude-background-subagent-mcp-tools/` left open. That package measured a background subagent under `-p` with fork mode off, and found its `tools` allowlist restricts MCP tools. Fork mode is on by default in an interactive session, and Claude Code's sub-agents documentation (code.claude.com/docs/en/sub-agents) says it changes how every subagent runs:

> Claude Code turns fork mode on by default in interactive sessions and leaves it off by default in non-interactive mode with `-p` and in the Agent SDK.

> `1` turns fork mode on in non-interactive mode and the Agent SDK as well

> Claude Code runs the subagents Claude spawns in the background, forks and non-fork subagents alike, apart from the cases that stay in the foreground. Claude Code also removes the Agent tool's `run_in_background` parameter, so Claude can't ask for the foreground.

> If you set `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS` to `1`, Claude Code runs the subagent in the foreground, in every kind of session and whether or not fork mode is on.

> Subagents spawned from a definition, such as Explore, work as usual.

A fork itself "receive[s] the main conversation's exact tool pool"; a subagent spawned from a definition, which is what agentspec emits, is documented to work as usual. `expected` encodes that reading: fork mode does not change what a restricted, definition-spawned subagent is offered.

- **If `fork_restricted` is offered no `fx` tool** (fork-clean), the allowlist holds under fork mode too, and the Claude adapter needs no fork-mode limitation.
- **If `fork_restricted` is offered `fx`'s tools** (fork-leaks), `capabilities.tools` does not restrict MCP tools for a Claude subagent under fork mode, and the adapter must report it.

## The two arms

Every arm runs `claude -p` with `CLAUDE_CODE_FORK_SUBAGENT=1` and `--mcp-config <arm>/mcp.json --strict-mcp-config --allowedTools Agent --effort low`, connecting `fx`, a local MCP stdio server (`fixtures/fx_server.py`) with two tools, `alpha` and `beta`.

| Arm               | Agent                   | `tools` |
| ----------------- | ----------------------- | ------- |
| `fork_restricted` | `probe-fork-restricted` | `Read`  |
| `fork_inherit`    | `probe-fork-inherit`    | none    |

Each arm's prompt is `Use the Agent tool to delegate to the <agent> subagent. Do not answer yourself.` It names no delegation mode, because fork mode removes `run_in_background`, and omits the marker, because a marked main-thread request would otherwise be read as governed.

`fork_inherit` is what makes the `fork_restricted` cell readable. Without it, an empty `fork_restricted` could equally mean "the allowlist held" or "a subagent under fork mode is never offered MCP tools".

## The oracle

`CLAUDE_CODE_ENABLE_TELEMETRY=1` plus `OTEL_LOG_RAW_API_BODIES=file:<dir>` writes each untruncated request body to `<dir>/<uuid>.request.json`. The projection is the sibling package's `governed`/`offered` projection, reading only requests whose `.system` carries the agent body's marker, `AGENTSPEC-PROBE-MARKER-CCFORK8` — the subagent's own requests.

Each arm's value is the sorted union of tool names in `tools[]` and names listed as deferred, on lines of a text block opening "The following deferred tools are now available via ToolSearch", kept to names starting `mcp__fx__`. An arm with no governed request projects to `"arm-had-no-governed-request"`.

In `-p`, a session that starts a background subagent stays open until it completes, so the subagent's requests reach the sink.

## Isolation

- **`--setting-sources project`** — excludes the operator's user-tier settings.
- **`--strict-mcp-config` with a per-arm `--mcp-config`** — excludes every MCP server but `fx`. With `-p`, `--mcp-config` also makes Claude Code wait for `fx` to connect before the first turn.
- **One fixture tree per arm**, holding only that arm's agent.
- **Unset variables**, matching the sibling runner: `ENABLE_TOOL_SEARCH`, `ANTHROPIC_BASE_URL`, `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`, `CLAUDE_CODE_MCP_STARTUP_WAIT_MS`, `CLAUDE_CODE_SUBAGENT_MODEL`, `CLAUDE_CODE_SUBAGENT_MODEL_FORCE`, and `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS`. The runner also unsets `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS`, which would force the foreground under fork mode.
- **The pinned model**, plus `model: inherit` in each agent, so the subagent runs on the session's model.
- **`--max-budget-usd 0.50`** — counts subagent spend. A cap hit stops subagent spawns, which surfaces as a marker-gate failure rather than a wrong answer.

## Model choice

Claude Sonnet 5.5 (`claude-sonnet-5-5`) at `--effort low`, matching `claude-subagent-mcp-tools`. Every arm here delegates, and Claude Haiku 4.5 did not delegate on 2.1.287 in that package (its README, Model choice).

## The gates

A gate failure is a statement about the run, not about Claude, and writes no record.

- **Stamp, per arm (runner-local).** `fx_server.py` writes `<arm>/fx.stamp` when it answers `tools/list`.
- **Marker** (`probe_claude_gate_marker`). Every arm holds a request whose `.system` carries the marker, so the arm's subagent actually ran.
- **Model** (`probe_claude_gate_model`). Every governed request ran on the pinned model.
- **MCP connected before delegation** (`probe_claude_gate_mcp_connected`). In every arm, every main-thread request that offers `Agent` lists `mcp__fx__alpha` and `mcp__fx__beta`, so `fx`'s tools existed before the subagent did.
- **Fork mode on, and the delegation ran under it** (`probe_claude_gate_delegation_fork`), once per arm. Every request in the arm's sink that offers `Agent` declares no `run_in_background` parameter in that tool's input schema, the arm's responses hold at least one `Agent` call, and every such call was answered by a `tool_result` that opens "Async agent launched". The missing schema parameter is the positive sign fork mode was on, since fork mode removes it. A call that merely leaves the field out would not do: with fork mode off, such a call also launches in the background (`claude-background-subagent-mcp-tools`, The three arms). The sibling's `probe_claude_gate_delegation_background` requires `run_in_background` to equal a boolean, which fork mode makes impossible.

The schema check was shown to separate the two modes against real 2.1.287 captures. In this package's `PROBE_DRY_RUN=1` workspace, both arms' `Agent` schema declared `description`, `isolation`, `model`, `prompt`, and `subagent_type`, and the gate passed. In a kept `claude-background-subagent-mcp-tools` workspace (fork mode off), the schema also declared `run_in_background`, and the gate failed both arms it read with "declares run_in_background, so fork mode was off".

## The assertion is relational across arms

`fork_restricted` expects `[]`; `fork_inherit` expects both `fx` tools. The empty cell is readable because `fork_inherit`, delegated the same way, was offered the tools. No separate discriminator fixture is wired (`TODO.md` #24).

## Discriminate evidence

Before the first billed run, `record.sh --manifest experiments/claude-fork-subagent-mcp-tools/probe.json --view <view> --dry-run` against two fabricated views printed `confirmed` for one matching `expected` and `refuted` for one where `fork_restricted` was offered both `fx` tools:

```
record: dry run — confirmed — observed {"fork_restricted":[],"fork_inherit":["mcp__fx__alpha","mcp__fx__beta"]} (no file written)
record: dry run — refuted — observed {"fork_restricted":["mcp__fx__alpha","mcp__fx__beta"],"fork_inherit":["mcp__fx__alpha","mcp__fx__beta"]} (no file written)
```

Two records exist from 2026-10-03 against Claude Code 2.1.287, both `confirmed`. The first (`T215439`) was written under an earlier gate 10 that checked only the `Agent` call, which a fork-off run would also have passed, so it does not show fork mode was on. The second (`T221149`) was written under the schema check above, and is the record this README's conclusions rest on.

## What the record licenses

At `outbound-request`, this is evidence about what reached the model.

- **A `tools` allowlist restricts MCP tools for a definition-spawned subagent under fork mode.** `fork_restricted`, with `tools: Read`, was offered no `fx` tool. With `claude-background-subagent-mcp-tools`, this covers the background run in both fork-mode states, and the Claude adapter needs no fork-mode limitation for MCP grants.
- **A subagent under fork mode can be offered MCP tools.** `fork_inherit`, with no `tools` field, was offered both, so the empty `fork_restricted` cell is the allowlist at work.

## Oracle limits

- **One model and one CLI version** per record.
- **`-p` with `CLAUDE_CODE_FORK_SUBAGENT=1`.** An interactive session, where fork mode is on by default, is unmeasured; the documentation describes the same fork-mode behavior in both.
- **Definition-spawned subagents only.** A fork (the `fork` subagent type) inherits the main session's exact tool pool by design and is not what agentspec emits.
- **Default tool search only.** Claude with tool search off is unmeasured.

## Related

- `experiments/claude-background-subagent-mcp-tools/` — the same question with fork mode off, and the source of this package's fixtures and projection.
- `experiments/claude-subagent-mcp-tools/` — the grant spellings in a subagent's `tools`.
