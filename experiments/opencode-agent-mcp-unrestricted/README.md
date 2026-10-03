# opencode-agent-mcp-unrestricted

**Question:** Is an OpenCode agent with no `permission` map offered every tool of a connected MCP server, where an agent whose map is only `"*": "deny"` with `external_directory` and `doom_loop` restated is offered none?

**Driver:** `unattended`. No human step, no credentials, no network, and no model quota: OpenCode talks only to a fake provider on loopback.

**Depth:** `outbound-request`. The oracle is the request body OpenCode sends to the provider, not `opencode debug agent`'s resolved view.

## Why it matters

This is Key Assumption 6 of `$THOUGHTS_DIR/designs/2026-10-03-agentspec-mcp-tool-grants.md`. A spec that declares no `capabilities.tools` gets no `permission` map from agentspec's OpenCode adapter, and the design treats such an agent as unrestricted — offered every connected server's tools, so there is nothing for a grant to add. A spec declaring `tools: []` gets the map in the `deny_all` arm, and the design treats it as offered no MCP tool at all.

- **If `no_map` is offered both `fx` tools**, the adapter needs nothing for an unrestricted spec: omitting the map is the whole-server grant.
- **If `no_map` is offered fewer**, OpenCode withholds MCP tools by default, and an unrestricted spec would need an explicit allow for every declared server.
- **If `deny_all` is offered any `fx` tool**, the map agentspec writes for `tools: []` does not hide MCP tools, and an empty grant would leak every server.

`experiments/opencode-agent-permission-deny-all/` measured the same deny-all with a re-allowed tool beside it. Its `baseline` arm, which has no map, is a gate there rather than a finding, so no record of it exists to design on. This package makes it one.

## The arms

Each arm runs `opencode run --agent <agent> -m fake/m "hi"` in its own project. The two agents are identical except for `permission`:

| Arm | Agent | `permission` map, in authored order |
| --- | --- | --- |
| `no_map` | `probe-no-map` | none |
| `deny_all` | `probe-deny-all` | `"*": deny`, `external_directory: ask`, `doom_loop: ask` |

The `deny_all` map is the one agentspec's OpenCode adapter emits for a spec declaring `tools: []`.

Every arm also connects `fx`, a local MCP stdio server (`fixtures/fx_server.py`) offering two tools, `alpha` and `beta`, which OpenCode names `fx_alpha` and `fx_beta`. Two tools are what lets "every tool of the server" be told apart from "one tool of it".

## The oracle

`fixtures/fake_provider.py` is an OpenAI-compatible chat endpoint on `127.0.0.1`. The project's `opencode.json` declares it as a provider through `@ai-sdk/openai-compatible`, which is built into the OpenCode binary, and the fake writes every request body it receives to the arm's `sink/` directory. The tool names read from those bodies — `tools[].function.name` — are what OpenCode actually offered the model.

The projection reads only requests whose `role: "system"` message carries the agent body's marker, `AGENTSPEC-PROBE-MARKER-OCMCPU1`, and keeps only the names that start with `fx_`. OpenCode also sends a title-generation request per run, with its own system prompt and `tools: []`; the marker keeps it out. Built-in tools are left out of the value because their set varies by OpenCode version, and the question is about the server's tools.

## Isolation

- Every `XDG_*` directory (`CONFIG`, `CACHE`, `DATA`, `STATE`) points into the arm's workspace, which keeps the operator's global `opencode.json`, plugins, and cache out of the run.
- `HOME` points into the arm's workspace too. OpenCode also reads `~/.opencode/` as a config directory, located from the home directory rather than any `XDG_*` variable.
- Not covered: OpenCode's system-managed config directory and macOS managed preferences, which an administrator rather than the operator controls.
- `OPENCODE_CONFIG`, `OPENCODE_CONFIG_DIR`, `OPENCODE_CONFIG_CONTENT`, and `OPENCODE_PERMISSION` are cleared, since each would inject config or permission rules from the operator's shell. `OPENCODE_DISABLE_PROJECT_CONFIG` is cleared as well: set, it would discard the fixture's own agents.
- `OPENCODE_DISABLE_AUTOUPDATE=1` and `OPENCODE_DISABLE_MODELS_FETCH=1`, so the run touches no network.
- The fake binds loopback only, and runs one arm at a time.
- The fake, the MCP server, and the request dumps sit outside each arm's `project/` directory. The project's `opencode.json` still names the server's path and the fake's port, as it must for OpenCode to reach them; the model is a fake that searches nothing, so naming the paths cannot change what is measured.

## The gates

A gate failure keeps the workspace and exits 1 without recording, because each describes an apparatus failure that would otherwise be recorded as `refuted`.

