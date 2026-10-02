# OpenCode — reading outside the project behind a deny-all `permission` map

**Question.** Under an OpenCode agent `permission` map that leads with `"*": "deny"` and allows `read`, does a read of a file outside the project raise a permission prompt when `external_directory: "ask"` follows the allow, and fail on the rule when it does not?

**Why it matters.** OpenCode's `*` matches every permission name, not only tool names. `external_directory` — which `read`, `glob`, `grep`, `edit`, `write`, `apply_patch`, and `lsp` ask for any path outside the project — is one of them, so behind a bare deny-all those tools fail on any such path with no prompt. agentspec's OpenCode adapter therefore restates `external_directory: "ask"` (and `doom_loop: "ask"`) after a restricted agent's allows. Agents that read `$THOUGHTS_DIR`, which sits outside any project, depend on that restatement turning the denial back into a prompt.

**Driver:** `unattended`. No human step, no credentials, no network, and no model quota: OpenCode talks only to a fake provider on loopback. **Depth:** `outbound-request`.

## Running it

```sh
experiments/opencode-agent-permission-external-read/probe.sh
PROBE_DRY_RUN=1 experiments/opencode-agent-permission-external-read/probe.sh   # evaluate without recording
```

## The arms

Each arm runs `opencode run --agent <agent> -m fake/m "hi"` in its own project, without `--auto`. The two agents are identical except for `permission`:

| Arm | Agent | `permission` map, in authored order |
| --- | --- | --- |
| `restated` | `probe-restated` | `"*": deny`, `read: allow`, `external_directory: ask`, `doom_loop: ask` |
| `bare` | `probe-bare` | `"*": deny`, `read: allow` |

Each arm writes `<arm>/outside/target.txt`, containing `AGENTSPEC-OUTSIDE-CONTENT-OCEXTR6`, beside its `project/` directory and outside it.

## The scripted call

`fixtures/fake_provider.py` is an OpenAI-compatible chat endpoint on `127.0.0.1`, declared in the project's `opencode.json` through `@ai-sdk/openai-compatible`. It writes every request body to the arm's `sink/` directory. Unlike the fake in `experiments/opencode-agent-permission-deny-all/`, it does not only answer `ok`: a request whose system message carries the marker and which holds no `role: "tool"` message gets one streamed `read` tool call, with `filePath` set to the arm's target. Every other request — the follow-up, and OpenCode's title generation — gets `ok`.

The call is scripted because the question is what OpenCode does when the agent reads outside the project, and a model that never calls a tool never makes OpenCode check a permission. The fake's reply is not the oracle; what OpenCode sends back is.

## The oracle

After OpenCode runs the scripted call, it sends a follow-up request carrying the call's result as a `role: "tool"` message. The projection reads that message from the marked requests and classifies it:

| Value | Tool result contains |
| --- | --- |
| `read` | the target's content: the read succeeded |
| `ask-rejected` | "The user rejected permission to use this specific tool call." |
| `rule-denied` | "The user has specified a rule which prevents you from using this specific tool call." |
| `no-tool-result` | no tool message at all |

The two error strings are the messages of OpenCode's `RejectedError` and `DeniedError` (`packages/core/src/v1/permission.ts` in `anomalyco/opencode` at `1ddb0873aee50d209d1a8d7f91b89c5daf692d49`). A rejection follows an ask; a denial follows a rule resolving to `deny`. Telling the two apart is what lets the probe observe a prompt without anyone answering it.

**Nothing is approved during the run.** On a `permission.asked` event, `opencode run` replies `reject` and prints `permission requested: <permission> (<patterns>); auto-rejecting` (`packages/opencode/src/cli/cmd/run.ts` L801–820); that line appears in `restated`'s output. A rejection normally ends the session, though: the session processor stops on a `RejectedError` unless `experimental.continue_loop_on_deny` is `true` (`packages/opencode/src/session/processor.ts` L200–201, L647, L694). The fixture's `opencode.json` sets it, so the session sends the follow-up request that carries the result. A `DeniedError` never stops the session, so `bare` needs no such setting.

Each arm's projection also carries the tools its marked requests offer. `["read"]` is non-default — OpenCode's default agent offers ten built-ins — and it shows the deny-all was in force when the read ran. Without it, `restated`'s `ask-rejected` would also be a reading of OpenCode's defaults, which ask for `external_directory` too.

**The outside-the-project check fires in a non-git project.** `containsPath` (`packages/opencode/src/project/instance-context.ts` L18–24) skips the worktree comparison when the worktree is `/`, which is what a non-git project gets, so a file outside the arm's `project/` directory raises `external_directory`. The runner fails an arm whose workspace sits inside a git repository, where OpenCode would compare against that worktree instead.

## Isolation

