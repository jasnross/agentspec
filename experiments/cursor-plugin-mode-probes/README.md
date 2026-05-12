# Cursor Plugin-Mode Empirical Probes

Two one-shot empirical tests against a real Cursor build, exploring the
plugin-tier surface that agentspec needs to emit into. Both feed into the
plugin sync mode idea
(`thoughts/ideas/2026-05-11-agentspec-plugin-sync-mode.md`).

## Status (as of 2026-05-11)

**Gate A is blocked.** Cursor has a current bug preventing plugin-tier hooks
from loading into the IDE at all:
https://forum.cursor.com/t/plugin-hooks-not-loading-into-cursor-ide/156702.
Until that's fixed (or a workaround is identified), the `sessionStart` hook
inside this probe-plugin won't fire and `${CURSOR_PLUGIN_DATA}` injection
cannot be verified. Implementation work against the documented schemas
proceeds in parallel; rerun Gate A once the bug is resolved.

**Gate B may still be partially runnable** since it observes Cursor's
manifest-acceptance behavior (UI surfacing, validation of unknown fields,
install success) without requiring hooks to fire. Confirm before assuming
— if `/plugins` UI also fails to load the plugin entry, Gate B is also
blocked.

Background: comparative schema research for both providers is captured in
`thoughts/research/2026-05-11-claude-cursor-plugin-manifests.md`. Key
research findings relevant to these gates:

- Cursor's `plugin.json` schema declares `additionalProperties: false`. The
  `unknown-field.json` variant should be rejected by any validator using
  the schema; whether Cursor's loader enforces it at runtime is what Gate
  B's unknown-field test is meant to measure.
- `${CURSOR_PLUGIN_DATA}` is not documented in any official Cursor source
  and is not used in any of the 11 official Cursor plugins. Gate A is the
  first-hand attestation we'd have if it works.
- `${CURSOR_PLUGIN_ROOT}` IS empirically attested (used by
  `cursor/plugins/continual-learning/hooks/hooks.json`). The hooks.json
  command in this probe uses it.

Pattern mirrors `experiments/cursor-output-gates/` and
`experiments/session-id-resume/`: small shell hook, plugin tree to install,
observe behavior in real Cursor, record results.

Unlike the prior experiments, this one ships a full plugin tree
(`probe-plugin/`) because plugin-tier env vars only inject when the hook is
registered through a `.cursor-plugin/` package — registering via the global
`~/.cursor/hooks.json` would defeat the test.

## Gate A — `${CURSOR_PLUGIN_DATA}` env var injection

**Question.** When a hook is registered inside a `.cursor-plugin/` package
and fires (`sessionStart` here), does Cursor inject `${CURSOR_PLUGIN_DATA}`
into the hook child process? If so, what path does it resolve to, and
does the directory actually exist on disk (or is it lazily created on first
reference)?

**Why it matters.** Research surfaced `${CURSOR_PLUGIN_DATA}` via a Cursor
maintainer forum post and a community env-var reference, but neither
documented the resolved path or confirmed runtime injection on a real
install. The downstream hook payload translation shim depends on this
directory existing for its `jq` auto-bootstrap pattern.

**Files.** `probe-plugin/.cursor-plugin/plugin.json`,
`probe-plugin/hooks/hooks.json`, `probe-plugin/hooks/scripts/env-capture.sh`.

## Gate B — `.cursor-plugin/plugin.json` field acceptance

**Question.** Empirical baseline already known: a Cursor plugin installs and
functions with no `plugin.json` at all (rsync to `~/.cursor/plugins/local/<name>/`).
What is the boundary of what Cursor accepts when a `plugin.json` IS present?
Specifically: which fields does Cursor recognize/surface in its UI, does it
reject unknown fields, and does any field affect the plugin's name as seen
by hook env vars or skill addressing?

**Why it matters.** agentspec's plugin sync mode needs to know which fields
are safe to emit and which (if any) are required for the plugin to be
recognized. Drives the per-provider plugin manifest config surface
(`plugin-name`, `plugin-version`, `plugin-description`, `plugin-author`).

**Files.** `manifest-variants/{empty,name-only,full,unknown-field}.json`.

## Procedure

### Setup (both gates)

1. **Decide install path.** The probe plugin is named `agentspec-probe`. Default
   install target: `~/.cursor/plugins/local/agentspec-probe/`. Adjust if your
   Cursor install uses a different plugin root.
