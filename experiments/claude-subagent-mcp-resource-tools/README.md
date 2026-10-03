# claude-subagent-mcp-resource-tools

**Question:** When a connected MCP server offers resources, which of `ListMcpResourcesTool`, `ReadMcpResourceTool`, and `ReadMcpResourceDirTool` is a Claude subagent offered with no `tools` field, with `tools: Read`, with `Read` beside an exact or whole-server MCP grant, and with the three named explicitly?

**Driver:** `billed`. A command answers the question with no human step, but every arm spends a billed model call, so `just probe-run` withholds this package by name and reason; it runs only under `just probe-run --billed`.

**Depth:** `outbound-request`. The oracle is the request body Claude Code sends, captured through OpenTelemetry — the far end of the chain, not the provider's resolved view.

## Why it matters

This is Key Assumption 3 of `$THOUGHTS_DIR/designs/2026-10-03-agentspec-mcp-tool-grants.md`. Claude Code exposes a server's MCP resources through dedicated tools rather than through the server's own `mcp__<server>__*` names. The design assumed a subagent whose `tools` allowlist omits those tools is not offered them, so an MCP grant gives a subagent the server's tools and nothing more. A free `init`-event check on 2.1.287 found three such tools — `ListMcpResourcesTool`, `ReadMcpResourceTool`, and `ReadMcpResourceDirTool` — where the documentation and the design name none and two respectively, so the package counts all three.

- **If the restricted arms are offered none**, an MCP grant does not widen into resource access, and the Claude adapter needs nothing to withhold resources.
- **If a restricted arm is offered one**, the adapter cannot keep resources out of a subagent it grants MCP tools to.
- **What `inherit` and `explicit` receive** decides whether Claude can give a subagent resource access at all — unrestricted, or by naming the tools.

## The five arms

Every arm runs `claude -p` with `--mcp-config <arm>/mcp.json --strict-mcp-config --allowedTools Agent --effort low`, connecting `fx`, a local MCP stdio server (`fixtures/fx_server.py`) with two tools, `alpha` and `beta`, that also advertises the `resources` capability and serves one resource and one template. The five agents differ only in `tools`:

| Arm | Agent | `tools` |
| --- | --- | --- |
| `inherit` | `probe-res-inherit` | none |
| `read` | `probe-res-read` | `Read` |
| `exact` | `probe-res-exact` | `Read, mcp__fx__alpha` |
| `server_glob` | `probe-res-server-glob` | `Read, mcp__fx__*` |
| `explicit` | `probe-res-explicit` | `Read, ListMcpResourcesTool, ReadMcpResourceTool, ReadMcpResourceDirTool` |

Each arm's prompt is `Use the Agent tool to delegate to the <agent> subagent. Do not answer yourself.` It omits the marker: a marked main-thread request would otherwise be read as governed.

## The oracle

`CLAUDE_CODE_ENABLE_TELEMETRY=1` plus `OTEL_LOG_RAW_API_BODIES=file:<dir>` writes each untruncated request body to `<dir>/<uuid>.request.json`. The projection reads only requests whose `.system` carries the agent body's marker, `AGENTSPEC-PROBE-MARKER-CCRES5` — the subagent's own requests.

Each arm's list is the sorted union of tool names in `tools[]` and names listed as deferred, on lines of a text block opening "The following deferred tools are now available via ToolSearch", kept to the three resource tool names, across every governed request in the arm. A tool that appears in any of them counts as offered, even if an earlier request lacked it. The union makes the value the same whether or not tool search defers a tool.

`read`, `exact`, and `server_glob` project to that list. `inherit` and `explicit` project to `"none-or-all"` when the list is empty or exactly the three tools, and to the list itself otherwise; see the assertion section for why. An arm with no governed request projects to `"arm-had-no-governed-request"`.

## Isolation

- **`--setting-sources project`** — excludes the operator's user-tier settings.
- **`--strict-mcp-config` with a per-arm `--mcp-config`** — excludes every MCP server but `fx`. With `-p`, `--mcp-config` also makes Claude Code wait for `fx` to connect before the first turn.
- **One fixture tree per arm**, holding only that arm's agent, so a delegation to the wrong name finds no agent and produces no governed request.
- **Unset variables.** The runner unsets `ENABLE_TOOL_SEARCH`, `ANTHROPIC_BASE_URL`, and `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS` (each changes whether tool search is on), `CLAUDE_CODE_MCP_STARTUP_WAIT_MS` (whether `fx` has connected when the subagent spawns), and `CLAUDE_CODE_SUBAGENT_MODEL` and `CLAUDE_CODE_SUBAGENT_MODEL_FORCE` (which model the subagent runs).
- **The pinned model**, plus `model: inherit` in each agent, so the subagent runs on the session's model.
- **`--max-budget-usd 0.50`** — counts subagent spend. A cap hit stops subagent spawns, which surfaces as a marker-gate failure rather than a wrong answer.

