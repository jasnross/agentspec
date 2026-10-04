# opencode-agent-mcp-resource-read

**Question:** Under an OpenCode agent map that leads with `"*": "deny"` and sets `read` to `{"*": "allow", "mcp:*": "deny"}`, which tools is the agent offered, and does each MCP resource call fail on the rule while a project-file read still succeeds, where the same resource calls succeed under `read: allow`?

**Driver:** `unattended`. No human step, no credentials, no network, and no model quota: OpenCode talks only to a fake provider on loopback.

**Depth:** `outbound-request`. The oracle is the tool list and the tool result OpenCode sends to the provider, not `opencode debug agent`'s resolved view.

## Why it matters

This is Key Assumption 4 of `$THOUGHTS_DIR/designs/2026-10-03-agentspec-mcp-tool-grants.md`. OpenCode offers three MCP resource tools — `list_mcp_resources`, `list_mcp_resource_templates`, and `read_mcp_resource` — whenever a connected server advertises resources, and checks each against the `read` permission rather than against an MCP tool id. Each asks `read` with pattern `mcp:<server>:*` (the list tools) or `mcp:<server>:<uri>` (the read tool) (`packages/opencode/src/session/tools.ts` L172–185 and L343–348 at `1ddb087`, byte-identical to v1.18.34). So an agent agentspec grants `read` can read every connected server's resources, behind the adapter's `"*": "deny"`.

The design has the OpenCode adapter emit the `probe-pattern-map` shape for a spec declaring `read`: file reads allowed, `mcp:*` patterns denied, deny last because rules resolve last-match-wins in authored order.

- **If every resource call is `rule-denied` and the file read `succeeded`**, the pattern map closes the resource path without costing file reads, and the adapter can emit it.
- **If a resource call succeeds under the map**, the `mcp:*` pattern does not reach those calls, and the design needs another way to withhold resources.
- **If the file read fails under the map**, the map costs the spec the permission it declared.
- **If the resource tools are not offered under the map**, OpenCode hides them outright, and the design's reported limitation — offered but refused — overstates the exposure.

## The arms

Each arm runs `opencode run --agent <agent> -m fake/m "hi"` in its own project. The fake scripts one tool call per arm:

| Arm | Agent | Scripted call |
| --- | --- | --- |
| `map_read_resource` | `probe-pattern-map` | `read_mcp_resource` `{"server":"fx","uri":"fx://AGENTSPEC-RESOURCE-OCRES3"}` |
| `map_list_resources` | `probe-pattern-map` | `list_mcp_resources` `{"server":"fx"}` |
| `map_list_templates` | `probe-pattern-map` | `list_mcp_resource_templates` `{"server":"fx"}` |
| `map_read_file` | `probe-pattern-map` | `read` `{"filePath":"<arm>/project/notes.txt"}` |
| `plain_read_resource` | `probe-plain-read` | `read_mcp_resource` `{"server":"fx","uri":"fx://AGENTSPEC-RESOURCE-OCRES3"}` |
| `plain_list_resources` | `probe-plain-read` | `list_mcp_resources` `{"server":"fx"}` |
| `plain_list_templates` | `probe-plain-read` | `list_mcp_resource_templates` `{"server":"fx"}` |

The two agents differ only in `read`:

| Agent | `permission` map, in authored order |
| --- | --- |
| `probe-pattern-map` | `"*": deny`, `read: {"*": allow, "mcp:*": deny}`, `external_directory: ask`, `doom_loop: ask` |
| `probe-plain-read` | `"*": deny`, `read: allow`, `external_directory: ask`, `doom_loop: ask` |

The three `plain_*` arms are comparison arms, not a control: they are cells of the assertion like any other. They run a plain `read: allow` behind the same deny-all, and each pairs with a `map_*` arm making the same call. A rule denial reads the same whichever rule matched — OpenCode lists every rule for the `read` permission, the top-level `"*": deny` included — so only the pairing shows the `mcp:*` deny, rather than the deny-all, is what refused a call.

Every arm connects `fx`, a local MCP stdio server (`fixtures/fx_server.py`) that advertises the `resources` capability and serves one resource (`fx://AGENTSPEC-RESOURCE-OCRES3`) and one template. A successful read returns `AGENTSPEC-RESOURCE-CONTENT-OCRES3`. `fixtures/project/notes.txt` holds `AGENTSPEC-FILE-CONTENT-OCRES3`.

## The oracle