- **Marker.** Every arm holds at least one request carrying the marker in a system message. OpenCode warns rather than fails on an unknown `--agent` and runs its default agent instead, whose requests carry no marker.
- **Stamp, per arm.** `fx_server.py` writes `<arm>/fx.stamp` when it answers `tools/list`. Without it, an empty `deny_all` would read as "the deny-all hid `fx`" when `fx` never connected, and an empty `no_map` as "OpenCode withholds MCP tools" when it had none to offer.

There is no baseline tool-listing gate, unlike the deny-all package. `no_map` is the arm that would carry it, and here it is the finding. The cost is that the stamp shows `fx` answered `tools/list` during the run, not before the marked request was built. A server that connected too late would record `refuted` on `no_map`, which is a false refutation rather than a false pass; read the arm's request dump before treating such a record as a finding.

## The assertion is relational across arms

The two agents differ only in `permission`, so distinct observed values across the arms show the projection reads each arm independently. An arm with no marked request projects to `"arm-had-no-governed-request"`, so an unmeasured `deny_all` can never confirm `[]`. An arm whose marked requests disagree projects to `{inconsistent: [...]}`, so one request carrying both `fx` tools cannot speak for another that carried neither. No separate discriminator fixture is wired (`TODO.md` #24).

`expected` states the design's belief: no map offers both `fx` tools, and the deny-all offers neither.

## Discriminate evidence

Before the first run, `record.sh --dry-run` against fabricated views printed `confirmed` for one matching `expected`, and `refuted` for one where `deny_all` carried `["fx_alpha", "fx_beta"]`, one where `no_map` carried only `fx_alpha`, one where `deny_all` held only an unmarked request, and one where `no_map`'s two marked requests disagreed:

```
record: dry run — confirmed — observed {"no_map":["fx_alpha","fx_beta"],"deny_all":[]} (no file written)
record: dry run — refuted — observed {"no_map":["fx_alpha","fx_beta"],"deny_all":["fx_alpha","fx_beta"]} (no file written)
record: dry run — refuted — observed {"no_map":["fx_alpha"],"deny_all":[]} (no file written)
record: dry run — refuted — observed {"no_map":["fx_alpha","fx_beta"],"deny_all":"arm-had-no-governed-request"} (no file written)
record: dry run — refuted — observed {"no_map":{"inconsistent":[[],["fx_alpha","fx_beta"]]},"deny_all":[]} (no file written)
```

Two records exist from 2026-10-03, both `confirmed` with the same `observed`. The first (`T124612`) was made under a projection that merged the `fx_` names across an arm's marked requests; the second (`T130943`) under the current one, which flags disagreeing requests.

From the `PROBE_DRY_RUN=1` run on 2026-10-03 against opencode **1.18.34**, each arm sent two requests, the agent's and the title generator's. The marked ones carried:

| Arm | `tools[].function.name`, sorted |
| --- | --- |
| `no_map` | `bash`, `edit`, `fx_alpha`, `fx_beta`, `glob`, `grep`, `read`, `skill`, `task`, `todowrite`, `webfetch`, `write` |
| `deny_all` | none |

## What the record licenses

At `outbound-request`, this is evidence about what reached the model.

- **An agent with no `permission` map is offered every tool of a connected server.** `no_map` carried `fx_alpha` and `fx_beta`. For an unrestricted spec the OpenCode adapter needs to emit nothing to grant MCP tools.
- **The map agentspec writes for `tools: []` hides every MCP tool.** `deny_all` carried neither `fx` tool.

The record keeps only `fx_` names. That `no_map` also carried the built-ins, and `deny_all` no tool at all, comes from the dry run's table above, not from the record.

## Oracle limits

- **Primary agents only.** The agents run as primaries through `opencode run --agent`, while agentspec emits `mode: subagent`. OpenCode's source shows a subagent session adds only `todowrite` and `task` denies, plus the parent session's denies and `external_directory` rules (`agent/subagent-permissions.ts` L14–27); none of those touches an MCP tool id. That is a reading of source, not a measurement.
- **No session or global permission rules.** The isolation strips them on purpose. A user whose global config denies `fx_*` would see `no_map` offered less; that is the user's own rule, not OpenCode's default.
- **One provider package.** It shows what OpenCode sends to a provider using `@ai-sdk/openai-compatible`. A different provider package could, in principle, filter tools differently.
- **No MCP resources.** `fx_server.py` offers none, so this says nothing about OpenCode's resource tools.
- **One OpenCode version per record.**

## Related

- `experiments/opencode-agent-permission-deny-all/` — the deny-all with built-ins, a lone `edit`, or one MCP tool re-allowed.
