# opencode-ancestor-instructions

**Question:** Under the probe harness's isolated home and XDG directories, does OpenCode put an `AGENTS.md` or a `CLAUDE.md` placed in a directory above the project into the agent's system prompt?

**Driver:** `unattended`. No human step, no credentials, no network, and no model quota: OpenCode talks only to a fake provider on loopback.

**Depth:** `outbound-request`. The oracle is the system prompt OpenCode sends to the provider.

## Why it matters

The fake-provider and `opencode debug` families isolate OpenCode by pointing `HOME` and every `XDG_*` directory into the arm's workspace (`OpenCodeIsolation` in `lib/probe_harness/families/opencode_env.py`). That keeps the operator's global `~/.config/opencode/AGENTS.md` and `~/.claude/CLAUDE.md` out of a run. It does not move the workspace itself, which sits under the operator's home at `~/.local/state/agentspec/probe-workspaces/`. If OpenCode's search for project instruction files walks up from the project, a file such as `~/AGENTS.md` or `~/CLAUDE.md` reaches every isolated run, and so does anything an operator keeps in a directory between their home and the workspace root.

- **If both arms carry their token**, isolation by `HOME` and `XDG_*` does not keep instruction files above the workspace out of an OpenCode run. A run that must load none of the operator's configuration needs another guard, such as a canary file above the project that the run must not carry.
- **If neither does**, OpenCode's search stops at the project, and the redirects are enough.

## The arms

Each arm runs `opencode run --agent probe-ancestor -m fake/m "hi"` in `<ws>/<arm>/project/`, with the same fixture tree. Before the run, the arrange step writes one instruction file into `<ws>/<arm>/`, the project's parent:

| Arm         | File in `<ws>/<arm>/` | Its token                          |
| ----------- | --------------------- | ---------------------------------- |
| `agents_md` | `AGENTS.md`           | `AGENTSPEC-ANCESTOR-AGENTS-OCANC1` |
| `claude_md` | `CLAUDE.md`           | `AGENTSPEC-ANCESTOR-CLAUDE-OCANC1` |

The two names are separate arms because OpenCode reads `CLAUDE.md` only where it finds no `AGENTS.md`, which the discriminator below measures; a single arm holding both would say nothing about `CLAUDE.md`.

The arrange step refuses a workspace inside a git repository. OpenCode bounds a project's search by its git worktree, and a non-git project's worktree is `/`; a workspace root under `$HOME` and outside any repository is the case an operator's `~/AGENTS.md` would meet.

## The oracle

`experiments/lib/stubs/fake_provider.py` writes every request body OpenCode sends to the arm's `sink/`. The assertion reads only requests whose `role: "system"` message carries the agent body's marker, `AGENTSPEC-PROBE-MARKER-OCANC1`, which keeps OpenCode's title-generation request out. For each arm it yields whether those requests carry the arm's file line, `Instruction canary: <token>`, in a system message: one bool when every governed request agrees, `{"inconsistent": [...]}` when they differ, and `"arm-had-no-governed-request"` when there is none.

OpenCode introduces each file it loads with `Instructions from: <path>`, but the assertion matches the file's own line rather than that header, so a reworded header cannot turn a loaded file into a `false`.

## Isolation

Each arm runs under `OpenCodeIsolation`, as every fake-provider probe does: `HOME` and every `XDG_*` directory point into the arm's workspace, the variables that inject or discard config are cleared, and auto-update, the models fetch and the external skill scan are off. See `experiments/opencode-agent-mcp-unrestricted/README.md` for each one's reason.

The measurement is one directory above the project. The search's bound is the worktree, `/` here, which is a reading of OpenCode's behavior rather than a measurement of every level: no arm places a file further up, since a file above `<ws>/<arm>/` would sit outside the arm's own directory, in the workspace root every arm shares or above it.

## The gates

- **Marker.** Every arm holds at least one request carrying the marker in a system message. OpenCode warns rather than fails on an unknown `--agent` and runs its default agent instead.
- **Git.** The arrange step refuses a workspace inside a git repository, as described above.

## Discriminate evidence

`samples/confirming.json` and `samples/refuting.json` were captured on 2026-11-01 against opencode **1.18.34**, from dry runs of the `assertion` and `discriminator` fixture variants. The `discriminator` variant adds a project-level `AGENTS.md`. OpenCode then reads that file and the ancestor `AGENTS.md` in `agents_md`, and skips the ancestor `CLAUDE.md` in `claude_md`. The assertion yields:

- `confirming.json`: `{"agents_md":true,"claude_md":true}`, equal to `expected`.
- `refuting.json`: `{"agents_md":true,"claude_md":false}`.

`just probe-test` re-proves both results on every run.

## History

`expected` was first stated as `{"agents_md":false,"claude_md":false}`, the belief that OpenCode's search stops at the project, before any record existed. The first dry run, on 2026-11-01 against 1.18.34, carried both tokens, so `expected` states the measured value.

## Version

The committed records were taken on opencode **1.18.34**, run as `mise exec opencode@1.18.34 -- experiments/probe run opencode-ancestor-instructions`. OpenCode 2.x has not been measured (agentspec `TODO.md` #55).
