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