`fixtures/fake_provider.py` is an OpenAI-compatible chat endpoint on `127.0.0.1`, declared in the project's `opencode.json` through `@ai-sdk/openai-compatible`. It writes every request body it receives to the arm's `sink/` directory. A request carrying the marker and no tool result is answered with the arm's scripted call; every other request is answered "ok". The call is scripted because the question is what OpenCode does with a call, and a fake that only answers "ok" never makes OpenCode run a tool.

Two things are read from the marked requests, those whose `role: "system"` message carries `AGENTSPEC-PROBE-MARKER-OCRES3`:

- **`tools`** — the sorted `tools[].function.name` list, which is what OpenCode offered the model.
- **`call`** — the `role: "tool"` content OpenCode sends back in the follow-up request, classified with denials first:
  - none → `no-tool-result`
  - contains "The user has specified a rule" → `rule-denied`
  - contains "The user rejected permission" → `ask-rejected`
  - contains `AGENTSPEC-RESOURCE-CONTENT-OCRES3` (the read), `probe-doc` (the resource listing), `probe-template` (the template listing), or `AGENTSPEC-FILE-CONTENT-OCRES3` (the file read) → `succeeded`
  - otherwise `{unrecognized: [...]}`

  Each success token is something only a call that reached its target can return. The resource URI is not one: OpenCode's error for a failed MCP read, `Failed to read MCP resource: fx/<uri>`, repeats it, so a token drawn from the URI would classify a broken read as `succeeded`.

`experimental.continue_loop_on_deny: true` keeps the session going after a denial, so the follow-up carrying the tool result is sent. OpenCode's source shows a rule denial never stops the loop anyway (`packages/opencode/src/session/processor.ts` L200–202); the setting covers an ask rejection.

## Isolation

- Every `XDG_*` directory (`CONFIG`, `CACHE`, `DATA`, `STATE`) points into the arm's workspace, which keeps the operator's global `opencode.json`, plugins, and cache out of the run.
- `HOME` points into the arm's workspace too. OpenCode also reads `~/.opencode/` as a config directory, located from the home directory rather than any `XDG_*` variable.
- Not covered: OpenCode's system-managed config directory and macOS managed preferences, which an administrator rather than the operator controls.
- `OPENCODE_CONFIG`, `OPENCODE_CONFIG_DIR`, `OPENCODE_CONFIG_CONTENT`, and `OPENCODE_PERMISSION` are cleared, since each would inject config or permission rules from the operator's shell. `OPENCODE_DISABLE_PROJECT_CONFIG` is cleared as well: set, it would discard the fixture's own agents.
- `OPENCODE_DISABLE_AUTOUPDATE=1` and `OPENCODE_DISABLE_MODELS_FETCH=1`, so the run touches no network.
- No `--auto`: the non-interactive runner rejects every permission ask, so nothing is approved during the run.
- The fake binds loopback only, and runs one arm at a time.
- The fake, the MCP server, and the request dumps sit outside each arm's `project/` directory. The project's `opencode.json` still names the server's path and the fake's port, as it must for OpenCode to reach them; the model is a fake that searches nothing, so naming the paths cannot change what is measured.

## The gates

A gate failure keeps the workspace and exits 1 without recording, because each describes an apparatus failure that would otherwise be recorded as `refuted`.

- **Project file present.** `<arm>/project/notes.txt` exists, so a failed `map_read_file` cannot be a missing file.
- **Marker.** Every arm holds at least one request carrying the marker in a system message. OpenCode warns rather than fails on an unknown `--agent` and runs its default agent instead, whose requests carry no marker.
- **Stamp, per arm.** `fx_server.py` writes `<arm>/fx.stamp` when it answers `tools/list`. Without a connected server OpenCode offers no resource tool, and a scripted resource call fails as an unknown tool rather than on any permission.
- **Tool result.** Every arm's marked requests carry a `role: "tool"` message. Without one, OpenCode never executed the scripted call, or the session stopped after it.

## The physical project path

The `read` arm's path is the project directory's physical path (`pwd -P`). On macOS `$TMPDIR` sits under `/var`, a symlink to `/private/var`. The first dry run passed the unresolved spelling, and OpenCode asked `external_directory` for a file inside the project — `permission requested: external_directory (/var/folders/…/map_read_file/project/*); auto-rejecting` — so `map_read_file` projected to `ask-rejected`. That is a property of how OpenCode compares paths, not of the `read` map; the runner sidesteps it, and nothing here measures it.

## Discriminate evidence

