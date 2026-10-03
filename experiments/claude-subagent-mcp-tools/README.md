# claude-subagent-mcp-tools

**Question:** For each spelling of an MCP grant in a Claude subagent's `tools` field, which MCP tools does the subagent's model request carry, directly or as deferred, and does `ToolSearch` come with them?

**Driver:** `billed`. A command answers the question with no human step, but every arm spends a billed model call, so `just probe-run` withholds this package by name and reason; it runs only under `just probe-run --billed`.

**Depth:** `outbound-request`. The oracle is the request body Claude Code sends, captured through OpenTelemetry — the far end of the chain, not the provider's resolved view.

## Why it matters

`$THOUGHTS_DIR/ideas/2026-10-01-agentspec-mcp-tools-in-capabilities.md` proposes letting a spec grant an agent individual MCP tools, or a whole server. Which `tools` spelling agentspec should emit for Claude, and which Claude Code version it can require, depend on what each spelling actually puts in front of the model. Under tool search, an MCP tool is offered by name and loaded through `ToolSearch`. A subagent granted an MCP tool without `ToolSearch` would then see a tool it could not load — the failure claude-code#25200 suggests. This package measures whether that happens.

## The six arms

Every arm runs `claude -p` with `--mcp-config <arm>/mcp.json --strict-mcp-config --allowedTools Agent --effort low`, connecting `fx`, a local MCP stdio server (`fixtures/fx_server.py`) with two tools, `alpha` and `beta`. Two tools are what lets "only the granted tool is visible" be told apart from "every tool is visible". The six agents differ only in `tools`:

| Arm | Agent | `tools` |
| --- | --- | --- |
| `inherit` | `probe-mcp-inherit` | none |
| `read` | `probe-mcp-read` | `Read` |
| `exact` | `probe-mcp-exact` | `Read, mcp__fx__alpha` |
| `exact_search` | `probe-mcp-exact-search` | `Read, ToolSearch, mcp__fx__alpha` |
| `server` | `probe-mcp-server` | `Read, mcp__fx` |
| `server_glob` | `probe-mcp-server-glob` | `Read, mcp__fx__*` |

Each arm's prompt is `Use the Agent tool to delegate to the <agent> subagent. Do not answer yourself.` It deliberately omits the marker: a marked main-thread request would otherwise be read as governed by the fixture.

## The oracle

`CLAUDE_CODE_ENABLE_TELEMETRY=1` plus `OTEL_LOG_RAW_API_BODIES=file:<dir>` writes each untruncated request body to `<dir>/<uuid>.request.json`. The projection reads only requests whose `.system` carries the agent body's marker, `AGENTSPEC-PROBE-MARKER-CCMCP4T` — the subagent's own requests.

An MCP tool can reach a request in two places, and the projection reads both:

- **Directly**, as an entry in `tools[]`.
- **Deferred**, named on its own line in a text block that opens "The following deferred tools are now available via ToolSearch." A spike on 2026-10-01 against Claude Code 2.1.284 found tool search puts only `ToolSearch` in `tools[]` and lists the MCP tools there instead. Reading `tools[]` alone would report every deferred grant as absent.

## Isolation

