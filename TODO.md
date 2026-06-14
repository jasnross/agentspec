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
