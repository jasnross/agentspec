# claude-background-subagent-mcp-tools

**Question:** Is a Claude subagent whose `tools` is `Read` offered a connected MCP server's tools when it runs in the background, where the same agent run in the foreground is offered none and a background subagent with no `tools` field is offered all of them?

**Driver:** `billed`. A command answers the question with no human step, but every arm spends a billed model call, so `just probe-run` withholds this package by name and reason; it runs only under `just probe-run --billed`.

**Depth:** `outbound-request`. The oracle is the request body Claude Code sends, captured through OpenTelemetry — the far end of the chain, not the provider's resolved view.

## Why it matters

This is Key Assumption 7 of `$THOUGHTS_DIR/designs/2026-10-03-agentspec-mcp-tool-grants.md`. Claude Code's sub-agents documentation frames a subagent's tools as inherited tools "narrowed by two filters: the first removes a short list of tools from every subagent, and the second reduces the built-in tool set for subagents that run in the background", and of the second says:

> The second filter applies to subagents running in the background. Apart from `Agent` and `ExitPlanMode`, which follow the first filter's conditions wherever the subagent runs, a background subagent keeps every MCP tool but only these built-in tools: `Read`, `Grep`, `Glob`, …

The design read "keeps every MCP tool" as a leak: an agent agentspec restricts to `Read` would, run in the background, be offered every connected server's tools, and the Claude adapter would have to report that limitation. `expected` originally encoded that reading.

- **If `background` is offered `fx`'s tools**, a `tools` allowlist does not restrict MCP tools for a background subagent, and the adapter must report it.
- **If `background` is offered none**, the allowlist applies in both modes, and "keeps every MCP tool" describes the background-mode filter on built-ins, applied after the allowlist rather than instead of it.

## The three arms

Every arm runs `claude -p` with `--mcp-config <arm>/mcp.json --strict-mcp-config --allowedTools Agent --effort low`, connecting `fx`, a local MCP stdio server (`fixtures/fx_server.py`) with two tools, `alpha` and `beta`.

| Arm | Agent | `tools` | Prompt asks for `run_in_background` |
| --- | --- | --- | --- |
| `foreground` | `probe-bg-foreground` | `Read` | `false` |
| `background` | `probe-bg-background` | `Read` | `true` |
| `background_inherit` | `probe-bg-inherit` | none | `true` |

Each arm's prompt is `Use the Agent tool with run_in_background set to <mode> to delegate to the <agent> subagent. Do not answer yourself.` It omits the marker: a marked main-thread request would otherwise be read as governed.

**Every prompt names the mode.** On 2.1.287 in `-p`, an `Agent` call with no `run_in_background` field launched asynchronously: its tool result read "Async agent launched successfully", the same as `true`. The documentation agrees — "Where fork mode is off, Claude runs the subagent in the background by default", and fork mode is off by default under `-p`. So a foreground arm must ask for `false` explicitly. A one-off check with `false` ran synchronously: the subagent's report came back as the tool result.

`background_inherit` is what makes the `background` cell readable. Without it, an empty `background` could equally mean "the allowlist held" or "a background subagent is never offered MCP tools".

## The oracle

`CLAUDE_CODE_ENABLE_TELEMETRY=1` plus `OTEL_LOG_RAW_API_BODIES=file:<dir>` writes each untruncated request body to `<dir>/<uuid>.request.json`. The projection reads only requests whose `.system` carries the agent body's marker, `AGENTSPEC-PROBE-MARKER-CCBG6` — the subagent's own requests.

Each arm's value is the sorted union of tool names in `tools[]` and names listed as deferred, on lines of a text block opening "The following deferred tools are now available via ToolSearch", kept to names starting `mcp__fx__`. The union makes the value the same whether or not tool search defers a tool. An arm with no governed request projects to `"arm-had-no-governed-request"`.

In `-p`, a session that starts a background subagent stays open until it completes, so the background arms' subagent requests reach the sink.

## Isolation

- **`--setting-sources project`** — excludes the operator's user-tier settings.
- **`--strict-mcp-config` with a per-arm `--mcp-config`** — excludes every MCP server but `fx`. With `-p`, `--mcp-config` also makes Claude Code wait for `fx` to connect before the first turn.
- **One fixture tree per arm**, holding only that arm's agent.
- **Unset variables.** The runner unsets `ENABLE_TOOL_SEARCH`, `ANTHROPIC_BASE_URL`, and `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS` (each changes whether tool search is on), `CLAUDE_CODE_MCP_STARTUP_WAIT_MS` (whether `fx` has connected when the subagent spawns), `CLAUDE_CODE_SUBAGENT_MODEL` and `CLAUDE_CODE_SUBAGENT_MODEL_FORCE` (which model the subagent runs), and `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS` (how long `-p` waits for a background subagent).
- **The pinned model**, plus `model: inherit` in each agent, so the subagent runs on the session's model.
- **`--max-budget-usd 0.50`** — counts subagent spend. A cap hit stops subagent spawns, which surfaces as a marker-gate failure rather than a wrong answer.

The asynchronous default above was also observed with the parent Claude Code session's own variables (`CLAUDECODE`, `CLAUDE_CODE_CHILD_SESSION`, and their siblings) cleared, so it is not an artifact of running the probe from inside a Claude Code session.

## Model choice

