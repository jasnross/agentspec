# OpenCode — an agent `permission` map that leads with `"*": "deny"`

**Question.** When an OpenCode agent's `permission` map leads with `"*": "deny"`, which tools does the model request carry for re-allowed built-ins, a lone `edit`, and a re-allowed MCP tool, and does putting `"*": "deny"` after the allow carry no tools?

**Why it matters.** agentspec's OpenCode adapter writes a spec's declared `capabilities.tools` as an agent `permission` map of this shape: `"*": "deny"`, then `allow` for each declared tool, then `external_directory: "ask"` and `doom_loop: "ask"`. The design rests on three things this package measures: the deny-all hides every tool OpenCode would otherwise offer, MCP tools included; each re-allowed permission brings its tools back; and the frontmatter path keeps the map's authored key order, which is what OpenCode's last-match-wins resolution reads. `experiments/opencode-agent-tools-deny-all/` covers only MCP ids under the deprecated `tools` map, which OpenCode converts into `permission` rules — a conversion this package does not rely on.

**Driver:** `unattended`. No human step, no credentials, no network, and no model quota: OpenCode talks only to a fake provider on loopback. **Depth:** `outbound-request`.

## Running it

```sh
experiments/opencode-agent-permission-deny-all/probe.sh
PROBE_DRY_RUN=1 experiments/opencode-agent-permission-deny-all/probe.sh   # evaluate without recording
```

## The arms

Each arm runs `opencode run --agent <agent> -m fake/m "hi"` in its own project. The five agents are identical except for `permission`, and none has a `tools` key. Every map but `reversed`'s has the shape the adapter emits:

| Arm | Agent | `permission` map, in authored order |
| --- | --- | --- |
| `baseline` | `probe-baseline` | none |
| `read_edit_bash` | `probe-read-edit-bash` | `"*": deny`, `bash: allow`, `edit: allow`, `read: allow`, `external_directory: ask`, `doom_loop: ask` |
| `edit_only` | `probe-edit-only` | `"*": deny`, `edit: allow`, `external_directory: ask`, `doom_loop: ask` |
| `mcp_one` | `probe-mcp-one` | `"*": deny`, `fx_alpha: allow`, `external_directory: ask`, `doom_loop: ask` |
| `reversed` | `probe-reversed` | `fx_alpha: allow`, `"*": deny`, `external_directory: ask`, `doom_loop: ask` |

Every arm also connects `fx`, a local MCP stdio server (`fixtures/fx_server.py`) offering two tools, `alpha` and `beta`, which OpenCode names `fx_alpha` and `fx_beta`. Two tools are what lets "only the granted tool is visible" be told apart from "every tool is visible".

## The oracle

`fixtures/fake_provider.py` is an OpenAI-compatible chat endpoint on `127.0.0.1`. The project's `opencode.json` declares it as a provider through `@ai-sdk/openai-compatible`, which is built into the OpenCode binary, and the fake writes every request body it receives to the arm's `sink/` directory. The tool list read from those bodies — `tools[].function.name` — is the set OpenCode actually offered the model.

That is what makes this `outbound-request` evidence. `opencode debug agent` stops at the resolved agent config, and `TODO.md` #14 shows OpenCode's resolved tool keys and the keys it honors diverge, so a resolved map is weak evidence about what the model is offered.

The projection reads only requests whose `role: "system"` message carries the agent body's marker, `AGENTSPEC-PROBE-MARKER-OCPERM5`. OpenCode also sends a title-generation request per run, with its own system prompt and `tools: []`; the marker keeps it out.

## Isolation

- Every `XDG_*` directory (`CONFIG`, `CACHE`, `DATA`, `STATE`) points into the arm's workspace, which keeps the operator's global `opencode.json`, plugins, and cache out of the run.
- `HOME` points into the arm's workspace too. OpenCode also reads `~/.opencode/` as a config directory, located from the home directory rather than any `XDG_*` variable, so an operator's `~/.opencode/` agents, plugins, or permission rules would otherwise reach every arm.
- Not covered: OpenCode's system-managed config directory and macOS managed preferences, which an administrator rather than the operator controls. Neither is set on a typical development machine.
- `OPENCODE_CONFIG`, `OPENCODE_CONFIG_DIR`, `OPENCODE_CONFIG_CONTENT`, and `OPENCODE_PERMISSION` are cleared. The `XDG_*` and `HOME` redirects do not block them, and each would inject config, or permission rules directly, from the operator's shell. `OPENCODE_DISABLE_PROJECT_CONFIG` is cleared as well: set, it would discard the fixture's own agents and fail every arm at the marker gate with no obvious cause.
- `OPENCODE_DISABLE_AUTOUPDATE=1` and `OPENCODE_DISABLE_MODELS_FETCH=1`, so the run touches no network.
- The fake binds loopback only, and runs one arm at a time.
- The fake, the MCP server, and the request dumps sit outside each arm's `project/` directory. The project's `opencode.json` still names the server's path and the fake's port, as it must for OpenCode to reach them. The contract's separation rule guards against an agent finding the apparatus and answering from it; here the oracle is the request dump and the model is a fake that searches nothing, so naming the paths cannot change what is measured.

