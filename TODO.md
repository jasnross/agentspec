1. Consider deriving id from path instead of requiring in frontmatter
   - Currently `id` is a required `String` in all frontmatter structs; missing it causes a parse error at load time
   - `id` is used for two things: duplicate detection in validation, and output filename/dirname construction in every adapter
   - The file path already encodes a natural identity: the stem for agents and rules (`agents/foo.md` → `foo`), the directory name for skills (`skills/foo/` → `foo`)
   - Deriving id from path would eliminate redundant frontmatter, prevent path/id mismatches, and make the path the single source of truth
   - The open question is whether callers ever legitimately want the output filename to differ from the source filename; if so, `id` (or a rename field) remains useful as an override
2. Do we actually need the normalized types?
   - `NormalizedSpec` / `NormalizedAgentSpec` / etc. are structurally identical to their raw counterparts — normalization is a field-by-field pass-through with no defaults, no coercions, and no validation applied
   - The only material difference is that normalized frontmatter structs derive `Clone`, which compile.rs needs to clone specs across multiple provider adapters in a single loop
   - The simplest fix: add `#[derive(Clone)]` to the raw frontmatter structs and delete the entire `Normalized*` type family, `normalize_specs()`, and the normalization stage from the pipeline
   - The typestate stages (Specs → NormalizedSpecs → ValidatedSpecs) would collapse to Specs → ValidatedSpecs with no loss of safety
   - Retain this layer only if we expect real normalization work to arrive (e.g., applying preset defaults into spec fields before compilation)
3. Allow specifying path to configuration file
4. Cleanup: Refactor methods with clippy::too_many_lines
5. Extract an adapter trait
   - Adapters currently share a common shape: `adapt_*` (compile), `post_write_hook` (sync), same parameter patterns (`Option<&AdapterConfig>`, `&ProviderPresetsMap`)
   - A formal trait would make adding a new provider a matter of implementing one trait rather than knowing which functions to export and how `compile_specs`/`sync_plan` dispatch to them
   - All adapters now share the same interface: `adapt_*` (compile) and `post_write_hook` (sync) — ready for trait extraction
6. Consider merging `bash` and `powershell` under a single `shell` canonical tool
   - `ToolFrontmatter::Bash` currently maps to `Bash` in Claude but Claude supports both `PowerShell` and `Bash`
   - Expanding the `shell` canonical tool to both `Bash` and `PowerShell` for Claude would help cover both
7. Separate `FileWrite` into typed variants for `CleanSlate` vs `ManifestTracked`
   - `FileWrite` uses `kind: Option<FileKind>` where `None` means `CleanSlate` (compile) and `Some` means `ManifestTracked` (sync) — this is a runtime invariant enforced via `anyhow::Context`
   - Separate structs (or an enum with per-variant fields) would make this compile-time safe
   - Would also let `emit()` accept sync-specific params (like `verbose`) only for the sync path, rather than threading them through the shared signature
8. Support executing skills in forked subagents
   - Claude supports running skills in forked subprocesses via frontmatter fields
   - OpenCode has a similar concept for command execution
   - Currently neither adapter emits the relevant frontmatter to enable this
9. Consider fanning out `ToolFrontmatter::Subagent` to the full Claude subagent toolkit
   - `ToolFrontmatter::Tasks` fans out to 6 `ClaudeTool` variants (`TaskCreate`, `TaskGet`, `TaskList`, `TaskUpdate`, `TaskStop`, `TodoWrite`) but `Subagent` maps to just `ClaudeTool::Agent`
   - Claude's subagent workflow also involves `SendMessage` (resume a subagent); spec authors declaring `tools: [subagent]` likely expect the full toolkit
   - Adding `SendMessage` requires a new `ClaudeTool` variant and deciding the complete subagent fan-out (potentially also `ToolSearch`, `Monitor`)
   - Out of scope for the tool-mapping-fixes plan; captured for a follow-up
10. Orphan cleanup for merged JSON entries when a sync target is removed
    - Sentinel-based ownership in `hooks_merge` only cleans up `_agentspec_id` entries when agentspec runs the merge for that provider
    - If a user removes `[sync.claude]` from `agentspec.toml` (or deletes all hook specs and the sync target), the next `sync` skips Claude entirely — previously-injected entries persist in `settings.json` as permanent orphans
    - The manifest layer handles the equivalent for files via per-destination `.agentspec-manifest.json` cleanup, but `settings.json` isn't manifest-tracked
    - Likely shape: an `agentspec sync --prune` (or `unmerge`/`uninstall`) subcommand that walks all candidate `settings.json` / `hooks.json` paths and strips agentspec-tagged entries
    - Out of scope for the hooks-pipeline branch; revisit before 1.0
