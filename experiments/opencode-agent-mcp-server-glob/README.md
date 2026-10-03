# opencode-agent-mcp-server-glob

**Question:** Under an OpenCode agent `permission` map that leads with `"*": "deny"`, which tools does a `fx_*` allow offer when servers `fx` and `fx_extra` are both connected?

**Driver:** `unattended`. No human step, no credentials, no network, and no model quota: OpenCode talks only to a fake provider on loopback.

**Depth:** `outbound-request`. The oracle is the request body OpenCode sends to the provider, not `opencode debug agent`'s resolved view.

## Why it matters

This is Key Assumption 5 of `$THOUGHTS_DIR/designs/2026-10-03-agentspec-mcp-tool-grants.md`. OpenCode names an MCP tool `<server>_<tool>`, so the only permission key that grants a whole server is a wildcard over that prefix. OpenCode matches a permission key against a tool name as an anchored pattern in which `*` becomes `.*` (`packages/core/src/util/wildcard.ts` L3–13 at `1ddb087`, byte-identical to v1.18.34), so `fx_*` matches every tool whose name begins with `fx_` — not only the tools of server `fx`. The design has the OpenCode adapter emit `<server>_*` for a whole-server grant and report the over-match as a provider limitation.

- **If `server_glob` is offered all four tools**, the over-match is real: the adapter's whole-server grant also grants any tool whose name begins with the server's prefix, and the limitation the design reports is warranted.
- **If it is offered only `fx_alpha` and `fx_beta`**, OpenCode scopes the allow to the server, and the limitation is not needed.
- **If it is offered neither server's tools**, a wildcard key does not reach MCP tool ids at all, and the whole-server grant needs another spelling.

`fx_extra` stands in for any OpenCode tool whose name begins with `fx_`: a second server the spec never declared, or a custom tool file named that way. Which of those exists depends on the user's install, which is why the design treats the over-match as provider-wide rather than something agentspec can check.

## The arms

Each arm runs `opencode run --agent <agent> -m fake/m "hi"` in its own project. The two agents are identical except for `permission`:

| Arm | Agent | `permission` map, in authored order |
| --- | --- | --- |
| `baseline` | `probe-baseline` | none |
| `server_glob` | `probe-server-glob` | `"*": deny`, `fx_*: allow`, `external_directory: ask`, `doom_loop: ask` |

Every arm connects two local MCP stdio servers, both running `fixtures/fx_server.py` with its own stamp. They are configured under the keys `fx` and `fx_extra`, and OpenCode names tools by config key, so they offer `fx_alpha`, `fx_beta`, `fx_extra_alpha`, and `fx_extra_beta`.

## The oracle

`fixtures/fake_provider.py` is an OpenAI-compatible chat endpoint on `127.0.0.1`. The project's `opencode.json` declares it as a provider through `@ai-sdk/openai-compatible`, which is built into the OpenCode binary, and the fake writes every request body it receives to the arm's `sink/` directory. The tool list read from those bodies — `tools[].function.name` — is the set OpenCode actually offered the model.

The projection reads only requests whose `role: "system"` message carries the agent body's marker, `AGENTSPEC-PROBE-MARKER-OCGLOB2`, and keeps the full sorted tool list, so a built-in the glob unexpectedly let through would show. OpenCode also sends a title-generation request per run, with its own system prompt and `tools: []`; the marker keeps it out.

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
- **Two stamps, per arm.** Each server writes its own stamp (`fx.stamp`, `fx_extra.stamp`) when it answers `tools/list`. Without both, a `server_glob` lacking `fx_extra_*` would read as "the glob stopped at the server boundary" when `fx_extra` never connected.
- **Baseline tool listing.** `baseline`'s marked requests list all four of `fx_alpha`, `fx_beta`, `fx_extra_alpha`, and `fx_extra_beta`. The stamps prove both servers connected; this proves their tools reach a request, under the names the projection expects, when no map filters them.

`baseline` is not in the projection, because its built-in tool set varies by OpenCode version. It is the gate above instead, and it is what the expected value is non-default against.

## The assertion

An arm with no marked request projects to `"arm-had-no-governed-request"`, and one whose marked requests disagree projects to `{inconsistent: [...]}`. `expected` states the over-match OpenCode's source predicts: all four tools, and no built-in. No separate discriminator fixture is wired (`TODO.md` #24).

## Discriminate evidence

Before the first run, `record.sh --dry-run` against fabricated views printed `confirmed` for one matching `expected`, and `refuted` for one where `server_glob` carried only `fx`'s tools, one where it carried no tool, and one where a built-in leaked through beside the four:

```
record: dry run — confirmed — observed {"server_glob":["fx_alpha","fx_beta","fx_extra_alpha","fx_extra_beta"]} (no file written)
record: dry run — refuted — observed {"server_glob":["fx_alpha","fx_beta"]} (no file written)
record: dry run — refuted — observed {"server_glob":[]} (no file written)
record: dry run — refuted — observed {"server_glob":["fx_alpha","fx_beta","fx_extra_alpha","fx_extra_beta","read"]} (no file written)
```

From the `PROBE_DRY_RUN=1` run on 2026-10-03 against opencode **1.18.34**, each arm sent two requests, the agent's and the title generator's. The marked ones carried:

| Arm | `tools[].function.name`, sorted |
| --- | --- |
| `baseline` | `bash`, `edit`, `fx_alpha`, `fx_beta`, `fx_extra_alpha`, `fx_extra_beta`, `glob`, `grep`, `read`, `skill`, `task`, `todowrite`, `webfetch`, `write` |
| `server_glob` | `fx_alpha`, `fx_beta`, `fx_extra_alpha`, `fx_extra_beta` |

## What the record licenses

At `outbound-request`, this is evidence about what reached the model.

- **A `<server>_*` allow over-matches.** `server_glob` was offered `fx_extra`'s tools beside `fx`'s. The OpenCode adapter's whole-server grant also grants any tool whose name begins with `<server>_`, so the provider limitation the design reports is warranted.
- **It lets no built-in through.** The value carries only the four MCP tools; the deny-all still hides everything the glob does not match.

## Oracle limits

- **Primary agents only.** The agents run as primaries through `opencode run --agent`, while agentspec emits `mode: subagent`. OpenCode's source shows a subagent session adds only `todowrite` and `task` denies, plus the parent session's denies and `external_directory` rules (`agent/subagent-permissions.ts` L14–27); none of those touches an MCP tool id. That is a reading of source, not a measurement.
- **A second server, not a custom tool.** `fx_extra` is an MCP server. That a custom tool file named `fx_<something>` would match the same way follows from the wildcard being a plain name match, but is not measured here.
- **One provider package.** It shows what OpenCode sends to a provider using `@ai-sdk/openai-compatible`.
- **One OpenCode version per record.** The over-match depends on OpenCode's wildcard semantics, which a release can change.

## Related

- `experiments/opencode-agent-permission-deny-all/` — the deny-all with built-ins, a lone `edit`, or one MCP tool re-allowed.
- `experiments/opencode-agent-mcp-unrestricted/` — what an agent with no map, and one with only the deny-all, is offered.
