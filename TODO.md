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
3. Decouple GeneratedFile.path from the output directory structure
   - Currently paths include the `generated/` prefix (e.g. `generated/claude/agents/foo.md`)
   - `write_generated_files` strips it via `.parent()` — fragile implicit coupling
   - `check_generated_state` uses `base_dir.join()` directly — inconsistent resolution
   - Fix: store paths relative to the provider root (e.g. `claude/agents/foo.md`)
4. Unify emit and sync into a placement/strategy layer
   - emit and sync are the same operation (write files) with different targets and strategies
   - `generated/` should be optional — a cache/inspection artifact, not a mandatory intermediate step
   - Library should expose placement strategy types so consumers can route CompileResult to
     tool config dirs directly, without needing to write to generated/ first
   - SyncStrategy (symlink vs copy) and per-provider config remain binary/consumer concerns
   - Special cases to untangle: OpenCode opencode.json patching, --no-compile reuse
   - Pre-requisite: clean up lib.rs extraction (emit/sync currently binary-only)
