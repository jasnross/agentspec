1. Figure out how to generate plugin content for Claude and Cursor
   - In particular, the plugin manifest files
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
1. Do we actually need the normalized types?
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