2. **Edit `hooks.json` if needed.** The hook command uses
   `${CURSOR_PLUGIN_ROOT}/hooks/scripts/env-capture.sh`. If that substitution
   fails on your Cursor version, replace it with an absolute path to the
   script's location after rsync.
3. **rsync the plugin into place.**
   ```sh
   rsync -av --delete \
     /Users/jasonr/Workspace/jasnross/agentspec/experiments/cursor-plugin-mode-probes/probe-plugin/ \
     ~/.cursor/plugins/local/agentspec-probe/
   chmod +x ~/.cursor/plugins/local/agentspec-probe/hooks/scripts/env-capture.sh
   ```
4. **Quit and reopen Cursor entirely.** Plugin discovery happens at start.

### Gate A procedure (env var injection)

1. After setup, open a new Cursor conversation (any project). The
   `sessionStart` hook fires.
2. Inspect `$HOME/.cache/agentspec-plugin-probe.log`. Confirm the hook fired.
3. Look for `CURSOR_PLUGIN_DATA=...` in the captured environment section.
4. Note: (a) whether the variable is present, (b) the resolved path,
   (c) whether the directory exists on disk at that path (the script reports
   this explicitly).
5. Quit Cursor, reopen, start another conversation. Confirm the env var is
   stable across restarts (same path, dir still present).
6. Record results in this README under "Results / Gate A".

### Gate B procedure (manifest field acceptance)

For each of the four `manifest-variants/*.json` files:

1. Copy the variant over the plugin's manifest:
   ```sh
   cp /Users/jasonr/Workspace/jasnross/agentspec/experiments/cursor-plugin-mode-probes/manifest-variants/<variant>.json \
      ~/.cursor/plugins/local/agentspec-probe/.cursor-plugin/plugin.json
   ```
2. Quit and reopen Cursor.
3. Open Cursor's plugins UI (look for `/plugins` slash command or a settings
   view; this surface is still evolving — check the most recent Cursor docs).
4. Record: (a) is the plugin listed? (b) which fields from the manifest are
   surfaced in the UI (name, version, description, author/publisher)? (c) any
   error messages?
5. Trigger the `sessionStart` hook (open a new conversation) and confirm the
   hook still fires. Note any `CURSOR_*` env var differences in the log.
6. Record results below.

## Results

_(Fill in after running.)_

### Gate A — `${CURSOR_PLUGIN_DATA}` injection

- **Date:**
- **Cursor version:**
- **`CURSOR_PLUGIN_DATA` present in hook env?** (yes / no)
- **Resolved path:**
- **Directory exists on disk at that path?** (yes / no / created-on-write)
- **Path stable across Cursor restarts?** (yes / no)
- **Other `CURSOR_*` env vars observed (note any beyond what
  `cursor-howto/06-hooks/README.md` documents):**
- **Verdict:** (CURSOR_PLUGIN_DATA usable for jq auto-bootstrap /
  unusable / partially usable — describe)

### Gate B — `.cursor-plugin/plugin.json` field acceptance

For each variant:

#### `empty.json`

- **Date:**
- **Plugin listed in Cursor UI?** (yes / no / partial)
- **Fields surfaced:**
- **Hook still fires?** (yes / no)
- **Errors / warnings:**

#### `name-only.json`

- **Date:**
- **Plugin listed in Cursor UI?**
- **`name` field surfaced where?** (plugins list / hook env / other)
- **Hook still fires?**
- **Errors / warnings:**

#### `full.json`

- **Date:**
- **Plugin listed in Cursor UI?**
- **Fields surfaced in UI:** (name / version / description / author / publisher — note which)
- **Hook still fires?**
- **Errors / warnings:**

#### `unknown-field.json`

- **Date:**
- **Plugin listed in Cursor UI?** (yes / no — does Cursor reject strictly?)
- **Errors / warnings:**
- **Hook still fires?**
- **Verdict:** (Cursor permissive re: unknown fields / strict / silently drops unknown)

### Cross-cutting findings

- **Does any manifest field affect the plugin's discovered name** (e.g., does
  setting `name = "agentspec-probe-renamed"` change anything observable, or
  is the install-path basename authoritative)?
- **Implications for agentspec's Cursor adapter:** (capture briefly — this is
  what the plan-phase planner needs)