## Model choice

Claude Sonnet 5.5 (`claude-sonnet-5-5`) at `--effort low`, matching `claude-subagent-mcp-tools`. Every arm here delegates, and Claude Haiku 4.5 did not delegate on 2.1.287 in that package (its README, Model choice), so the rule's fallback applies from the start.

## The gates

A gate failure is a statement about the run, not about Claude, and writes no record.

- **Stamp, per arm (runner-local).** `fx_server.py` writes `<arm>/fx.stamp` when it answers `tools/list`.
- **Marker** (`probe_claude_gate_marker`). Every arm holds a request whose `.system` carries the marker, so the arm's subagent actually ran.
- **Model** (`probe_claude_gate_model`). Every governed request ran on the pinned model.
- **Deferred listing on `inherit`** (`probe_claude_gate_deferred_mcp`). The `inherit` arm's governed requests list `mcp__fx__alpha` and `mcp__fx__beta` as deferred and carry `ToolSearch`, so the run is under default tool search. This also pins the deferred listing's one-name-per-line format the projection reads.
- **MCP connected before delegation, twice** (`probe_claude_gate_mcp_connected`). In every arm, every main-thread request that offers `Agent` lists `mcp__fx__alpha` and `mcp__fx__beta`, and — in a second call — `ListMcpResourcesTool`, `ReadMcpResourceTool`, and `ReadMcpResourceDirTool`, directly or as deferred. The second call reads the same places with the same exact names as the projection, so an empty value cannot mean "not in the session", "renamed", or "the filter is misspelled". If it fails, the runner adds that a rename or a Claude Code that stopped offering the tools is the likelier cause than a late connection.

After a successful recording the runner removes its workspace, except when `inherit` or `explicit` was offered a resource tool: then it keeps the workspace and prints its path, because such a run is the rare one, and its requests are the only evidence of why it differed.

## The assertion

`read`, `exact`, and `server_glob` expect `[]`. `inherit` and `explicit` expect `"none-or-all"`.

**Why two cells accept either outcome.** Those cells were offered nothing in most runs and all three tools in one (see the run table). Pinning them to either value would make a re-run record `refuted` about one time in eight with nothing in Claude changed, which reads as assertion drift — the contract's strong signal — when it is not. `"none-or-all"` keeps both observed outcomes `confirmed` while a partial set, a renamed tool, or a fourth tool still refutes. The manifest's belief for those two cells is therefore "unstable between none and all three", not a value.

**What the restricted cells can and cannot show.** An empty restricted cell means the allowlist withheld the tools only if the subagent path offers them at all in that run. In every run but one, the unrestricted `inherit` subagent was offered none either, so in those runs an empty `read`, `exact`, or `server_glob` is what an absent allowlist would also produce. The second MCP-connected gate rules out a session without the tools; it cannot rule that out for a subagent. Only `T154945`, where `inherit` and `explicit` held all three, shows the restricted arms empty while the subagent path offered the tools — and each arm is its own `claude -p` session, so even that is a comparison across sessions run minutes apart, not within one.

