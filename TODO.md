- Consider deriving id from path instead of requring in frontmatter
- Do we actually need the normalized types?
- Unify emit and sync into a placement/strategy layer
  - emit and sync are the same operation (write files) with different targets and strategies
  - `generated/` should be optional — a cache/inspection artifact, not a mandatory intermediate step
  - Library should expose placement strategy types so consumers can route CompileResult to
    tool config dirs directly, without needing to write to generated/ first
  - SyncStrategy (symlink vs copy) and per-provider config remain binary/consumer concerns
  - Special cases to untangle: OpenCode opencode.json patching, --no-compile reuse
  - Pre-requisite: clean up lib.rs extraction (emit/sync currently binary-only)
