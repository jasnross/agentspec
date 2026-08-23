1. Consider deriving id from path instead of requiring in frontmatter
   - Currently `id` is a required `String` in all frontmatter structs; missing it causes a parse error at load time
   - `id` is used for two things: duplicate detection in validation, and output filename/dirname construction in every adapter
   - The file path already encodes a natural identity: the stem for agents and rules (`agents/foo.md` → `foo`), the directory name for skills (`skills/foo/` → `foo`)
   - Deriving id from path would eliminate redundant frontmatter, prevent path/id mismatches, and make the path the single source of truth
   - The open question is whether callers ever legitimately want the output filename to differ from the source filename; if so, `id` (or a rename field) remains useful as an override
2. Cleanup: Refactor methods with clippy::too_many_lines
3. Support executing skills in forked subagents
   - Claude supports running skills in forked subprocesses via frontmatter fields
   - OpenCode has a similar concept for command execution
   - Currently neither adapter emits the relevant frontmatter to enable this
4. Orphan cleanup for merged JSON entries when a sync target is removed
   - Sentinel-based ownership in `hooks_merge` only cleans up `_agentspec_id` entries when agentspec runs the merge for that provider
   - If a user removes `[sync.claude]` from `agentspec.toml` (or deletes all hook specs and the sync target), the next `sync` skips Claude entirely — previously-injected entries persist in `settings.json` as permanent orphans
   - The manifest layer handles the equivalent for files via per-destination `.agentspec-manifest.json` cleanup, but `settings.json` isn't manifest-tracked
   - Likely shape: an `agentspec sync --prune` (or `unmerge`/`uninstall`) subcommand that walks all candidate `settings.json` / `hooks.json` paths and strips agentspec-tagged entries
   - Out of scope for the hooks-pipeline branch; revisit before 1.0
5. Support sharing spec content (helper scripts, fragments) across multiple agentspec spec directories
   - Intra-`spec/` sharing via symlinks is now supported: the load phase follows symlinks within `sources_dir`, resolves them to target content, and emits regular files in compiled output. The remaining scope is cross-`spec/`-directory sharing only.
   - Use case: a user has two separate spec directories (e.g., work and personal) that share common hook scripts or `{% include %}` fragment files; today they must duplicate these across directories
   - Two candidate approaches: (a) agentspec follows symlinks during the load/walk phase, resolving them to their real content and copying that content into synced output — no config change required, but symlink semantics may be surprising across OS/VCS boundaries; (b) a new `[spec].extra_dirs` (or similar) config key lists additional root directories whose contents are merged into the spec search paths at load time, making sharing explicit and portable
   - Open questions: does sharing apply only to fragment/supporting files, or also to full agent/skill/rule/hook specs? How do id collisions resolve when the same spec id appears in two roots? Does the manifest track provenance (which root a file came from) so `remove` knows which entries are local vs. shared?
   - Cost: `AdapterOutput.patches` and `RemovalOutput.patches` become two distinct types; sync_plan/remove_plan signatures change; existing patches' construction-site code is fine but the boxing types need the new trait names
   - Surfaced by Pass 3 of the adapter-trait-api-consolidation review. Defer until the next adapter-API touch lands; not blocking for the current branch
