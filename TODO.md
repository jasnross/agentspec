1. Consider deriving id from path instead of requiring in frontmatter
   - Currently `id` is a required `String` in all frontmatter structs; missing it causes a parse
     error at load time
   - `id` is used for two things: duplicate detection in validation, and output filename/dirname
     construction in every adapter
   - The file path already encodes a natural identity: the stem for agents and rules
     (`agents/foo.md` → `foo`), the directory name for skills (`skills/foo/` → `foo`)
   - Deriving id from path would eliminate redundant frontmatter, prevent path/id mismatches, and
     make the path the single source of truth
   - The open question is whether callers ever legitimately want the output filename to differ from
     the source filename; if so, `id` (or a rename field) remains useful as an override
2. Do we actually need the normalized types?
   - `NormalizedSpec` / `NormalizedAgentSpec` / etc. are structurally identical to their raw
     counterparts — normalization is a field-by-field pass-through with no defaults, no coercions,
     and no validation applied
   - The only material difference is that normalized frontmatter structs derive `Clone`, which
     compile.rs needs to clone specs across multiple provider adapters in a single loop
   - The simplest fix: add `#[derive(Clone)]` to the raw frontmatter structs and delete the entire
     `Normalized*` type family, `normalize_specs()`, and the normalization stage from the pipeline
   - The typestate stages (Specs → NormalizedSpecs → ValidatedSpecs) would collapse to
     Specs → ValidatedSpecs with no loss of safety
   - Retain this layer only if we expect real normalization work to arrive (e.g., applying preset
     defaults into spec fields before compilation)
3. Figure out how to generate plugin content for Claude and Cursor
   - In particular, the plugin manifest files
4. Allow specifying path to configuration file
5. Do we need strip_name if we can set a prefix?
6. Cleanup: Refactor methods with clippy::too_many_lines
7. Move post-write patching logic out of emit into adapters
   - `patch_opencode_instructions` in `emit.rs` is ~90 lines of OpenCode-specific logic that
     patches `opencode.json` with rule file paths after sync writes
   - This violates the principle that provider-specific logic belongs in adapters, but it can't
     simply move to compile time because it depends on destination paths (known only at plan time)
   - The `ConfigPatch` enum in `plan.rs` is also OpenCode-specific — adding another provider
     with post-write needs would require a new variant in the library crate
   - Possible approach: adapters produce a `PostWriteAction` at compile time describing what
     needs to happen ("patch opencode.json instructions with rule paths"), the plan stage
     resolves concrete paths, and emit executes the action generically without knowing it's
     OpenCode-specific
   - This would also set up the pattern for other providers that may need post-write hooks
     (e.g., Cursor plugin manifests)