## The gates

A gate failure keeps the workspace and exits 1 without recording, because each describes an apparatus failure that would otherwise be recorded as `refuted`.

- **Marker.** Every arm holds at least one request carrying the marker in a system message. OpenCode warns rather than fails on an unknown `--agent` and runs its default agent instead, whose requests carry no marker.
- **Stamp, per arm.** `fx_server.py` writes `<arm>/fx.stamp` when it answers `tools/list`. Without it, an arm's tool list says nothing about MCP tools — an empty `reversed` would read as "deny-all hid `fx_alpha`" when `fx_alpha` never existed.
- **Baseline tool listing.** `baseline`'s marked requests list both `fx_alpha` and `fx_beta`. The stamp proves the server connected; this proves its tools reach a request when no `permission` map filters them.

`baseline` is not in the projection, because its built-in tool set varies by OpenCode version. It is the gate above instead, and it is what every expected value is non-default against.

## The assertion is relational across arms

The five agents differ only in `permission`, so distinct observed values across arms show the projection reads each arm independently. No separate discriminator fixture is wired (`TODO.md` #24).

`reversed` is what tests key order. Its map holds the same keys as `mcp_one`'s with `fx_alpha` moved ahead of the deny-all. If OpenCode's frontmatter path reordered keys before resolving them — sorting them, say, which puts `*` first — `reversed` would resolve exactly like `mcp_one` and offer `fx_alpha`. Expecting no tools is therefore what shows the authored order reaches the last-match-wins resolution intact.

An arm with no marked request projects to `"arm-had-no-governed-request"`, so an unmeasured `reversed` can never confirm `[]`. An arm whose marked requests disagree projects to `{inconsistent: [...]}`.

## The assertion discriminates

Measured 2026-10-02 against opencode **1.18.34**, from a `PROBE_DRY_RUN=1` run. Each arm sent two requests (the agent's and the title generator's); these are the marked ones:

| Arm | `tools[].function.name`, sorted |
| --- | --- |
| `baseline` | `bash`, `edit`, `fx_alpha`, `fx_beta`, `glob`, `grep`, `read`, `skill`, `task`, `todowrite`, `webfetch`, `write` |
| `read_edit_bash` | `bash`, `edit`, `read`, `write` |
| `edit_only` | `edit`, `write` |
| `mcp_one` | `fx_alpha` |
| `reversed` | none |

Every measured arm differs from `baseline`. Against the same saved view, `record.sh --dry-run` printed `refuted` for a copy with `edit_only` replaced by `baseline`'s requests, which projects `edit_only` to the baseline list.

## Limits of this oracle

- The probes run their agents as primaries through `opencode run --agent`, while agentspec emits `mode: subagent`. A subagent's session also carries the parent session's deny and `external_directory` rules, and the `todowrite` and `task` denies `deriveSubagentSessionPermission` adds unless the subagent's own ruleset names those permissions.
- It shows what OpenCode sends to a provider using the `@ai-sdk/openai-compatible` package with a model id containing no `gpt-`, so `apply_patch` is never offered. A different provider package could, in principle, filter tools differently.
- `websearch` and `question` never appear in this setup, with or without a map, so their absence says nothing about the deny-all.
- `edit_only` offering `write` is OpenCode's own coupling: the `edit`, `write`, and `apply_patch` tools all answer to one `edit` permission.
- `fx_server.py` offers no MCP resources, so `read_edit_bash`'s value says nothing about `list_mcp_resources`, `list_mcp_resource_templates`, and `read_mcp_resource`, which OpenCode checks against the `read` permission and so keeps for any agent allowed `read`.
- Title-generation requests are excluded by the marker, so this says nothing about the tools those carry.
- One OpenCode version per record. The key order the answer depends on is OpenCode's resolution order for `permission` keys, which a release can change.

## Related

- `TODO.md` #14 — OpenCode's resolved tool keys and the keys it honors diverge.
- `experiments/opencode-agent-tools-deny-all/` — the same deny-all under the deprecated `tools` map, MCP ids only.
- `experiments/opencode-agent-permission-external-read/` — what the deny-all does to a read outside the project, and what restating `external_directory` changes.