6. Consider how to support provider-specific specs in general (hooks and beyond)
   - The hook payload translation shim (`thoughts/ideas/2026-05-10-agentspec-hook-payload-translation-shim.md`) intentionally ships v1 with no per-hook canonicalization opt-out — every emitted hook command points at the wrapper, every wrapper canonicalizes. This keeps v1 scope bounded but leaves provider-specific hooks (Claude's `effort.level` reads, Cursor's `subagent_model` reads, anything depending on `defer` / `followup_message` / `WorktreeCreate` plain-stdout / `failClosed`) reachable only through the canonical `provider_raw` escape hatch — usable for inputs, but provider-only output features remain out of reach
   - Same gap exists in spirit for other spec kinds: a Claude-only agent or Cursor-only skill has no first-class way to declare "only emit me for this provider" today; the workaround is per-provider sync targets which is coarse-grained
   - Likely shape: a per-spec `provider_specific = "claude" | "cursor" | "opencode"` (or list) frontmatter field that compile/sync respects across all spec kinds. For hooks, `provider_specific` would also bypass wrapper generation. For agents/skills/rules, it gates emission
   - Open questions: cross-provider sync-target validation (load-time error vs. silent skip when a spec targets a provider that isn't an active sync target); manifest interaction (does the spec count toward provider-N's owned-files set even when not emitted?); interaction with prefixing/AdapterConfig; whether the field name should be singular (`provider_specific`) or list (`providers`) for the multi-provider case
   - Out of scope for the hook-payload-translation-shim branch; revisit when one of: (a) someone hits the missing escape hatch in practice, (b) a provider-only spec kind comes up that can't be expressed as a hook
7. Auto-install `jq` for plugin-mode hooks via the plugin persistent-data directory
   - v1 of the canonical hook payload shim (plan `thoughts/plans/2026-05-10-agentspec-hook-payload-translation-shim.md`) treats `jq` as an external prerequisite — if it isn't installed on the user's machine, the shim prints a clear error and exits non-zero. This works but pushes a small one-time install onto plugin end users
   - Both providers already expose a writable, persistent-across-updates per-plugin directory: Claude Code's `${CLAUDE_PLUGIN_DATA}` (`~/.claude/plugins/data/{id}/`, documented at https://code.claude.com/docs/en/plugins-reference#persistent-data-directory) and Cursor's `${CURSOR_PLUGIN_DATA}` (path undocumented; injected only into plugin hooks). Both substrates landed before this TODO was written
   - Likely shape: a `SessionStart` shim that follows the Claude Code docs' manifest-diff-and-reinstall pattern (compare a bundled `jq.sha256` manifest against a copy in the data directory; download + verify on mismatch). One bootstrap script per provider, parameterized by platform/arch via `uname -sm`. SHA256s are stamped into the generated bootstrap script by agentspec at sync time, sourced from upstream jq release attestations
   - Platform handling: linux-amd64, linux-arm64, darwin-amd64, darwin-arm64. macOS downloads must strip the `com.apple.quarantine` xattr (`xattr -d com.apple.quarantine "$JQ"`) before exec — without this, Gatekeeper silently fails the exec
   - Failure-mode design: if the download fails (offline, captive portal, firewall), the bootstrap exits non-zero with a clear stderr message pointing at the manual `brew install jq` / `apt install jq` fallback. Subsequent shim invocations in the same session see the missing `jq` and fail loudly per v1 behavior
8. Support compiling a single spec (or subset) and emitting expanded output to stdout
   - Currently `agentspec compile` always runs the full pipeline across all specs and writes the entire `generated/` directory; there is no way to compile one spec in isolation or emit to stdout
   - Use case: the `review-spec` skill in `agentconfig` needs to feed compiled (fragment-expanded) spec content to a reviewer agent — today the workaround is a full `agentspec compile` followed by reading the relevant file from `generated/`, which is heavier than necessary
   - Likely shape: `agentspec compile --spec spec/skills/foo/ --stdout` (or similar) that runs the pipeline for a single spec and prints the compiled output to stdout instead of writing files. Provider selection (`--provider claude`) would be needed since output format varies per adapter
   - Open questions: should this support multiple specs in one invocation (`--spec a --spec b`)? Should it emit raw expanded markdown or the full adapter-formatted output (with frontmatter transformations)? How does this interact with fragments that reference sibling specs (e.g., cross-spec `{% include %}`)?
9. Validate template-to-template inheritance chains, not just spec-to-template
   - `validate_child_blocks` currently runs only for specs (via `resolve_fragments`), not for templates themselves
   - A mid-level template with a typo in a block override against its parent won't be caught until a spec that transitively extends it hits a MiniJinja render error
   - Likely shape: walk `templates/` at validation time and run `validate_child_blocks` for each template that uses `{% extends %}`