Before the first run, `record.sh --dry-run` against two fabricated views printed `confirmed` for one matching `expected` and `refuted` for one where `map_read_resource.call` was `succeeded`. Against the current projection, three more printed `confirmed` for a matching view, `refuted` with `plain_read_resource.call` as `{unrecognized: ["Error: Failed to read MCP resource: fx/fx://AGENTSPEC-RESOURCE-OCRES3"]}` for one carrying OpenCode's read-failure text, and `refuted` for one where `plain_list_resources` was rule-denied:

```
record: dry run — confirmed — observed {"map_read_resource":{…,"call":"rule-denied"},…} (no file written)
record: dry run — refuted — observed {"map_read_resource":{…,"call":"succeeded"},…} (no file written)
record: dry run — refuted — observed {…,"plain_read_resource":{…,"call":{"unrecognized":["Error: Failed to read MCP resource: fx/fx://AGENTSPEC-RESOURCE-OCRES3"]}},…} (no file written)
record: dry run — refuted — observed {…,"plain_list_resources":{…,"call":"rule-denied"},…} (no file written)
```

The first live dry run, with the unresolved path, also discriminated: it printed `refuted` with `map_read_file.call` as `ask-rejected`, beside `rule-denied` and `succeeded` in the other arms — three distinct classes from one run.

From the second `PROBE_DRY_RUN=1` run on 2026-10-03 against opencode **1.18.34**, with the physical path, the marked follow-up requests carried these tool results:

| Arm | Tool result, excerpted |
| --- | --- |
| `map_read_resource`, `map_list_resources`, `map_list_templates` | `The user has specified a rule which prevents you from using this specific tool call. Here are some of the relevant rules [...]`, the rules quoting `mcp:*` |
| `map_read_file` | `<content>` holding `1: AGENTSPEC-FILE-CONTENT-OCRES3` |
| `plain_read_resource` | `Resource: fx://AGENTSPEC-RESOURCE-OCRES3`, `MIME: text/plain`, `AGENTSPEC-RESOURCE-CONTENT-OCRES3` |
| `plain_list_resources` | a `resources` list holding `probe-doc` at `fx://AGENTSPEC-RESOURCE-OCRES3` |
| `plain_list_templates` | a `resourceTemplates` list holding `probe-template` |

Two records exist from 2026-10-03, both `confirmed`. The first (`T132254`) has five arms and the URI-prefix success token; the second (`T134728`) has the two `plain_list_*` arms and the per-call success tokens above.

## What the record licenses

At `outbound-request`, this is evidence about what reached the model.

- **`read: allow` exposes MCP resources.** All three `plain_*` arms reached the server: the read returned the resource's content and both listings returned their entries.
- **The pattern map refuses every resource call.** Each of the three resource tools failed on a rule under `probe-pattern-map`, and succeeded under `read: allow`. The two maps differ only in `read`, so the `mcp:*` deny is what refused them.
- **It keeps file reads.** `map_read_file` read the project file under the same map.
- **The resource tools are still offered.** Every arm's `tools` lists all three resource tools beside `read`. The map refuses their calls but does not hide them, which is the offered-but-refused limitation the design has the OpenCode adapter report.
- **The deny-all still hides `fx`'s own tools.** No arm's `tools` holds `fx_alpha` or `fx_beta`.

## Oracle limits

- **Primary agents only.** The agents run as primaries through `opencode run --agent`, while agentspec emits `mode: subagent`. OpenCode's source shows a subagent session adds only `todowrite` and `task` denies, plus the parent session's denies and `external_directory` rules (`agent/subagent-permissions.ts` L14–27); none of those touches `read`. That is a reading of source, not a measurement.
- **Scripted calls, not a model's.** The fake makes each call with fixed arguments. A model could call a resource tool without `server`, which OpenCode's schema allows for the list tools; that path is unmeasured.
- **One server.** A pattern naming a server, such as `mcp:fx:*`, is not measured; the map denies every server.
- **The project file only.** A read outside the project is `experiments/opencode-agent-permission-external-read/`'s question.
- **One provider package and one OpenCode version per record.**

## Related

- `build_permission_map` in `src/adapters/opencode.rs` — writes the `probe-pattern-map` shape for an agent declaring `read`.
- `experiments/opencode-agent-permission-external-read/` — the scripted-call apparatus this package generalizes.
- `experiments/opencode-agent-permission-deny-all/` — the deny-all with built-ins, a lone `edit`, or one MCP tool re-allowed.
