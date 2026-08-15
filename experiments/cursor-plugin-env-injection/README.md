# Cursor — `${CURSOR_PLUGIN_DATA}` env var injection (Gate A)

**Question.** When a hook is registered inside a `.cursor-plugin/` package and fires, does Cursor inject `${CURSOR_PLUGIN_DATA}` into the hook child process? If so, what path does it resolve to, and does that directory exist on disk?

**Why it matters.** Research surfaced `${CURSOR_PLUGIN_DATA}` via a Cursor maintainer forum post and a community env-var reference, but neither documented the resolved path nor confirmed runtime injection on a real install. The hook payload translation shim's `jq` auto-bootstrap depends on that directory existing.

## This probe cannot run

**Blocked upstream.** Cursor has a bug preventing plugin-tier hooks from loading into the IDE at all: <https://forum.cursor.com/t/plugin-hooks-not-loading-into-cursor-ide/156702>. Until it is fixed, the `sessionStart` hook inside `probe-plugin/` will not fire and `${CURSOR_PLUGIN_DATA}` injection cannot be observed.

**This package therefore has no `probe.json`, no `probe.sh`, and no records.** That is the contract, not an omission:

- A manifest is written only once its assertion has been validated to discriminate. Nothing here has been validated, because nothing has run.
- `blocked` is not a status the harness can produce. A record describes a run that happened; there has been no run. `just probe-run` skips a package with no manifest, and `just probe-status` shows no row for a package with no records.
- A process note like this one belongs in a README, which is where you are reading it.

Whoever unblocks this authors the manifest, the runner, and the record together.

## What ships here

`probe-plugin/` is the apparatus and stays valid: a full plugin tree, because plugin-tier env vars only inject when the hook is registered through a `.cursor-plugin/` package. Registering through `~/.cursor/hooks.json` would defeat the test.

- `probe-plugin/.cursor-plugin/plugin.json` — the manifest that makes it a plugin
- `probe-plugin/hooks/hooks.json` — registers `sessionStart`, addressed through `${CURSOR_PLUGIN_ROOT}`
- `probe-plugin/hooks/scripts/env-capture.sh` — dumps the hook's environment

`${CURSOR_PLUGIN_ROOT}` **is** empirically attested — `cursor/plugins/continual-learning/hooks/hooks.json` uses it — which is why the hook command is addressed that way. If that substitution turns out not to resolve on your Cursor version, replace it with an absolute path to the script after the rsync; a hook whose command does not resolve simply never fires, and would be indistinguishable from the upstream bug this gate is blocked on.

`env-capture.sh` writes to `$HOME/.cache/agentspec-plugin-probe.log` and reports, for each of `${CURSOR_PLUGIN_DATA}` and `${CURSOR_PLUGIN_ROOT}`, whether the variable is set, what path it holds, and whether that directory exists on disk. Note that this predates the harness: a rewritten version would append stamped JSON to a workspace capture instead.

## Setup, for whoever picks this up

The plugin installs by rsync into `~/.cursor/plugins/local/agentspec-probe/`, then Cursor must be fully quit and reopened, since plugin discovery happens at start. Note that this is the one probe here whose apparatus is **not** confined to a temp workspace — a plugin has to be installed where Cursor looks for plugins. Remove it when done.

Adapting it to the current harness — a generated workspace, a templated capture hook, a manifest — is part of the work of unblocking it.