10. Consider caching template reads between validation and rendering
    - `validate_child_blocks` reads template files from disk to walk the parent chain; MiniJinja's loader reads the same files again during render
    - Negligible for small template sets but duplicated I/O worth awareness as template usage grows
11. _Done — see `$THOUGHTS_DIR/designs/2026-08-20-agentspec-claude-effort-probe-packages.md`._ Slot kept to preserve incoming `TODO #11` references.
12. _Done — see `$THOUGHTS_DIR/designs/2026-08-22-agentspec-adapter-originated-degradation-warnings.md`._ Slot kept to preserve incoming `TODO #12` references.
13. Cursor subagent identity in canonical hook input may be wrong for `subagentStart`/`subagentStop`
    - `from_cursor` (`src/hooks_canonical.rs:330-347`) reconstructs canonical `session_id`/`agent_id` on the documented assumption that "Cursor renews `conversation_id` per subagent and carries the parent link as `parent_conversation_id`". Captured payloads falsify that for the two subagent lifecycle events
    - Observed on every `subagentStart` and `subagentStop` (Cursor 3.15.19 / 3.16.17): `conversation_id`, `parent_conversation_id`, and `session_id` all hold the **same** value — the parent conversation. The child identity is in `subagent_id` (e.g. `tool_13007703-…`), which agentspec ignores entirely
    - Consequence: `parent.is_some()` is true, so line 343 sets `agent_id = conversation_id`. Canonical `agent_id` therefore equals `session_id` and identifies the _parent_. A hook subscribing to `subagent_start`/`subagent_stop` through agentspec receives no usable subagent identity
    - **Scope is not yet established.** A subagent-scoped `preToolUse` was never captured — the probe's tool calls all fired at parent scope — and that is the case both the code comment and the fixture at `src/hooks_canonical.rs:575-586` actually model. The reconstruction may well be correct for tool events fired _inside_ a subagent. Do not generalize from the two lifecycle events
    - Next step is a probe, not a fix: force a Cursor subagent to actually execute a tool and capture the resulting `preToolUse` payload. Only then is it clear whether this is a two-event special case or a wrong model of Cursor's id semantics
    - Related fixture-realism gap, lower priority: the hand-written Cursor fixtures in `src/hooks_canonical.rs` omit fields real payloads carry — `session_id` (present as its own key, equal to `conversation_id` in all 92 observed samples), `generation_id`, `model`, `user_email`, and `transcript_path` (non-null in 58 of 93 payloads; `from_cursor` does read it, so this is fixture realism rather than a translation defect)
    - Evidence: the raw payload logs live in the probe scratchpad and should be attached to any issue filed from this. `experiments/cursor-subagent-effort/` is the closest runnable apparatus, but **it registers `subagentStart` alone** and deliberately says so — the probe this item needs must register more. The original capture used four events, and that registration exists nowhere else now, so it is recorded here:

      ```json
      {
        "version": 1,
        "hooks": {
          "beforeSubmitPrompt": [
            { "type": "command", "command": "{{CAPTURE_SCRIPT}}" }
          ],
          "subagentStart": [
            { "type": "command", "command": "{{CAPTURE_SCRIPT}}" }
          ],
          "subagentStop": [
            { "type": "command", "command": "{{CAPTURE_SCRIPT}}" }
          ],
          "preToolUse": [{ "type": "command", "command": "{{CAPTURE_SCRIPT}}" }]
        }
      }
      ```

      `preToolUse` is the one this item turns on: the open question is whether a tool call fired _inside_ a subagent carries the child identity, and the original probe never captured one because its subagent made no tool calls. A new package would copy `cursor-subagent-effort`'s capture hook, widen the registration to the above, and force the subagent to actually run a tool