No separate discriminator fixture is wired (`TODO.md` #24).

## Discriminate evidence

Against the four-arm manifest, before the first billed run, `record.sh --dry-run` printed `confirmed` for a fabricated view matching the then-current `expected` and `refuted` for one where `server_glob` carried all three:

```
record: dry run — confirmed — observed {"inherit":["ListMcpResourcesTool","ReadMcpResourceDirTool","ReadMcpResourceTool"],"read":[],"exact":[],"server_glob":[]} (no file written)
record: dry run — refuted — observed {"inherit":["ListMcpResourcesTool","ReadMcpResourceDirTool","ReadMcpResourceTool"],"read":[],"exact":[],"server_glob":["ListMcpResourcesTool","ReadMcpResourceDirTool","ReadMcpResourceTool"]} (no file written)
```

Against the current manifest it printed `confirmed` for a view with `explicit` holding all three and for one with `explicit` empty, and `refuted` for one where `server_glob` held all three and for one where `inherit` and `read` each held only `ListMcpResourcesTool`:

```
record: dry run — confirmed — observed {"inherit":"none-or-all","read":[],"exact":[],"server_glob":[],"explicit":"none-or-all"} (no file written)
record: dry run — confirmed — observed {"inherit":"none-or-all","read":[],"exact":[],"server_glob":[],"explicit":"none-or-all"} (no file written)
record: dry run — refuted — observed {"inherit":"none-or-all","read":[],"exact":[],"server_glob":["ListMcpResourcesTool","ReadMcpResourceDirTool","ReadMcpResourceTool"],"explicit":"none-or-all"} (no file written)
record: dry run — refuted — observed {"inherit":["ListMcpResourcesTool"],"read":["ListMcpResourcesTool"],"exact":[],"server_glob":[],"explicit":"none-or-all"} (no file written)
```

Live runs also discriminated: `T154945` holds all three tools in `inherit` and `explicit` beside empty restricted arms.

## The nine runs

Every run below passed every gate, on 2026-10-03 against Claude Code 2.1.287, with `claude-sonnet-5-5`. Each dry run and each record is a separate invocation:

| Run | `inherit` | `read`, `exact`, `server_glob` | `explicit` |
| --- | --- | --- | --- |
| dry run 1 (four arms) | none | none | — |
| `T153722` (four arms) | none | none | — |
| dry run 2 | none | none | none |
| `T154945` | all three | none | all three |
| dry runs 3–5 | none | none | none |
| `T160044` | none | none | none |
| `T161344` | none | none | none |

`inherit` was offered the tools in one run of nine, `explicit` in one of seven, and no restricted arm in any. The two flipped cells flipped in the same invocation, though each arm is its own session, which points at something run-wide — a time window or a server-side setting — rather than at either agent; that is an inference, not a measurement. In each of the five dry runs, whose workspaces were kept, the main thread listed all three resource tools as deferred, while the unrestricted `inherit` subagent's deferred listing held `mcp__fx__alpha` and `mcp__fx__beta` and no resource tool. `T154945`'s workspace was removed on recording, before the runner kept anomalous ones, so only its record remains.

## What the record licenses

At `outbound-request`, this is evidence about what reached the model.

- **When the subagent path offered the resource tools, an MCP grant did not bring them along.** In `T154945` — the one run where `inherit` and `explicit` held all three — `read`, `exact`, and `server_glob` held none. That is Key Assumption 3's prediction, on one run, compared across sessions. Every other run's empty restricted cells carry no information about the allowlist, because `inherit` was empty too.
- **Claude Code usually offers no subagent the resource tools, even one that names them.** `inherit` was offered none in eight of nine runs, and `explicit` in six of seven. A spec cannot reliably give a Claude subagent MCP resource access on 2.1.287.
- **Not "never".** One run offered all three to both `inherit` and `explicit`, so a design must not rely on either outcome for those two cells.

## History

**2026-10-03, Claude Code 2.1.287: the belief that an unrestricted subagent receives the resource tools was refuted, and the manifest now states those cells as unstable.**

- **Old belief.** A subagent with no `tools` field inherits every session tool, the three resource tools included; `inherit` expected all three. When the `explicit` arm was added, a subagent naming the three was expected to receive them.
- **Measured.** `T153722` recorded `inherit` empty (`refuted`). `T154945` recorded `inherit` and `explicit` holding all three, under an `expected` that by then held `inherit` empty and `explicit` full (`refuted` on `inherit`). `T160044` confirmed an all-empty `expected`. The run table has the dry runs around them.
- **Acted on.** No adapter or capability accessor encoded the old belief. `expected` holds `[]` for the restricted arms and `"none-or-all"` for `inherit` and `explicit`, and `T161344` confirmed it. The projection's two-outcome cells and the kept-workspace rule were added so a future flip neither raises a false alarm nor loses its evidence.
- **Not determined.** Why one run in nine differed.

## Oracle limits

- **One model and one CLI version** per record. The flip may be timing-dependent, and its rate can depend on both.
- **A server with the `resources` capability only.** That the capability is what makes Claude Code offer the resource tools is assumed from the tools' purpose, not measured: no run connected a server without it.
- **Default tool search only.** Claude with tool search off is unmeasured.
- **Delegated subagents only.** A skill's `allowed-tools`, and an agent run as the session with `--agent`, are separate surfaces; this says nothing about whether the main thread can use the resource tools, only that it is offered them.
- **No call is made.** Whether a resource tool, once offered, reads the server's resource is not measured.

## Related

- `experiments/claude-subagent-mcp-tools/` — the same grant spellings, measured for the server's own tools.
