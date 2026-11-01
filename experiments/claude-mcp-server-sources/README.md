# claude-mcp-server-sources

**Question:** Under `--setting-sources project`, which MCP server sources (user-scope servers in `~/.claude.json`, claude.ai connectors, a `--plugin-dir` plugin's server) does Claude Code load into a `claude -p` run with no server and into a plugin run, with and without `--strict-mcp-config`, and does `ENABLE_CLAUDEAI_MCP_SERVERS=false` keep connectors out of a plugin run while its server loads?

**Driver:** `unattended`. No model quota: every arm runs under a model id no catalog holds. The arms on the operator's login need that login, and an account with at least one claude.ai connector.

**Depth:** `resolved-config`. The oracle is the session Claude Code resolved, as its stream-json `init` event prints it, before any request reaches a model.

## Why it matters

The billed Claude family runs `claude -p --setting-sources project`, adding `--strict-mcp-config` only to arms whose server comes from an `--mcp-config` file. Anything else Claude Code loads into those runs is the operator's configuration: it sits in every request the probe measures, and can reach a committed sample. Three MCP sources could put it there: user-scope servers in `~/.claude.json`, the account's claude.ai connectors, and, in a plugin arm, the plugin's own server alongside them. A harness that must load none of the operator's configuration needs to know which flag or variable keeps each source out, and whether it also drops the server a plugin arm arranges.

- **If `--strict-mcp-config` keeps every source out of an arm with no server**, that flag is enough there.
- **If it also drops the plugin's server**, it cannot go on a plugin arm, which needs another way to keep connectors and user-scope servers out.
- **If `--setting-sources project` already keeps user-scope servers out**, no flag is needed for them in any arm the family runs.

## The arms

Every arm runs `claude -p --input-format stream-json --output-format stream-json --verbose --model agentspec-probe-unrecognized-model` in an empty project, with `--setting-sources project` unless the table says otherwise. The user message, `hi`, is sent after 8 s (see the gates).

| Arm | Login | `--plugin-dir` | `--strict-mcp-config` | Other |
| --- | --- | --- | --- | --- |
| `none` | operator's |  |  |  |
| `strict` | operator's |  | yes |  |
| `plugin` | operator's | yes |  |  |
| `plugin_strict` | operator's | yes | yes |  |
| `plugin_no_connectors` | operator's | yes |  | `ENABLE_CLAUDEAI_MCP_SERVERS=false` |
| `user_none` | relocated |  |  |  |
| `user_strict` | relocated |  | yes |  |
| `user_plugin` | relocated | yes |  |  |
| `user_plugin_strict` | relocated | yes | yes |  |
| `user_all_sources` | relocated |  |  | no `--setting-sources` |

The plugin is `fxp`, whose `.mcp.json` declares `fx`, the shared stub server (`experiments/lib/stubs/fx_server.py`).

A **relocated** arm runs with `HOME` at `<arm>/home`, whose `.claude.json` declares one user-scope server, `ux`, the same stub. The relocation is what lets the probe arrange a user-scope server without editing the operator's `~/.claude.json`. It costs the arm the operator's login, so a relocated arm runs on a placeholder `ANTHROPIC_API_KEY`, can load no claude.ai connector, and says nothing about one. The operator's arms say nothing about user-scope servers unless the operator has one. `user_all_sources` is the positive control for the relocation: with no `--setting-sources`, its `ux` must load, which shows the relocated `.claude.json` is read at all.

## The oracle

`init` lists every MCP server Claude Code loaded, each with a `source` and a `status`. The assertion yields, per arm, the distinct sources listed, sorted: `user`, `claudeai`, `plugin`, and `project` for a project-scope `.mcp.json`. A source missing from an arm's value is a source the arm did not load.

The servers' names are not read, so a sample keeps none of the operator's connector names. Each stub also writes `<arm>/fx.stamp` or `<arm>/user.stamp` when it answers `tools/list`; in the committed runs the stamps agree with `init` in every arm.

## The gates

A gate failure keeps the workspace and exits 1 without recording.

- **Unbilled.** Every arm's `init` names the unrecognized model, and its run ended in an error. An arm on the operator's login must have failed on that model, which shows the login held: a run that stopped on its login may not have loaded the account's connectors.
- **Settled.** No server in any arm's `init` is `pending`. Claude Code prints `init` when the first message arrives, listing only the servers registered by then, and the claude.ai connectors register asynchronously. Sent at once, the message produced an `init` listing them as `pending`, or not at all, in scratch runs on 2.1.306. Holding the message back 8 s left them `connected` in every run.
- **Connectors exist.** The `none` arm lists a claude.ai connector. Without one, every operator arm's missing `claudeai` would describe the account, not the arm.

## Isolation

`ENABLE_CLAUDEAI_MCP_SERVERS` and `CLAUDE_CODE_MCP_STARTUP_WAIT_MS` are cleared, except where an arm sets the first: an exported value would change which servers load, or how long they take to. `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` and `CLAUDE_CONFIG_DIR` are cleared for every arm by the family, so each arm runs on the login it declares.

The operator arms run in the operator's real home, under `--setting-sources project`, which leaves out the user settings that enable the operator's installed plugins; in the committed runs no operator arm without `--plugin-dir` listed a `plugin` source. A relocated arm reads none of the operator's files.

## Discriminate evidence

`samples/confirming.json` and `samples/refuting.json` were captured on 2026-11-01 against Claude Code **2.1.306**, from dry runs of the `assertion` and `discriminator` fixture variants. The `discriminator` variant adds a project-scope `.mcp.json`, declaring a server whose command fails, and a project `.claude/settings.json` approving it. The assertion yields:

- `confirming.json`: equal to `expected`.
- `refuting.json`: `project` beside every value of an arm without `--strict-mcp-config`, and `[]` in every arm with it.

`just probe-test` re-proves both results on every run.

## Findings

On 2.1.306:

- `--strict-mcp-config` keeps every source out, the plugin's own server included, as it did on 2.1.287 (measured 2026-10-03 with the same `init` oracle).
- `--setting-sources project` keeps user-scope servers out with or without the flag, and leaves a plugin's server loading. It does not keep connectors out.
- `ENABLE_CLAUDEAI_MCP_SERVERS=false` keeps connectors out of a plugin arm while its server loads.

So under `--setting-sources project`, the variable keeps every source of the operator's out of a plugin arm, and `--strict-mcp-config` or the variable does so for an arm with no server.

## History

`expected` states the values the first dry run measured, which match scratch runs made the same day while the probe was designed. No belief predates them: the 2.1.287 measurement covered only `--strict-mcp-config` on a plugin arm.