14. Verify agentspec's OpenCode tool-id mapping against the provider's resolved tool set
    - `build_tool_map` (`src/adapters/opencode.rs:601`) emits twelve ids for every OpenCode agent: `bash, edit, glob, grep, question, read, skill, task, todowrite, webfetch, websearch, write`. opencode 1.18.15 resolves a different set: `apply_patch, bash, glob, grep, invalid, question, read, skill, task, todowrite, webfetch`
    - Probed with `opencode debug agent` against a no-`tools` baseline, using non-default values (a key set to its default value is invisible in the resolved map — the same methodological trap recorded as a rule in `experiments/README.md`):
      - `edit: false` → resolved `apply_patch: false`. **Honored — `edit` aliases to `apply_patch`**
      - `write: false` → resolved `apply_patch: false`. **Honored — `write` also aliases to `apply_patch`**
      - `apply_patch: false` → resolved `apply_patch: **true**`. **Not honored** — the id OpenCode _reports_ is not an id it _accepts_
      - `websearch: false` → no effect; `websearch` never appears in the resolved map
    - **The trap worth recording is the third row.** A contributor comparing agentspec's emitted ids against `opencode debug agent` output would reasonably "modernize" `edit`/`write` into `apply_patch` — and silently break tool denial for every OpenCode agent, with no error from either tool. The current names are correct precisely because they are the alias form
    - Open question is narrow: is `websearch` a stale id, or is it absent from the resolved map only because no web-search provider is configured in the probe environment? Until that is answered it is unclear whether a spec granting `websearch` grants anything. Worth one probe with a configured provider
    - Adjacent observation, not yet investigated: the resolved agent record also carries a top-level `permission` array (`{permission, action, pattern}` entries — `doom_loop`, `external_directory`, and a `*` default). agentspec emits nothing for it. Whether any spec-level capability should map there rather than to `tools` is unexamined
    - Scope note: this is the _agent_ tool surface. It is unrelated to TODO #13 (Cursor subagent ids) and to the skill-frontmatter defect, where `tools` is discarded outright because OpenCode's skill schema has no tools surface at all