- **`--setting-sources project`** — excludes the operator's user-tier settings.
- **`--strict-mcp-config` with a per-arm `--mcp-config`** — excludes every MCP server but `fx`. With `-p`, `--mcp-config` also makes Claude Code wait for `fx` to connect before the first turn, so a subagent's allowlist cannot resolve before the server's tools exist (claude-code#79728).
- **One fixture tree per arm**, holding only that arm's agent, so a delegation to the wrong name finds no agent and produces no governed request.
- **Unset variables.** The runner unsets `ENABLE_TOOL_SEARCH`, `ANTHROPIC_BASE_URL`, and `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS` (each changes whether tool search is on — a custom base URL turns it off by default), `CLAUDE_CODE_MCP_STARTUP_WAIT_MS` (whether `fx` has connected when the subagent spawns), and `CLAUDE_CODE_SUBAGENT_MODEL` and `CLAUDE_CODE_SUBAGENT_MODEL_FORCE` (which model the subagent runs).
- **The pinned model**, plus `model: inherit` in each agent, which outranks `CLAUDE_CODE_SUBAGENT_MODEL`, so the subagent runs on the session's model.
- **`--max-budget-usd 0.50`** — counts subagent spend. A cap hit stops subagent spawns, which surfaces as a marker-gate failure rather than a wrong answer.

## Model choice

The rule: Claude Haiku 4.5 (`claude-haiku-4-5-20251001`), unless Haiku cannot run the probe — it does not delegate, or Claude Code does not enable tool search for it — in which case Claude Sonnet 5.5 (`claude-sonnet-5-5`) at `--effort low`.

**This package runs on Sonnet 5.5.** On 2026-10-02 against Claude Code 2.1.287, two consecutive Haiku dry runs failed the marker gate in every arm:

```
probe: an arm captured no request whose .system carries the fixture marker.
probe: that arm never engaged the fixture, so its value describes nothing.
```

Each arm held a single main-thread request. That request offered `Agent` in `tools[]` and listed the arm's agent among the available agent types, so the apparatus was intact. Haiku answered in prose instead of delegating, e.g. "I'm ready to help. What would you like me to work on?" The Sonnet run that followed passed every gate.

## The gates

A gate failure is a statement about the run, not about Claude, and writes no record.

- **Stamp, per arm (runner-local).** `fx_server.py` writes `<arm>/fx.stamp` when it answers `tools/list`. Without it, an arm's tool set says nothing about MCP grants. The check only tests that a file exists, with no projection logic for a fabricated view to exercise, so it stays in the runner.
- **Marker** (`probe_claude_gate_marker`). Every arm holds a request whose `.system` carries the marker, so the arm's subagent actually ran.
- **Model** (`probe_claude_gate_model`). Every governed request ran on the pinned model. The subagent's model decides whether tool search is on, and a delegation call's own `model` parameter outranks the agent's frontmatter — only the request shows that one was passed.
- **Deferred listing on `inherit`** (`probe_claude_gate_deferred_mcp`). The `inherit` arm's governed requests list `mcp__fx__alpha` and `mcp__fx__beta` as deferred, and carry `ToolSearch`. `inherit` has no allowlist, so its expected tool set is unconditional; a failure here means the run was not under default tool search, which is the configuration this package measures. The cost is that `inherit` is a precondition rather than a finding: any recorded run has that cell by construction, and a Claude that stopped deferring for an unrestricted subagent would fail this gate rather than record `refuted`. The gate's diagnostic names both causes, a model without tool search and such a change.
- **MCP connected before delegation** (`probe_claude_gate_mcp_connected`). In every arm, every main-thread request that offers `Agent` lists `mcp__fx__alpha` and `mcp__fx__beta`, directly or as deferred. The delegating request is one of them, so fx's tools existed before the subagent did. The stamp alone would not show this, and without it the `read` arm's expected value — no MCP tools — would read identically to "fx had not connected yet".

`probe_claude_gate_control` is not used: it gates an effort control set, and nothing here measures effort.

The four jq gates live in `experiments/lib/probe-claude-otel.sh` with bats coverage in `experiments/lib/tests/probe-claude-otel.bats`, so they are exercised without a paid run.

## The assertion is relational across arms

The six agents differ only in `tools`, so distinct observed values across arms show the projection reads each arm independently. An arm with no governed request projects to `"arm-had-no-governed-request"`; without it, an unmeasured `read` arm would project exactly to its expected value. No separate discriminator fixture is wired (`TODO.md` #24).

`expected` states agentspec's documented belief: tool search defers every granted MCP tool, a granted MCP tool always arrives with `ToolSearch`, and an allowlist without MCP entries grants no MCP tool.

## Discriminate evidence

Before the first billed run, `record.sh --dry-run` against two fabricated views printed `confirmed` for one matching `expected` and `refuted` for one where `exact` carried both fx tools.

From the Sonnet dry run on 2026-10-02 against Claude Code 2.1.287 (`status` and `observed`, excerpted from the printed record):

```
{"status":"refuted","observed":{
  "inherit":      {"direct":[],"deferred":["mcp__fx__alpha","mcp__fx__beta"],"tool_search":true},
  "read":         {"direct":[],"deferred":[],"tool_search":false},
  "exact":        {"direct":["mcp__fx__alpha"],"deferred":[],"tool_search":false},
  "exact_search": {"direct":[],"deferred":["mcp__fx__alpha"],"tool_search":true},
  "server":       {"direct":["mcp__fx__alpha","mcp__fx__beta"],"deferred":[],"tool_search":false},
  "server_glob":  {"direct":["mcp__fx__alpha","mcp__fx__beta"],"deferred":[],"tool_search":false}}}
```

`read`, `exact`, and `server` all differ, so the projection reads each arm on its own.

Both committed records carry these same values. The first (`2026-10-02T005617`) predates the MCP-connected gate; the second (`2026-10-02T012920`) was made with every gate above in place.

## What the record licenses

At `outbound-request`, this is evidence about what reached the model.

- **An allowlist without `ToolSearch` loads granted MCP tools directly.** In `exact`, `server`, and `server_glob`, the subagent's `tools[]` held the granted fx tools themselves, with no deferred listing and no `ToolSearch`. The tools were offered to the model directly rather than named as deferred with no way to load them, so the failure claude-code#25200 suggests did not occur here. Whether the model then calls them successfully is not measured.
- **Listing `ToolSearch` keeps the grant deferred.** In `exact_search`, `mcp__fx__alpha` was deferred, `ToolSearch` was present, and `mcp__fx__beta` was absent.
- **The server name alone grants every tool of that server.** `mcp__fx` and `mcp__fx__*` both granted `alpha` and `beta`.
- **An allowlist without MCP entries grants none.** `read` received no fx tool in either place.

The first Sonnet record is `refuted` because `expected` assumed tool search would defer every granted MCP tool. The `exact`, `server`, and `server_glob` arms show it defers only when the subagent's tool set includes `ToolSearch`.

## History

**2026-10-02, Claude Code 2.1.287: the deferral belief was refuted, and `expected` now holds the measured value.**

- **Old belief.** Tool search defers every granted MCP tool, and a granted tool always arrives with `ToolSearch`. The `exact`, `server`, and `server_glob` arms expected `{direct: [], deferred: [<granted>], tool_search: true}`.
- **Measured** (`results/2026-10-02T012920-claude-2.1.287__Claude_Code_.json`). Without `ToolSearch` in the allowlist, those arms received the granted tools directly in `tools[]`, with no deferred listing and no `ToolSearch`. The `inherit`, `read`, and `exact_search` arms matched the old belief.
- **Acted on.** No adapter or capability accessor encoded the old belief. The MCP-tools idea (`$THOUGHTS_DIR/ideas/2026-10-01-agentspec-mcp-tools-in-capabilities.md`) records the measured result, and it removes any need for agentspec to emit `ToolSearch` alongside an MCP grant.
- **Not determined.** Whether the model calls a directly loaded MCP tool successfully. This probe stops at the request.

The `refuted` record stays, because records are append-only. `just probe-status` keeps reporting it until a `just probe-run --billed` run records under the corrected `expected`.

## Oracle limits

- **One model and one CLI version** per record. The deferral behavior depends on both.
- **Default tool search only.** Claude with tool search off — the default for users behind a custom `ANTHROPIC_BASE_URL` — is unmeasured.
- **User- and project-configured servers only.** Plugin-bundled servers use `mcp__plugin_<plugin>_<server>__<tool>` and are not measured here; `experiments/claude-subagent-plugin-mcp-tools/` measures them.
- **`inherit` is not evidence.** The deferred-listing gate requires its value, so no record can show it differing.
- **Delegated subagents only.** A skill's `allowed-tools`, and an agent run as the session with `--agent`, are separate surfaces; `experiments/claude-skill-mcp-allowed-tools/` measures the first.
- **Delegation mode not recorded.** No prompt names `run_in_background`, and `experiments/claude-background-subagent-mcp-tools/` measured that such a delegation launches in the background under `-p` on 2.1.287; these subagents most likely ran there.