- Every `XDG_*` directory (`CONFIG`, `CACHE`, `DATA`, `STATE`) points into the arm's workspace, which keeps the operator's global `opencode.json`, plugins, and cache out of the run.
- `HOME` points into the arm's workspace too. OpenCode also reads `~/.opencode/` as a config directory, located from the home directory rather than any `XDG_*` variable, so an operator's `~/.opencode/` agents, plugins, or permission rules would otherwise reach every arm.
- Not covered: OpenCode's system-managed config directory and macOS managed preferences, which an administrator rather than the operator controls. Neither is set on a typical development machine.
- `OPENCODE_CONFIG`, `OPENCODE_CONFIG_DIR`, `OPENCODE_CONFIG_CONTENT`, and `OPENCODE_PERMISSION` are cleared. The `XDG_*` and `HOME` redirects do not block them, and each would inject config, or permission rules directly, from the operator's shell. `OPENCODE_DISABLE_PROJECT_CONFIG` is cleared as well: set, it would discard the fixture's own agents and fail every arm at the marker gate with no obvious cause.
- `OPENCODE_DISABLE_AUTOUPDATE=1` and `OPENCODE_DISABLE_MODELS_FETCH=1`, so the run touches no network.
- The fake binds loopback only, and runs one arm at a time. No MCP server is connected.
- The fake and the request dumps sit outside each arm's `project/` directory. The project's `opencode.json` still names the fake's port, as it must for OpenCode to reach it; the model is a fake that searches nothing, so naming it cannot change what is measured.

## The gates

A gate failure keeps the workspace and exits 1 without recording, because each describes an apparatus failure that would otherwise be recorded as `refuted`.

- **Git.** Each arm's workspace is outside any git repository (`git -C <arm> rev-parse` fails). Inside one, the read would be compared against that worktree, and the arm would measure a different check from the one traced above.
- **Marker.** Every arm holds at least one request carrying the marker in a system message. OpenCode warns rather than fails on an unknown `--agent` and runs its default agent instead, whose requests carry no marker.
- **Tool result.** Every arm holds a marked request carrying a `role: "tool"` message. Without one, either OpenCode never executed the scripted call, or the session stopped after the rejection — which is what a release that no longer honors `continue_loop_on_deny` would do — and the arm says nothing about permissions.

## The assertion is relational across arms

The two agents differ only in the restated rules, so distinct `external_read` values across arms show the projection reads each arm independently. No separate discriminator fixture is wired (`TODO.md` #24).

An arm with no marked request projects its `tools` to `"arm-had-no-governed-request"`; an arm whose tool result matches none of the strings projects `external_read` to `{unrecognized: [...]}`.

## The assertion discriminates

Measured 2026-10-02 against opencode **1.18.34**, from a `PROBE_DRY_RUN=1` run. Each arm sent three requests: the agent's, its follow-up, and the title generator's.

| Arm | `tools` | Tool result in the follow-up | `external_read` |
| --- | --- | --- | --- |
| `restated` | `read` | "The user rejected permission to use this specific tool call." | `ask-rejected` |
| `bare` | `read` | "The user has specified a rule which prevents you from using this specific tool call. Here are some of the relevant rules […]" | `rule-denied` |

`restated`'s `opencode run` output carried `permission requested: external_directory (<arm>/outside/*); auto-rejecting`; `bare`'s carried no such line. Against the same saved view, `record.sh --dry-run` printed `refuted` for a copy with `restated` replaced by `bare`'s requests.

## Limits of this oracle

- `doom_loop` is unmeasured. It rides the same `*`-then-restatement mechanism, but triggering it takes three identical tool calls in a row.
- Only `read` is exercised, though `glob`, `grep`, `edit`, `write`, and `lsp` raise the same `external_directory` ask.
- The probes run their agents as primaries through `opencode run --agent`, while agentspec emits `mode: subagent`. A subagent's session also carries the parent session's deny and `external_directory` rules, and the `todowrite` and `task` denies `deriveSubagentSessionPermission` adds unless the subagent's own ruleset names those permissions.
- It shows what OpenCode does behind a provider using the `@ai-sdk/openai-compatible` package with a model id containing no `gpt-`, so `apply_patch` is never offered.
- The target sits outside OpenCode's own `external_directory` allows — its tmp directory, its tool-output directory, and skill and reference directories — which OpenCode adds at runtime and an agent-level `external_directory` rule overrides. This package says nothing about paths inside them.
- One OpenCode version per record. The error strings and `continue_loop_on_deny` are both things a release can change; the tool-result gate catches the second.

## Related

- `TODO.md` #14 — OpenCode's resolved tool keys and the keys it honors diverge.
- `experiments/opencode-agent-permission-deny-all/` — the tools offered under the same deny-all-first map shape.
- `experiments/opencode-agent-tools-deny-all/` — the deny-all under the deprecated `tools` map, MCP ids only.
