1. Consider deriving id from path instead of requiring in frontmatter
   - Currently `id` is a required `String` in all frontmatter structs; missing it causes a parse error at load time
   - `id` is used for two things: duplicate detection in validation, and output filename/dirname construction in every adapter
   - The file path already encodes a natural identity: the stem for agents and rules (`agents/foo.md` → `foo`), the directory name for skills (`skills/foo/` → `foo`)
   - Deriving id from path would eliminate redundant frontmatter, prevent path/id mismatches, and make the path the single source of truth
   - The open question is whether callers ever legitimately want the output filename to differ from the source filename; if so, `id` (or a rename field) remains useful as an override
2. Allow specifying path to configuration file
3. Cleanup: Refactor methods with clippy::too_many_lines
4. Establish a canonical shell-tool concept and apply it consistently across all surfaces (agent `tools:` field AND hook matchers)
   - `ToolFrontmatter::Bash` currently maps to `Bash` in Claude but Claude supports both `PowerShell` and `Bash`
   - Expanding the `shell` canonical tool to both `Bash` and `PowerShell` for Claude would help cover both
   - **Hook matchers are affected by the same gap:** `HookFrontmatter::matcher` (`src/spec.rs:162`) is currently a pass-through `Option<String>` cloned verbatim to both providers (`src/adapters/hook_compile.rs:131`; emitted at `cursor.rs:181` and `claude.rs:279`). A user writing `matcher = "Bash"` in `hooks.toml` gets `Bash` emitted to Claude (correct) and Cursor (silently wrong — Cursor's matcher for the shell tool is `Shell`, empirically confirmed against Cursor 3.2.21). The hook fires under Claude and never fires under Cursor, with no error message to the user.
   - The same canonical-tool concept that fixes the `tools:` field divergence would naturally fix this: adapters map canonical `shell` → `Bash` (Claude) / `Shell` (Cursor) at emit time for both `tools:` and `matcher`. Strings that aren't canonical tool names (e.g., regex-style values like `Bash|Read|Edit`) must pass through unchanged.
   - Surfaced during Phase 0 verification of `thoughts/ideas/2026-05-10-agentspec-hook-payload-translation-shim.md`
5. Support executing skills in forked subagents
   - Claude supports running skills in forked subprocesses via frontmatter fields
   - OpenCode has a similar concept for command execution
   - Currently neither adapter emits the relevant frontmatter to enable this
6. Consider fanning out `ToolFrontmatter::Subagent` to the full Claude subagent toolkit
   - `ToolFrontmatter::Tasks` fans out to 6 `ClaudeTool` variants (`TaskCreate`, `TaskGet`, `TaskList`, `TaskUpdate`, `TaskStop`, `TodoWrite`) but `Subagent` maps to just `ClaudeTool::Agent`
   - Claude's subagent workflow also involves `SendMessage` (resume a subagent); spec authors declaring `tools: [subagent]` likely expect the full toolkit
   - Adding `SendMessage` requires a new `ClaudeTool` variant and deciding the complete subagent fan-out (potentially also `ToolSearch`, `Monitor`)
   - Out of scope for the tool-mapping-fixes plan; captured for a follow-up
7. Orphan cleanup for merged JSON entries when a sync target is removed
   - Sentinel-based ownership in `hooks_merge` only cleans up `_agentspec_id` entries when agentspec runs the merge for that provider
   - If a user removes `[sync.claude]` from `agentspec.toml` (or deletes all hook specs and the sync target), the next `sync` skips Claude entirely — previously-injected entries persist in `settings.json` as permanent orphans
   - The manifest layer handles the equivalent for files via per-destination `.agentspec-manifest.json` cleanup, but `settings.json` isn't manifest-tracked
   - Likely shape: an `agentspec sync --prune` (or `unmerge`/`uninstall`) subcommand that walks all candidate `settings.json` / `hooks.json` paths and strips agentspec-tagged entries
   - Out of scope for the hooks-pipeline branch; revisit before 1.0
8. Cursor empirical-verification gate for hook environment
   - Several Cursor behaviors are inferred from research docs and not verified against a real Cursor build:
     - Event-name mappings (`postToolUseFailure`, `sessionStart`, `sessionEnd`, `subagentStart`, `subagentStop`, `beforeSubmitPrompt`) — see `cursor.rs:215-220`
     - `${CLAUDE_PROJECT_DIR}` resolution outside plugin scope in `MergedProject` mode — see `compile.rs:128-130`
     - ~~`${CLAUDE_PLUGIN_ROOT}` aliasing in Bundled (Path) mode~~ — RESOLVED by plugin-sync-mode plan (2026-05-11): the bundled-mode emission now uses `${CURSOR_PLUGIN_ROOT}` for Cursor and `${CLAUDE_PLUGIN_ROOT}` for Claude (per-adapter constant threaded through `hook_command_anchor`), eliminating the aliasing assumption.
     - Whether Cursor accepts unknown sub-fields like `_agentspec_id` in hook entries (the merged-mode contingent design depends on this)
   - All four are blockers for confident Cursor support — verify before 1.0 against a real Cursor build and document the actual behavior
   - If Cursor doesn't alias `CLAUDE_PLUGIN_ROOT` in Bundled mode, the fix is likely to mirror the Merged-mode pattern: emit `CLAUDE_PLUGIN_ROOT=<derived> <command>` for Cursor specifically
9. Verbose parity for `agentspec remove`
   - `emit_sync(plan, dry_run, verbose)` threads `--verbose` into `render_sync_report` to show unchanged destinations and the `Unchanged` column; `emit_remove(plan, dry_run)` does not accept a `verbose` parameter at all
   - `RemoveArgs.common.verbose` is silently ignored — the typestate refactor made the asymmetry visible by giving `emit_remove` its own signature, but did not change the underlying behavior (the prior unified `emit()` only routed `verbose` into `render_sync_report`)
   - Decide what `verbose` means on the remove path: show per-file deletions? show destinations that had no manifest (currently silently skipped)? emit the same line shape `render_remove_report` already produces but unconditionally regardless of activity?
   - Likely shape: add `verbose: bool` to `emit_remove`'s signature, thread into a new branch in `render_remove_report` that surfaces destinations where `Ok(None)` from `remove_manifest_tracked` was returned (today these are invisible)
   - Surfaced by code review of the FileWrite typestate refactor; out of scope for that branch
10. Support sharing spec content (helper scripts, fragments) across multiple agentspec spec directories
    - Intra-`spec/` sharing via symlinks is now supported: the load phase follows symlinks within `sources_dir`, resolves them to target content, and emits regular files in compiled output. The remaining scope is cross-`spec/`-directory sharing only.
    - Use case: a user has two separate spec directories (e.g., work and personal) that share common hook scripts or `{% include %}` fragment files; today they must duplicate these across directories
    - Two candidate approaches: (a) agentspec follows symlinks during the load/walk phase, resolving them to their real content and copying that content into synced output — no config change required, but symlink semantics may be surprising across OS/VCS boundaries; (b) a new `[spec].extra_dirs` (or similar) config key lists additional root directories whose contents are merged into the spec search paths at load time, making sharing explicit and portable
    - Open questions: does sharing apply only to fragment/supporting files, or also to full agent/skill/rule/hook specs? How do id collisions resolve when the same spec id appears in two roots? Does the manifest track provenance (which root a file came from) so `remove` knows which entries are local vs. shared?
11. Hoist sync-time validations to config-load time
    - The "sync mode is 'path' but no `dir` configured for provider 'X'" error currently fires inside `resolve_dest_dir` (`src/sync.rs`) — well after config has been loaded and validated. By that point we've already accepted the malformed config, started the sync pipeline, and only surface the misconfiguration when we go to resolve a destination
    - Audit other validations of the same shape (cross-field constraints on `[sync.<provider>]` blocks, `mode`/`dir` interactions, presence-required combinations) and move them into `validate::validate_semantics` (or the config-load-time equivalent) so misconfigurations surface from `agentspec validate` rather than mid-pipeline
    - The defense-in-depth check at the use site can stay (or be downgraded to a `debug_assert`/`unreachable!`) once the load-time gate is in place
    - Surfaced by Pass 3 of the adapter-trait-api-consolidation review; see also TODO #15
12. Add a project rule about hoisting validations to config-validation time
    - Add a new file under `.claude/rules/` (e.g. `validation-locality.md`) prescribing: when a config-shape constraint can be checked at load time, do that check at load time, not at the use site. Use-site checks are defense-in-depth; the user-facing error is the load-time one
    - Pattern: cross-field constraints on parsed config structs belong in `validate::validate_semantics`; per-field constraints (types, required-vs-optional) belong in serde frontmatter parsing. Use-site checks should be `debug_assert`/`unreachable!` for the cases the load-time gate already covered
    - Cross-link to TODO #14 as a current example. Other follow-ons may surface from auditing existing validations
13. Consider splitting `ConfigPatch` into `ForwardPatch` + `ReversePatch` traits
    - Today every `ConfigPatch` impl uses `unreachable!()` for the direction it doesn't support: forward patches (Claude/Cursor `*HooksPatch`, OpenCode `*InstructionsPatch`) `unreachable!()` their `run_remove`, reverse patches (`*RemoveHooksPatch`, `*RemoveInstructionsPatch`) `unreachable!()` their `run`
    - The orchestrator already routes correctly (sync_plan drains forward patches, remove_plan constructs reverse patches), so `unreachable!()` is documented dead code rather than a live bug
    - A trait split would (a) move the routing invariant into the type system: `Vec<Box<dyn ForwardPatch>>` for sync, `Vec<Box<dyn ReversePatch>>` for remove, no inverse-direction methods to dispatch wrong, (b) eliminate ~12 lines of `unreachable!()` per pair across three providers, and (c) shrink each trait surface from 2 methods to 1
    - Cost: `AdapterOutput.patches` and `RemovalOutput.patches` become two distinct types; sync_plan/remove_plan signatures change; existing patches' construction-site code is fine but the boxing types need the new trait names
    - Surfaced by Pass 3 of the adapter-trait-api-consolidation review. Defer until the next adapter-API touch lands; not blocking for the current branch
14. Consider how to support provider-specific specs in general (hooks and beyond)
    - The hook payload translation shim (`thoughts/ideas/2026-05-10-agentspec-hook-payload-translation-shim.md`) intentionally ships v1 with no per-hook canonicalization opt-out — every emitted hook command points at the wrapper, every wrapper canonicalizes. This keeps v1 scope bounded but leaves provider-specific hooks (Claude's `effort.level` reads, Cursor's `subagent_model` reads, anything depending on `defer` / `followup_message` / `WorktreeCreate` plain-stdout / `failClosed`) reachable only through the canonical `provider_raw` escape hatch — usable for inputs, but provider-only output features remain out of reach
    - Same gap exists in spirit for other spec kinds: a Claude-only agent or Cursor-only skill has no first-class way to declare "only emit me for this provider" today; the workaround is per-provider sync targets which is coarse-grained
    - Likely shape: a per-spec `provider_specific = "claude" | "cursor" | "opencode"` (or list) frontmatter field that compile/sync respects across all spec kinds. For hooks, `provider_specific` would also bypass wrapper generation. For agents/skills/rules, it gates emission
    - Open questions: cross-provider sync-target validation (load-time error vs. silent skip when a spec targets a provider that isn't an active sync target); manifest interaction (does the spec count toward provider-N's owned-files set even when not emitted?); interaction with prefixing/AdapterConfig; whether the field name should be singular (`provider_specific`) or list (`providers`) for the multi-provider case
    - Out of scope for the hook-payload-translation-shim branch; revisit when one of: (a) someone hits the missing escape hatch in practice, (b) a provider-only spec kind comes up that can't be expressed as a hook
15. Support `email` in `plugin-author` for plugin-mode manifest emission
    - Today (v1 of plugin mode, plan `thoughts/plans/2026-05-11-agentspec-plugin-sync-mode.md`) `plugin-author` is `Option<String>` in TOML; both Claude and Cursor manifests serialize it as `{ name: <string> }` with no `email` support
    - Both providers' schemas accept `{ name, email? }`; the v1 limit is a scope decision, not a provider constraint
    - Likely shape: accept either a bare string or an inline table `plugin-author = { name = "...", email = "..." }` in TOML; serde untagged enum on the field. Both adapter manifests pass the structure through as-is (still one shared internal struct)
    - Out of scope for the initial plugin-mode plan; revisit when a user asks or when a multi-author shape (`authors = [...]`) comes up alongside it
16. Auto-install `jq` for plugin-mode hooks via the plugin persistent-data directory
    - v1 of the canonical hook payload shim (plan `thoughts/plans/2026-05-10-agentspec-hook-payload-translation-shim.md`) treats `jq` as an external prerequisite — if it isn't installed on the user's machine, the shim prints a clear error and exits non-zero. This works but pushes a small one-time install onto plugin end users
    - Both providers already expose a writable, persistent-across-updates per-plugin directory: Claude Code's `${CLAUDE_PLUGIN_DATA}` (`~/.claude/plugins/data/{id}/`, documented at https://code.claude.com/docs/en/plugins-reference#persistent-data-directory) and Cursor's `${CURSOR_PLUGIN_DATA}` (path undocumented; injected only into plugin hooks). Both substrates landed before this TODO was written
    - Likely shape: a `SessionStart` shim that follows the Claude Code docs' manifest-diff-and-reinstall pattern (compare a bundled `jq.sha256` manifest against a copy in the data directory; download + verify on mismatch). One bootstrap script per provider, parameterized by platform/arch via `uname -sm`. SHA256s are stamped into the generated bootstrap script by agentspec at sync time, sourced from upstream jq release attestations
    - Platform handling: linux-amd64, linux-arm64, darwin-amd64, darwin-arm64. macOS downloads must strip the `com.apple.quarantine` xattr (`xattr -d com.apple.quarantine "$JQ"`) before exec — without this, Gatekeeper silently fails the exec
    - Failure-mode design: if the download fails (offline, captive portal, firewall), the bootstrap exits non-zero with a clear stderr message pointing at the manual `brew install jq` / `apt install jq` fallback. Subsequent shim invocations in the same session see the missing `jq` and fail loudly per v1 behavior
    - Reference predecessor work: #9 (provider-payload translation shim — the parent idea, partially landed by the v1 plan above)