Claude Sonnet 5.5 (`claude-sonnet-5-5`) at `--effort low`, matching `claude-subagent-mcp-tools`. Every arm here delegates, and Claude Haiku 4.5 did not delegate on 2.1.287 in that package (its README, Model choice), so the rule's fallback applies from the start.

## The gates

A gate failure is a statement about the run, not about Claude, and writes no record.

- **Stamp, per arm (runner-local).** `fx_server.py` writes `<arm>/fx.stamp` when it answers `tools/list`.
- **Marker** (`probe_claude_gate_marker`). Every arm holds a request whose `.system` carries the marker, so the arm's subagent actually ran.
- **Model** (`probe_claude_gate_model`). Every governed request ran on the pinned model.
- **MCP connected before delegation** (`probe_claude_gate_mcp_connected`). In every arm, every main-thread request that offers `Agent` lists `mcp__fx__alpha` and `mcp__fx__beta`, so `fx`'s tools existed before the subagent did.
- **Delegation mode** (`probe_claude_gate_delegation_background`), once per arm. It judges every `Agent` call in the arm's sink, a subagent's own included; none of these agents delegates. Every `Agent` call in the arm's `*.response.json` files set `run_in_background` to exactly the arm's boolean — a missing field fails — and every such call was answered by a `tool_result` that opens "Async agent launched" exactly when the arm is a background one. It reads the arm's sink rather than the view, because on 2.1.287 a request body carries no earlier assistant turn: the `Agent` call appears only in the responses.

`probe_claude_gate_deferred_mcp` is not used: no arm pins the deferred listing's format. `background_inherit`'s governed request listed both `fx` tools as deferred beside `ToolSearch` in the dry run, which is the format the projection reads.

## The assertion is relational across arms

`foreground` and `background` expect `[]`; `background_inherit` expects both `fx` tools. The two empty cells are readable because `background_inherit`, delegated the same way as `background`, was offered the tools. No separate discriminator fixture is wired (`TODO.md` #24).

## Discriminate evidence

Before the first billed run of the three-arm manifest, `record.sh --dry-run` against fabricated views printed `confirmed` for one matching the then-current `expected` and `refuted` for one where `background` was empty and one where `background_inherit` was empty:

```
record: dry run — confirmed — observed {"foreground":[],"background":["mcp__fx__alpha","mcp__fx__beta"],"background_inherit":["mcp__fx__alpha","mcp__fx__beta"]} (no file written)
record: dry run — refuted — observed {"foreground":[],"background":[],"background_inherit":["mcp__fx__alpha","mcp__fx__beta"]} (no file written)
record: dry run — refuted — observed {"foreground":[],"background":["mcp__fx__alpha","mcp__fx__beta"],"background_inherit":[]} (no file written)
```

From the `PROBE_DRY_RUN=1` run on 2026-10-03 against Claude Code 2.1.287, each arm's one governed request ran on `claude-sonnet-5-5`:

| Arm | MCP tools in `tools[]` | Listed as deferred |
| --- | --- | --- |
| `foreground` | none | none |
| `background` | none | none |
| `background_inherit` | `ToolSearch` | `mcp__fx__alpha`, `mcp__fx__beta` |

## What the record licenses

At `outbound-request`, this is evidence about what reached the model.

- **A `tools` allowlist restricts MCP tools for a background subagent.** `background`, with `tools: Read`, was offered no `fx` tool, the same as `foreground`. The Claude adapter needs no background-mode limitation for MCP grants.
- **A background subagent can be offered MCP tools.** `background_inherit`, with no `tools` field, was offered both, so the empty `background` cell is the allowlist at work.

That `background_inherit` received them deferred, beside `ToolSearch`, comes from the dry run's table, not from the record.

## History

**2026-10-03, Claude Code 2.1.287: the belief that a background subagent keeps every MCP tool despite its allowlist was refuted, and `expected` now holds the measured value.**

- **Old belief.** From the documentation sentence quoted above: `background` expected both `fx` tools.
- **Measured** (`results/2026-10-03T174258-claude-2.1.287__Claude_Code_.json`, `refuted`). `background` was offered none; `foreground` none; `background_inherit` both.
- **Acted on.** No adapter or capability accessor encoded the old belief. `expected.background` is `[]`, and `results/2026-10-03T174315-claude-2.1.287__Claude_Code_.json` confirmed it.
- **Not determined.** Whether an interactive session with fork mode on behaves the same. Read with its framing sentence, the documentation says the background filter reduces built-ins and leaves MCP tools alone, which is consistent with the allowlist applying first; the design had read the sentence on its own.

## Oracle limits

- **One model and one CLI version** per record.
- **`-p` with fork mode off only.** In an interactive session fork mode is on by default, and the documentation says Claude Code then runs every subagent in the background; that configuration is not measured.
- **Default tool search only.** Claude with tool search off is unmeasured.
- **The delegation mode is asked for, not forced.** A model that ignored the prompt fails gate 8 rather than recording. The documented `background: true` agent frontmatter field would force the background, but nothing forces the foreground, and putting it on only the background agents would make the arms differ in frontmatter as well as in mode.

## Related

- `experiments/claude-subagent-mcp-tools/` — the grant spellings, measured for delegated subagents whose mode no prompt specified. Given the asynchronous default above, those subagents most likely ran in the background too; that package does not record which way they ran.