15. _Done — see `$THOUGHTS_DIR/plans/2026-08-21-agentspec-probe-driver-vocabulary-and-claude-effort-packages.md`._ Slot kept to preserve incoming `TODO #15` references.
16. Give the orphaned OpenCode findings probe packages
    - Commit `54c2160` recorded six OpenCode measurements in `docs/provider-verification.md`. The probe harness converted two into `experiments/opencode-agent-variant/` (the sibling-key assertion and the `#`-suffix discriminator) and left four with no package. When `a6efa06` deleted the document, those four stopped existing as records
      - Skill surface: `model:` and `variant:` are parsed and absent from the resolved skill record
      - Skill surface: with `tools:` added, the resolved record is exactly `{content, description, location, name}` — all three discarded
      - Command surface: `variant:` **is** accepted (`"model": "anthropic/claude-sonnet-4-5", "variant": "high"`)
      - Skills resolve under both `.opencode/skill/` and `.opencode/skills/`
    - The first two survive only as one clause in the "absent from that list" note under "The workflow" in `experiments/README.md` and as a scope note on TODO #14. The last two survive nowhere in the repository
    - **One of them is a live defect.** `src/adapters/opencode.rs` declares `tools` as a non-`Option` map on the skill frontmatter struct, so every generated OpenCode skill file carries a `tools:` block OpenCode parses and discards. `model`/`variant` appear only when a preset applies, so `tools` is the one that ships on every skill
    - The plan's "no historical result is backfilled" rule justified itself by saying old findings become `expected` values in manifests. That only holds where a manifest was written; for these four none was, so the justification never applied and the loss went unnoticed
    - Re-measure rather than restore from git history: a 2026-08-15 observation against opencode 1.18.15 written back as prose is a backfill under another name. The oracle is `opencode debug skill` — script-driven, no live session, no credentials, seconds to run
    - This is the first real exercise of the negative-finding shape: assert on a positive signal about the discard (the resolved record's key set), and use the agent surface as the discriminator, since it is the surface where the same frontmatter is honored
    - Package boundaries are a planning question under the one-package-one-question invariant — the skill surface may be one package or several, and the command surface is separate
    - Design reference: `$THOUGHTS_DIR/designs/2026-08-15-agentspec-provider-verification-harness.md`, sections "Recording a negative finding" and "Remaining Work"
17. Probe whether Cursor injects `${CURSOR_PLUGIN_DATA}` into plugin-tier hooks
    - **Question.** When a hook registered inside a `.cursor-plugin/` package fires, does Cursor inject `${CURSOR_PLUGIN_DATA}` into the hook child process? What path does it resolve to, and does that directory exist?
    - **Why it matters.** The hook payload translation shim's `jq` auto-bootstrap depends on that directory existing. The variable is attested by a Cursor maintainer forum post and a community env-var reference, but neither documents the resolved path nor confirms runtime injection.
    - **Blocked upstream.** Cursor has a bug preventing plugin-tier hooks from loading into the IDE at all: <https://forum.cursor.com/t/plugin-hooks-not-loading-into-cursor-ide/156702>. Until it is fixed the `sessionStart` hook cannot fire and injection cannot be observed.
    - Apparatus was built and is recoverable: `git checkout probe-apparatus-cursor-plugin-gates -- experiments/cursor-plugin-env-injection` restores the tree (the tag pins `d71d6cc`, so the recovery point survives a squash-merge). Read its `README.md` first — it is the load-bearing part, carrying the empirical `${CURSOR_PLUGIN_ROOT}` attestation and the warning that an unresolvable hook command is indistinguishable from the upstream bug. It was written against Cursor 3.2-era behavior, so check it still makes sense before reusing it.
    - It has no `experiments/` package because a package is a measurement. Whoever unblocks this authors the manifest, the assertion, and the runner together.
18. Probe which `plugin.json` fields Cursor recognizes and surfaces
    - **Question.** What is the boundary of what Cursor accepts when a `.cursor-plugin/plugin.json` is present? Which fields does it recognize and surface in its UI, does it reject unknown fields, and does any field affect the plugin's name as seen by hook env vars or skill addressing?
    - **Why it matters.** agentspec's plugin sync mode needs to know which fields are safe to emit and which, if any, are required for the plugin to be recognized. It drives the per-provider plugin manifest config surface — `plugin-name`, `plugin-version`, `plugin-description`, `plugin-author`.
    - The baseline is known empirically: a Cursor plugin installs and functions with **no `plugin.json` at all** (rsync to `~/.cursor/plugins/local/<name>/`). This probe maps the edges around that.
    - **Unattempted, not blocked.** Unlike TODO #17 this does not require hooks to fire, so it may well run today. It becomes blocked only if the `/plugins` UI also fails to load the plugin entry — establish that first.
    - Apparatus was built and is recoverable: `git checkout probe-apparatus-cursor-plugin-gates -- experiments/cursor-plugin-manifest-fields` restores the tree (the tag pins `d71d6cc`, so the recovery point survives a squash-merge). It holds `manifest-variants/`, but its `README.md` is the load-bearing part — the `additionalProperties: false` schema-strictness context, the distinction between a schema declaring strictness and a loader enforcing it, the note that the oracle is human-judged so the assertion must be an option set, and the four-variant procedure. Check it still matches current Cursor before reusing it.
    - It has no `experiments/` package because a package is a measurement. Whoever attempts this authors the manifest, the assertion, and the runner together.
19. _Done — see `$THOUGHTS_DIR/designs/2026-08-20-agentspec-claude-effort-probe-packages.md`._ Slot kept to preserve incoming `TODO #19` references.
20. _Done — see `$THOUGHTS_DIR/designs/2026-08-20-agentspec-claude-effort-probe-packages.md`._ Slot kept to preserve incoming `TODO #20` references.
21. Probe whether a mid-session typed slash command applies a skill's `effort:`
    - **Question.** When a user types `/<skill-name>` into an already-running interactive Claude Code session, does the skill's `effort:` frontmatter reach the outbound model request it governs?
    - **Why it matters.** It is the third path on the skill surface and the common interactive case. `experiments/claude-skill-effort/` measures the two adjacent cells — a skill Claude selects itself mid-session (`inline`, measured **inert**) and a slash command supplied as the session's `-p` entry prompt (`slash_entry`, measured **honored**) — and this cell sits between them, so neither result settles it.
    - **The current belief is instruction text, not a measurement.** Claude Code's own harness text instructs the model to invoke a typed `/<skill-name>` **through the Skill tool**, which would make it mechanically the `inline` path and therefore inert. That is exactly the kind of belief this repository exists to stop people designing against, and it cuts in favor of measuring: nothing recorded in `claude-skill-effort` needs correcting if this cell later measures honored.
    - **`driver: manual`, and a separate package.** Reaching the cell needs a multi-turn interactive session; `claude -p` is one turn, so `claude-skill-effort`'s apparatus cannot be extended to cover it. The oracle is the same — `CLAUDE_CODE_ENABLE_TELEMETRY=1` plus `OTEL_LOG_RAW_API_BODIES=file:<dir>` — but the invocation is a person at a terminal.
    - Read `experiments/claude-skill-effort/README.md` first: its three-path table, its `.messages` definition of "governed" (a skill's body never reaches `.system`), and its relational assertion all carry over.
22. Report the `--agent` effort result upstream to Claude Code
    - **What was measured.** `experiments/claude-agent-effort/` records, at depth `outbound-request` against Claude Code 2.1.232, that an agent file's `effort:` frontmatter reaches `.output_config.effort` when the agent is delegated to via the Task tool, and does **not** when the same file runs as the session under `--agent`.
    - **Why that is worth reporting.** Claude Code documents `effort` as a subagent frontmatter field applying "when this subagent is active", and separately documents that an agent file runs both as a delegated subagent and as the main session under `--agent`. Taken together those say the field should apply on both paths. It measures inert on the second.
    - **Cite the record, not the prose.** `experiments/claude-agent-effort/results/` holds the committed measurement and the README holds the apparatus, the three gates, and the relational assertion. That record is the evidence for whichever this turns out to be — a bug in the `--agent` path, or a documentation page that does not say the field is path-scoped.
    - **State the confound rather than overclaiming.** The fixture pins `effortLevel: medium` at the project tier, so the finding cannot separate "the `--agent` path drops frontmatter effort" from "a project-tier `effortLevel` outranks frontmatter on that path". Separating them needs an arm with no `effortLevel` set at all, which the package does not have; the README records this under Residual gaps.
    - The skill surface has a comparable inert cell (a model-invoked skill, `experiments/claude-skill-effort/`), but it is not the same report: there the mechanism is visible and arguably correct — the skill body arrives as a tool result on a request whose effort was already settled — rather than contradicting a documented claim.
23. Decide how `surface_compile_diagnostics` should handle a subjectless `CountedSubjects` degradation
    - `src/main.rs` renders a `Presentation::CountedSubjects` group as `{provider}: skipped {n} {word}` with `n = group.len()`, then lists subjects under `--verbose` via `filter_map(Degradation::subject)`, which drops entries whose `subject` is `None`
    - The two agree only because every `HooksUnsupported` push goes through `Degradation::for_spec`. That invariant lives in a doc comment, not in the types — a `Degradation::provider_wide(_, HooksUnsupported)` push would print a count line with no listing beneath it, and its `message()` (whose `HooksUnsupported` arm has no caller today) would never render either
    - The mirror-image gap: a `Presentation::Warning` group renders only `head.message()`, so if a warning-kind degradation were ever pushed via `for_spec`, every non-head entry in the group is silently discarded
    - Options weighed and deferred: derive `n` from the collected subjects (agrees by construction, but a stray push then renders `skipped 0 hooks`); fall back to `message()` when the subject list is empty (coherent in both directions, and gives the `HooksUnsupported` arm a real caller); or encode the constraint structurally so `CountedSubjects` kinds cannot be built without a subject
    - Raised in review of `$THOUGHTS_DIR/plans/2026-08-22-agentspec-adapter-originated-degradation-warnings.md` Phase 2; documented rather than fixed there because the invariant holds for every push site that exists
