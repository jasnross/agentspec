# OpenCode — `"*": false` in an agent's `tools` map, before and after an MCP tool

**Question.** When an OpenCode agent's `tools` map puts `"*": false` before an allowed MCP tool, does the model request carry only that tool, and does putting `"*": false` after it carry no tools at all?

**Why it matters.** agentspec's OpenCode adapter wrote `false` for the twelve canonical tool ids an agent did not declare, so every tool outside them — every MCP tool among them — stayed enabled on an agent that declared `capabilities.tools`, while the same spec yields an agent with no MCP access on Claude, whose `tools` is an allowlist. The fix emits a deny-all first and then allows each listed tool, which depends on OpenCode resolving the map's keys last-match-wins in authored order. The adapter now writes that shape to the agent `permission` field rather than `tools`; `experiments/opencode-agent-permission-deny-all/` measures that field directly. The fourth goal of `$THOUGHTS_DIR/ideas/2026-10-01-agentspec-mcp-tools-in-capabilities.md` — an agent granting no MCP tools has no MCP access on every provider that can restrict tools — depends on the same answer.

**Driver:** `unattended`. No human step, no credentials, no network, and no model quota: OpenCode talks only to a fake provider on loopback. **Depth:** `outbound-request`.

## Running it

```sh
experiments/opencode-agent-tools-deny-all/probe.sh
PROBE_DRY_RUN=1 experiments/opencode-agent-tools-deny-all/probe.sh   # evaluate without recording
```

## The arms

Each arm runs `opencode run --agent <agent> -m fake/m "hi"` in its own project. The three agents are identical except for `tools`:

| Arm         | Agent             | `tools` map                         |
| ----------- | ----------------- | ----------------------------------- |
| `baseline`  | `probe-baseline`  | none                                |
| `allow_one` | `probe-allow-one` | `"*": false`, then `fx_alpha: true` |
| `reversed`  | `probe-reversed`  | `fx_alpha: true`, then `"*": false` |

Every arm also connects `fx`, a local MCP stdio server (`fixtures/fx_server.py`) offering two tools, `alpha` and `beta`, which OpenCode names `fx_alpha` and `fx_beta`. Two tools are what lets "only the granted tool is visible" be told apart from "every tool is visible".

## The oracle

`fixtures/fake_provider.py` is an OpenAI-compatible chat endpoint on `127.0.0.1`. The project's `opencode.json` declares it as a provider through `@ai-sdk/openai-compatible`, which is built into the OpenCode binary, and the fake writes every request body it receives to the arm's `sink/` directory. The tool list read from those bodies — `tools[].function.name` — is the set OpenCode actually offered the model.

That is what makes this `outbound-request` evidence. `opencode debug agent` stops at the resolved agent config, and `TODO.md` #14 shows OpenCode's resolved tool map and the keys it honors diverge, so a resolved map is weak evidence about what the model is offered.

The projection reads only requests whose `role: "system"` message carries the agent body's marker, `AGENTSPEC-PROBE-MARKER-OCDENY7`. OpenCode also sends a title-generation request per run, with its own system prompt and `tools: []`; the marker keeps it out.

## Isolation

- Every `XDG_*` directory (`CONFIG`, `CACHE`, `DATA`, `STATE`) points into the arm's workspace, which keeps the operator's global `opencode.json`, plugins, and cache out of the run.
- `HOME` points into the arm's workspace too. OpenCode also reads `~/.opencode/` as a config directory, located from the home directory rather than any `XDG_*` variable, so an operator's `~/.opencode/` agents, plugins, or `tools` rules would otherwise reach every arm.
- Not covered: OpenCode's system-managed config directory and macOS managed preferences, which an administrator rather than the operator controls. Neither is set on a typical development machine.
- `OPENCODE_CONFIG`, `OPENCODE_CONFIG_DIR`, `OPENCODE_CONFIG_CONTENT`, and `OPENCODE_PERMISSION` are cleared. The `XDG_*` and `HOME` redirects do not block them, and each would inject config, or tool rules directly, from the operator's shell. `OPENCODE_DISABLE_PROJECT_CONFIG` is cleared as well: set, it would discard the fixture's own agents and fail every arm at the marker gate with no obvious cause.
- `OPENCODE_DISABLE_AUTOUPDATE=1` and `OPENCODE_DISABLE_MODELS_FETCH=1`, so the run touches no network.
- The fake binds loopback only, and runs one arm at a time.
- The fake, the MCP server, and the request dumps sit outside each arm's `project/` directory. The project's `opencode.json` still names the server's path and the fake's port, as it must for OpenCode to reach them. The contract's separation rule guards against an agent finding the apparatus and answering from it; here the oracle is the request dump and the model is a fake that searches nothing, so naming the paths cannot change what is measured.

## The gates

A gate failure keeps the workspace and exits 1 without recording, because each describes an apparatus failure that would otherwise be recorded as `refuted`.

- **Marker.** Every arm holds at least one request carrying the marker in a system message. OpenCode warns rather than fails on an unknown `--agent` and runs its default agent instead, whose requests carry no marker.
- **Stamp, per arm.** `fx_server.py` writes `<arm>/fx.stamp` when it answers `tools/list`. Without it, an arm's tool list says nothing about MCP tools — an empty `reversed` would read as "deny-all hid `fx_alpha`" when `fx_alpha` never existed.
- **Baseline tool listing.** `baseline`'s marked requests list both `fx_alpha` and `fx_beta`. The stamp proves the server connected; this proves its tools reach a request when no `tools` map filters them.

`baseline` is not in the projection, because its built-in tool set varies by OpenCode version. It is the gate above instead.

## The assertion is relational across arms

The three agents differ only in `tools`, so distinct observed values across arms show the projection reads each arm independently. No separate discriminator fixture is wired (`TODO.md` #24).

An arm with no marked request projects to `"arm-had-no-governed-request"`, so an unmeasured `reversed` can never confirm `[]`. An arm whose marked requests disagree projects to `{inconsistent: [...]}`.

## The assertion discriminates

Measured 2026-10-02 against opencode **1.18.34**, from a `PROBE_DRY_RUN=1` run. Each arm sent two requests (the agent's and the title generator's); these are the marked ones:

| Arm | `tools[].function.name`, sorted |
| --- | --- |
| `baseline` | `bash`, `edit`, `fx_alpha`, `fx_beta`, `glob`, `grep`, `read`, `skill`, `task`, `todowrite`, `webfetch`, `write` |
| `allow_one` | `fx_alpha` |
| `reversed` | none |

Against the same saved view, `record.sh --dry-run` printed `refuted` for two altered copies:

- `allow_one` replaced by `baseline`'s requests, which projects `allow_one` to the baseline list.
- `reversed` set to `[]`, which projects `reversed` to `"arm-had-no-governed-request"`.

## Limits of this oracle

- It shows what OpenCode sends to a provider using the `@ai-sdk/openai-compatible` package. A different provider package could, in principle, filter tools differently.
- Title-generation requests are excluded by the marker, so this says nothing about the tools those carry.
- One OpenCode version per record. The key order the answer depends on is OpenCode's resolution order for `tools` keys, which a release can change.

## Related

- `experiments/opencode-agent-permission-deny-all/` — the same deny-all under the agent `permission` field, which is what the adapter emits.
- `TODO.md` #14 — OpenCode's resolved tool map and the keys it honors diverge.
- `experiments/opencode-skill-frontmatter-discard/` — OpenCode discards `tools` on the skill surface entirely, at `resolved-config` depth.
