# Cursor Output-Schema Empirical Gates

Two one-shot empirical tests against a real Cursor build. Both feed into the
hook payload translation shim plan
(`thoughts/ideas/2026-05-10-agentspec-hook-payload-translation-shim.md`,
Open Questions #19 and #21).

Pattern mirrors `experiments/session-id-resume/`: small shell hook,
JSON snippet to drop into `~/.cursor/hooks.json`, observe behavior in real
Cursor, record results.

## Gate #19 — Cursor exit-2 + JSON consumption

**Question.** When a `preToolUse` hook exits with code 2 AND emits JSON to
stdout (`{"permission": "deny", "user_message": ..., "agent_message": ...}`),
does Cursor consume the JSON fields or treat exit 2 as a short-circuit?

**Files.** `gate-19-exit2-deny.sh`, `cursor-hooks-snippet-19.json`.

**Why it matters.** Determines whether a Claude `exit 2 + reason` hook can
be losslessly translated to Cursor by emitting `permission: "deny"` JSON
alongside exit 2. If Cursor only honors one or the other, the wrapper
must pick a side per event.

## Gate #21 — Plain-stdout context injection on Cursor `sessionStart`

**Question.** If a Cursor `sessionStart` hook emits plain text to stdout
(no JSON envelope) and exits 0, does Cursor inject that text into the
agent's context — the way Claude does?

**Files.** `gate-21-plain-stdout-context.sh`, `cursor-hooks-snippet-21.json`.

**Why it matters.** The real-world hook usage survey found plain-stdout
context injection is the dominant Claude SessionStart pattern (~60% of
observed scripts). If Cursor accepts the same shape, the canonical
context-injection path simplifies materially — no JSON envelope, no
field-name asymmetry to bridge.

## Procedure (both gates)

1. Edit the `cursor-hooks-snippet-*.json` to replace `/ABSOLUTE/PATH/TO/...`
   with the real absolute path to the corresponding `.sh` script.
2. Drop the snippet contents into `~/.cursor/hooks.json` (or
   `<project>/.cursor/hooks.json`). Back up any existing file first.
3. Quit Cursor entirely and reopen — the `_agentspec_id` Phase 0 experiment
   confirmed Cursor reads `hooks.json` at start.
4. Follow the per-gate procedure documented inside each `.sh` script
   header.
5. Record results in this README under "Results" (date-stamped).
6. Restore the original `~/.cursor/hooks.json` when done.

## Results

_(Fill in after running. Both gates feed Phase 0.5 of the plan.)_

### Gate #19 — exit-2 + JSON consumption

- **Date:**
- **Cursor version:**
- **user_message marker visible in UI?** (yes / no / partial)
- **agent_message marker referenced by agent in next turn?** (yes / no)
- **Verdict:** (Cursor consumes JSON on exit 2 / partial / exit 2 short-circuits)

### Gate #21 — plain-stdout context injection

- **Date:**
- **Cursor version:**
- **Hook fired (per log file)?** (yes / no)
- **Agent quoted the marker phrase in its response?** (yes / no)
- **Verdict:** (plain stdout injects on Cursor / requires JSON envelope / hook didn't fire)
