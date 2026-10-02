# claude-subagent-tools-empty

**Question:** Does a Claude subagent whose `tools` field is an empty list receive no tools, or the same tools as a subagent with no `tools` field?

**Driver:** `billed`. A command answers the question with no human step, but every arm spends a billed model call, so `just probe-run` withholds this package by name and reason; it runs only under `just probe-run --billed`.

**Depth:** `outbound-request`. The oracle is the request body Claude Code sends, captured through OpenTelemetry — the far end of the chain, not the provider's resolved view.

## Why it matters

agentspec's Claude adapter emits `tools: []` for an agent spec that declares `capabilities.tools: []`, and its OpenCode adapter emits a `permission` map that denies every tool for the same spec. Whether those two mean the same thing depends on what Claude does with an empty list. If Claude gives the subagent no tools, both providers mean "no tools" and agentspec can emit the empty list as is. If Claude treats the empty list as no restriction at all, the one spec would mean "nothing" on OpenCode and "everything" on Claude, and agentspec has to reject the empty list at validate time instead.

## The three arms

Every arm runs `claude -p` with `--strict-mcp-config --allowedTools Agent --effort low`. The three agents differ only in `tools`, and each carries `model: inherit`:

| Arm | Agent | `tools` |
| --- | --- | --- |
| `inherit` | `probe-tools-inherit` | none |
| `empty` | `probe-tools-empty` | `[]` — the YAML the Claude adapter emits for an empty list |
| `read` | `probe-tools-read` | `[Read]` |

Each arm's prompt is `Use the Agent tool to delegate to the <agent> subagent. Do not answer yourself.` It deliberately omits the marker: a marked main-thread request would otherwise be read as governed by the fixture.

## The oracle

`CLAUDE_CODE_ENABLE_TELEMETRY=1` plus `OTEL_LOG_RAW_API_BODIES=file:<dir>` writes each untruncated request body to `<dir>/<uuid>.request.json`. The projection reads only requests whose `.system` carries the agent body's marker, `AGENTSPEC-PROBE-MARKER-CCEMPTY8` — the subagent's own requests.

A tool can reach a request in two places, and the projection reads both. It reads `tools[]` directly, and it reads whether the request carries the text block that opens "The following deferred tools are now available via ToolSearch", which under tool search names tools the model can load without their being in `tools[]` (see `experiments/claude-subagent-mcp-tools/`). `no-tools` therefore requires both an empty `tools[]` and no deferred listing.

## Why the projection compares arms

The projection classifies `empty` against `inherit` rather than against a list of tool names:

| Value | Meaning |
| --- | --- |
| `no-tools` | `empty`'s requests carry an empty `tools[]` and no deferred listing |
| `same-as-inherit` | `empty`'s requests offer exactly what `inherit`'s offer |
| any other object | `empty` received a tool set matching neither |
| `arm-had-no-governed-request` | `empty`'s subagent never ran |

A refutation then records `same-as-inherit`, which stays the same across Claude releases that add or rename built-ins; a raw tool list would record a new `refuted` value on each such release. `same-as-inherit` reads as Claude's default because the belief it would replace is that the empty list restricts: an empty list Claude ignored would leave the subagent exactly as unrestricted as one with no `tools` field.

## Isolation

- **`--setting-sources project`** — excludes the operator's user-tier settings.
- **`--strict-mcp-config` with no `--mcp-config`** — Claude Code loads no MCP server at all, which keeps the operator's own servers out of the view and the record. `--setting-sources project` governs settings, not MCP servers, so it would not exclude them alone.
- **One fixture tree per arm**, holding only that arm's agent, so a delegation to the wrong name finds no agent and produces no governed request.
- **Unset variables.** The runner unsets `ENABLE_TOOL_SEARCH`, `ANTHROPIC_BASE_URL`, and `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS` (each changes whether tool search is on, and so whether a tool can reach the model through the deferred listing), and `CLAUDE_CODE_SUBAGENT_MODEL` and `CLAUDE_CODE_SUBAGENT_MODEL_FORCE` (which model the subagent runs).
- **The pinned model**, plus `model: inherit` in each agent, which outranks `CLAUDE_CODE_SUBAGENT_MODEL`, so the subagent runs on the session's model.
- **`--max-budget-usd 0.50`** — counts subagent spend. A cap hit stops subagent spawns, which surfaces as a marker-gate failure rather than a wrong answer.

## Model choice

Claude Sonnet 5.5 (`claude-sonnet-5-5`) at `--effort low`, as in `experiments/claude-subagent-mcp-tools/`. On 2026-10-02 against Claude Code 2.1.287, Claude Haiku 4.5 did not delegate in any arm of that package across two runs; its README's "Model choice" section has the gate output.

## The gates

A gate failure is a statement about the run, not about Claude, and writes no record.

- **Marker** (`probe_claude_gate_marker`). Every arm holds a request whose `.system` carries the marker, so the arm's subagent actually ran.
- **Model** (`probe_claude_gate_model`). Every governed request ran on the pinned model.
- **Inherit (runner-local).** `inherit`'s governed requests carry `Read` in `tools[]`. That shows an unrestricted subagent gets built-ins in this run, so a `no-tools` value for `empty` can only mean the empty list restricted it — not that every subagent here was offered nothing.
- **Read restricts (runner-local).** `read`'s governed requests carry `Read` in `tools[]`, and offer fewer distinct tools than `inherit`'s. That shows a non-empty `tools` field restricted the subagent in this run, so a `same-as-inherit` value for `empty` would mean Claude reads the empty list as unrestricted — not that the run ignored every agent's `tools` field.

The two runner-local gates are the controls for the two values the projection can take: `inherit` for `no-tools`, `read` for `same-as-inherit`.

## The assertion discriminates

From the Sonnet dry run on 2026-10-02 against Claude Code 2.1.287, the governed request of each arm:

| Arm | `tools[].name`, sorted | Deferred listing |
| --- | --- | --- |
| `inherit` | `Agent`, `Bash`, `DeferredToolPlaceholder`, `Edit`, `Read`, `Skill`, `ToolSearch`, `Write`, `advisor` | `EnterWorktree`, `ExitWorktree`, `Monitor`, `NotebookEdit`, `SendMessage`, `TaskStop`, `WebFetch`, `WebSearch` |
| `empty` | none | none |
| `read` | `Read`, `advisor` | none |

The dry run printed `confirmed`, observing `{"empty": "no-tools"}`. Against the same saved view, `record.sh --dry-run` printed `refuted`, observing `{"empty": "same-as-inherit"}`, for a copy with `empty` replaced by `inherit`'s requests.

`read` received `advisor` beside the one tool it listed, while `empty` received nothing at all. Where `advisor` comes from is not measured here; the read-restricts gate compares tool counts rather than requiring `Read` alone for that reason.

## Oracle limits

- **One model and one CLI version** per record.
- **Default tool search only.** Claude with tool search off — the default for users behind a custom `ANTHROPIC_BASE_URL` — is unmeasured.
- **No MCP servers.** Whether an empty list also withholds MCP tools is not measured here; `experiments/claude-subagent-mcp-tools/` shows a non-empty list without MCP entries withholds them.
- **Delegated subagents only.** A skill's `allowed-tools`, and an agent run as the session with `--agent`, are separate surfaces.
- **This stops at the request.** It shows the model was offered no tools, not how the subagent behaves without them.

## Related

- `experiments/claude-subagent-mcp-tools/` — the apparatus this package is built from, and which MCP tools each `tools` spelling grants.
- `experiments/opencode-agent-permission-deny-all/` — what OpenCode offers under the deny-all `permission` map agentspec emits for the same spec.