11. Cursor empirical-verification gate for hook environment
    - Several Cursor behaviors are inferred from research docs and not verified against a real Cursor build:
      - Event-name mappings (`postToolUseFailure`, `sessionStart`, `sessionEnd`, `subagentStart`, `subagentStop`, `beforeSubmitPrompt`) — see `cursor.rs:215-220`
      - `${CLAUDE_PROJECT_DIR}` resolution outside plugin scope in `MergedProject` mode — see `compile.rs:128-130`
      - `${CLAUDE_PLUGIN_ROOT}` aliasing in Bundled (Path) mode — Cursor's documented hook env only mentions `CLAUDE_PROJECT_DIR`, so the bundled-mode command literal `${CLAUDE_PLUGIN_ROOT}/hooks/scripts/<f>` may not resolve at runtime
      - Whether Cursor accepts unknown sub-fields like `_agentspec_id` in hook entries (Phase 2's contingent design depends on this)
    - All four are blockers for confident Cursor support — verify before 1.0 against a real Cursor build and document the actual behavior
    - If Cursor doesn't alias `CLAUDE_PLUGIN_ROOT` in Bundled mode, the fix is likely to mirror the Merged-mode pattern: emit `CLAUDE_PLUGIN_ROOT=<derived> <command>` for Cursor specifically
12. Preserve verbatim file modes for all `SupportingFile` emission
    - Today both `compile.rs::build_hook_script_files` and `adapters/claude.rs::adapt_skill_spec` (and the analogous Cursor path) emit `mode = Some(0o755)` only when the source file is executable, and `None` otherwise — meaning non-executable helpers fall back to the system default (typically 0o644)
    - A user who deliberately set 0o600 on a secrets-style helper, or 0o400 on a read-only fixture, loses that intent in transit
    - Fix is symmetric across hook scripts and skill `supporting_files`: pass through `metadata.permissions().mode()` verbatim instead of collapsing to the executable-bit
    - Out of scope for the hooks-pipeline branch; tackle as a single cross-cutting change touching `build_hook_script_files`, both skill-adapter paths, and any future `SupportingFile`-emitting helpers
13. Move provider-specific JSON-merge knowledge into adapters
    - Today `src/hooks_merge.rs` hard-codes both providers' `hooks.json`/`settings.json` shape: Claude's matcher-group wrappers (`event → [{ matcher, hooks: [entries] }]`) and Cursor's flat-array layout (`event → [entries]`)
    - This contradicts the CLAUDE.md design principle that adapters own all provider-specific content decisions; `hooks_merge` should be a generic CST-aware sentinel-based merger, with the per-provider shape (entry-to-CST conversion, owned-entry removal/pruning rules) supplied by the adapter
    - Likely shape: a `HookMergeStrategy` trait (or set of closures) per adapter that knows (a) where in the document the per-event arrays live, (b) how to convert an `EmittedHookEntry` to a CST node for that provider, and (c) what nesting wrapper (if any) needs pruning when emptied
    - Would also unblock surfacing the empty-event-array behavior choice in adapters: today Cursor lacks wrappers structurally, so the question of "prune empty event keys" is the same question for both providers but the answer must be encoded once per adapter
    - Out of scope for the hooks-pipeline branch; revisit as part of the broader Adapter-trait extraction (TODO #5)
14. Provider-payload translation shim for hook scripts
    - Cross-provider field-name divergence in hook stdin payloads: `tool_name`/`input.tool`, `tool_use_id`/`callID`, `session_id`/`conversation_id`. Tool-result types diverge on `PostToolUse` (object-or-string vs JSON-encoded string). `user_prompt_submit` renames payload fields under Cursor's `beforeSubmitPrompt`. (See `thoughts/ideas/.done/2026-05-04-agentspec-provider-agnostic-hooks.md`, sections "Findings from follow-up research" and "Open Questions" #1.)
    - Today agentspec's value prop assumes a hook script written against one schema works across providers. The assumption breaks the moment a script reads any field beyond the lowest common denominator.
    - Likely shape: each adapter generates a per-event wrapper that reads the provider's stdin, re-shapes it to a canonical agentspec schema, and execs the user's script. The wrapper lives alongside the user's script (e.g., `hooks/scripts/_agentspec/preToolUse.sh`); the emitted command in `hooks.json`/`settings.json` points at the wrapper. Per-event because the schema is event-specific.
    - Adapters own the provider→canonical mapping; `compile.rs`/`sync.rs` stay provider-agnostic. Pairs with TODO #5 (Adapter trait) and TODO #13 (provider-specific JSON-merge into adapters) — the same architectural shift.
    - Sub-questions for planning:
      - Canonical schema per event (one struct per `HookEvent`).
      - Passthrough policy: drop provider-only fields, or surface under a `_provider_extras` namespace?
      - Latency: stdin parse+serialize on every hook invocation; benchmark for high-frequency events (`pre_tool_use` on every Bash call).
      - Env var injection symmetry: the wrapper is the natural place to normalize env (e.g., synthesize `CLAUDE_PROJECT_DIR` if a provider doesn't set it).
    - References: idea doc at `thoughts/ideas/.done/2026-05-04-agentspec-provider-agnostic-hooks.md`; companion research at `thoughts/research/2026-05-04-hook-event-cross-provider-translation.md`.
    - Out of scope for the hooks-pipeline branch; revisit alongside TODO #5/#13 as part of post-1.0 cross-provider ergonomics.
15. Allow specifying multiple events for a single hook in `hooks.toml`
    - Today `HookFrontmatter::event` is a single `HookEvent` value (`src/spec.rs:253`); a user who wants the same script on (e.g.) both `pre_tool_use` and `post_tool_use` must duplicate the `[hooks.<id>]` block under different IDs and point both at the same script
    - Likely shape: rename the field from `event` to `events: Vec<HookEvent>` in `hooks.toml` (always a list — single-event hooks just use a one-element array). Expand internally to N normalized hook entries — one per event — each carrying the same script reference, timeout, matcher, etc. This is a breaking change to the TOML schema; appropriate pre-1.0 but should be called out in the changelog.
    - The matcher-compatibility rule (`HookEvent::allows_matcher` at `src/spec.rs:299`) shifts from per-source-entry to per-expanded-event: a `matcher` paired with an `events` list that mixes tool-execute and non-tool-execute events should fail validation with a message that names the offending event(s)
    - Open question — ownership-sentinel collision: `_agentspec_id` is emitted per `EmittedHookEntry` and sourced from `spec.id()` (`src/compile.rs:367`); expanding one source entry into N entries gives them all the same `_agentspec_id`. The merge layer groups by event/matcher before checking ownership (`src/hooks_merge.rs:127`, `298`), so cross-event duplicates should be independent rows — but worth re-verifying the removal path in `hooks_merge` before committing to the expansion strategy
    - Open question — id derivation interaction: if TODO #1 lands (id derived from path), one source file producing N entries means one path produces N ids; either the per-entry id needs an event suffix (`<id>:<event>`) or the sentinel needs to allow many-to-one mapping. Resolving #1 first would clarify the right call here.
    - Out of scope for the hooks-pipeline branch; small-enough change to land standalone once #1 is settled.
16. CST-aware tidy for `opencode.json` (parity with Claude/Cursor)
    - The `OpenCode` remove path (`remove_opencode_instructions`) and sync path (`patch_opencode_instructions`) both use plain `serde_json` to read/rewrite `opencode.json`, so any user-authored comments and formatting trivia are lost across a sync or remove cycle
    - Claude and Cursor go through `hooks_merge::tidy_jsonc_file` / `merge_*_settings`, both backed by `jsonc-parser`'s CST API, which preserves comments and trivia
    - Fix: route OpenCode's instructions tidy through a CST-aware helper analogous to `tidy_jsonc_file`, with a per-provider strategy (the array shape is `instructions: [string, ...]` rather than the nested matcher-group shape Claude uses); pairs naturally with TODO #13's broader migration of provider-specific JSON-merge knowledge into adapters
    - The doc comment on `remove_opencode_instructions` already calls out this asymmetry as pre-existing; the TODO entry pins it for tracking against pre-1.0 work
    - Out of scope for the remove-pipeline branch; revisit alongside TODO #13
17. Make `Manifest::load` strict-by-default (parity with `load_strict`)
    - `Manifest::load` (used by `sync` at `emit.rs:393`) silently accepts any version, including future-version manifests; `Manifest::load_strict` (used by `remove`) refuses any manifest whose `version > MANIFEST_VERSION`
    - A user who upgrades to a future agentspec, then runs an older agentspec's `sync`, would have the future-version manifest deserialized into the older shape and silently rewritten at the older version on save — destroying any future-version-specific fields
    - Today the schema is `MANIFEST_VERSION = 1` so the risk is dormant. The asymmetry only matters when the schema bumps; mitigating before the first bump is the right time
    - Likely shape: switch `sync`'s call site to `load_strict`, surface forward-incompatible manifests as actionable errors with the same upgrade-or-remove guidance the remove path already uses
    - A doc comment on `Manifest::load` (this branch) calls out the asymmetry inline so a future reader sees it at the call site
    - Out of scope for the remove-pipeline branch; address before the first manifest schema bump
