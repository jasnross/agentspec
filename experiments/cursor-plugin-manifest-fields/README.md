# Cursor — `.cursor-plugin/plugin.json` field acceptance (Gate B)

**Question.** What is the boundary of what Cursor accepts when a `plugin.json` is present? Which fields does it recognize and surface in its UI, does it reject unknown fields, and does any field affect the plugin's name as seen by hook env vars or skill addressing?

The baseline is already known empirically: a Cursor plugin installs and functions with **no `plugin.json` at all** (rsync to `~/.cursor/plugins/local/<name>/`). This probe maps the edges around that.

**Why it matters.** agentspec's plugin sync mode needs to know which fields are safe to emit and which, if any, are required for the plugin to be recognized. It drives the per-provider plugin manifest config surface — `plugin-name`, `plugin-version`, `plugin-description`, `plugin-author`.

## Runnability is unconfirmed

Gate B observes Cursor's _manifest-acceptance_ behavior — UI surfacing, validation of unknown fields, install success — and so does **not** require hooks to fire. That is what distinguishes it from `cursor-plugin-env-injection`, which is definitively blocked by an upstream bug.

**But it may be blocked too.** If Cursor's `/plugins` UI also fails to load the plugin entry, there is nothing to observe. Confirm that before assuming this is runnable.

**This package therefore has no `probe.json`, no `probe.sh`, and no records** — nothing has been validated to discriminate, because nothing has run. Whoever settles the runnability question authors the manifest and the runner together.

Note that the answer would be human-judged: "which fields are surfaced in the UI" has no machine-readable oracle, so the assertion would be an option set. Deciding which outcomes are distinguishable _before_ running is the projection-discriminates rule applied to a human oracle.

## What ships here

- `manifest-variants/{empty,name-only,full,unknown-field}.json` — the four manifests to install in turn
- `probe-plugin/` — its own copy of the plugin tree, not a reference to Gate A's. A package never depends on another package; a fixture two probes both need is copied.

## Known schema context

Cursor's `plugin.json` schema declares `additionalProperties: false`, so `unknown-field.json` should be rejected by any validator using the schema. **Whether Cursor's loader enforces that at runtime is exactly what this probe would measure** — a schema declaring strictness and a loader enforcing it are different claims, and only the second is observable.

## Procedure sketch

For each variant: copy it over `probe-plugin/.cursor-plugin/plugin.json`, rsync the plugin into `~/.cursor/plugins/local/agentspec-probe/`, fully quit and reopen Cursor, then record whether the plugin is listed, which fields are surfaced, and any errors. Remove the installed plugin when done.
