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
10. Move `CompileDiagnostics` population into `compile::run`
    - Today `main.rs:run_compile` re-walks `validated.specs()` to populate `CompileDiagnostics::skipped_hooks` after `compile_specs` already iterated the same list
    - `compile_specs` is the natural owner: it knows exactly which `(provider, spec)` pairs got dispatched and which short-circuited (e.g., `OpenCode` hook specs returning `Ok(Vec::new())`)
    - Returning `(CompileResult, CompileDiagnostics)` from `compile::run` removes the `O(specs × providers)` re-walk in the binary and aligns ownership with the rest of the pipeline (load/validate/compile each return their own report struct)
11. Orphan cleanup for merged JSON entries when a sync target is removed
    - Sentinel-based ownership in `hooks_merge` only cleans up `_agentspec_id` entries when agentspec runs the merge for that provider
    - If a user removes `[sync.claude]` from `agentspec.toml` (or deletes all hook specs and the sync target), the next `sync` skips Claude entirely — previously-injected entries persist in `settings.json` as permanent orphans
    - The manifest layer handles the equivalent for files via per-destination `.agentspec-manifest.json` cleanup, but `settings.json` isn't manifest-tracked
    - Likely shape: an `agentspec sync --prune` (or `unmerge`/`uninstall`) subcommand that walks all candidate `settings.json` / `hooks.json` paths and strips agentspec-tagged entries
    - Out of scope for the hooks-pipeline branch; revisit before 1.0
12. Cursor empirical-verification gate for hook environment
    - Several Cursor behaviors are inferred from research docs and not verified against a real Cursor build:
      - Event-name mappings (`postToolUseFailure`, `sessionStart`, `sessionEnd`, `subagentStart`, `subagentStop`, `beforeSubmitPrompt`) — see `cursor.rs:215-220`
      - `${CLAUDE_PROJECT_DIR}` resolution outside plugin scope in `MergedProject` mode — see `compile.rs:128-130`
      - `${CLAUDE_PLUGIN_ROOT}` aliasing in Bundled (Path) mode — Cursor's documented hook env only mentions `CLAUDE_PROJECT_DIR`, so the bundled-mode command literal `${CLAUDE_PLUGIN_ROOT}/hooks/scripts/<f>` may not resolve at runtime
      - Whether Cursor accepts unknown sub-fields like `_agentspec_id` in hook entries (Phase 2's contingent design depends on this)
    - All four are blockers for confident Cursor support — verify before 1.0 against a real Cursor build and document the actual behavior
    - If Cursor doesn't alias `CLAUDE_PLUGIN_ROOT` in Bundled mode, the fix is likely to mirror the Merged-mode pattern: emit `CLAUDE_PLUGIN_ROOT=<derived> <command>` for Cursor specifically
13. Preserve verbatim file modes for all `SupportingFile` emission
    - Today both `compile.rs::build_hook_script_files` and `adapters/claude.rs::adapt_skill_spec` (and the analogous Cursor path) emit `mode = Some(0o755)` only when the source file is executable, and `None` otherwise — meaning non-executable helpers fall back to the system default (typically 0o644)
    - A user who deliberately set 0o600 on a secrets-style helper, or 0o400 on a read-only fixture, loses that intent in transit
    - Fix is symmetric across hook scripts and skill `supporting_files`: pass through `metadata.permissions().mode()` verbatim instead of collapsing to the executable-bit
    - Out of scope for the hooks-pipeline branch; tackle as a single cross-cutting change touching `build_hook_script_files`, both skill-adapter paths, and any future `SupportingFile`-emitting helpers
