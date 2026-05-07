# agentspec

Rust binary that compiles provider-neutral agent/skill spec files (Markdown with YAML frontmatter) into ready-to-use configurations for Claude Code, OpenCode, and Cursor.

## Project Status

**agentspec is pre-1.0.** The architecture, public API, CLI surface, and configuration formats are all expected to evolve. This is the cheapest time to make foundational changes — once we ship 1.0 and downstream consumers start depending on stable behavior, every assumption hardens and refactors get expensive.

When weighing design decisions:

- Refactorings and breaking changes are on the table. Weigh the tradeoffs, but don't reflexively defer hard changes to "later" — later is when they're harder to make.
- If components are difficult to fit together, abstractions are trending toward leaky, or a change is awkward to implement, treat that friction as a signal to reshape the surrounding code rather than work around it.
- **"First make the change easy, then make the easy change."** When the next feature feels awkward, the right first step is often to refactor so the feature drops in cleanly — then add it.

This bias toward refactoring does not override scope discipline. Improve what you touch in service of the current task; surface larger structural changes as their own work rather than smuggling them into unrelated commits.

## Commands

```sh
just check                      # format + lint + build + test + license check (the full suite)
just fmt                        # format: cargo fix + cargo +nightly fmt
just lint                       # clippy on all targets
just test                       # run tests
just build                      # build only
just licenses                   # cargo deny check licenses
just install                    # install binary locally

# Or without just:
cargo build
cargo test
cargo fmt                       # format all source files
cargo fmt --check               # verify formatting without writing (what CI runs)
cargo clippy --all-targets      # lint all targets (including tests)
cargo install --path .          # reinstall binary after schema changes (see below)

# From dotfiles/agent-config/ (the spec library that exercises this compiler)
agentspec validate              # schema + semantic checks only
agentspec compile               # full pipeline; writes generated/
agentspec sync                  # compile + distribute to tool config dirs
agentspec sync --dry-run        # preview sync operations without writing
agentspec remove                # reverse of sync; consults manifest, never deletes host config files
```

## Release Runbook

GitHub Actions policy:

- All workflow `uses:` references must be pinned to full-length commit SHAs.
- Do not use floating tags like `@v4` or `@stable` in this repository.
- When adding or updating an action, resolve the current tag to a commit SHA and include the human-readable tag in an inline comment (for example `# v4`).

Standard release flow:

1. Merge conventional-commit changes to `main`.
2. Review and merge the `release-please` PR (version + changelog).
3. Verify tag-triggered `release.yml` jobs succeed:
   - all target builds and archive smoke tests
   - checksum generation/verification
   - SBOM + attestation publication
   - Homebrew gate
4. Update `jasnross/homebrew-tap` `Formula/agentspec.rb` with the new version and SHA256 entries from `SHA256SUMS`.
5. Confirm install paths (`brew` and `mise`) and announce release.

Hotfix exception path:

- Use `release.yml` `workflow_dispatch` only when waiting for the normal `release-please` PR is unacceptable (e.g., urgent breakage/security fix).
- Record the reason in release notes and follow up with a normal release PR to return to the default protected-branch flow.

Release execution checklist:

- [ ] release-please PR merged and tag created
- [ ] `release.yml` completed with all checks green
- [ ] release page contains archives + `SHA256SUMS` + SPDX SBOM + attestations
- [ ] `homebrew-tap` formula updated and CI passed
- [ ] `brew` + `mise` install paths verified

Hotfix decision criteria:

- user-facing breakage in latest release with no immediate workaround
- security issue requiring same-day patch
- release pipeline/tagging fault that blocks standard remediation timing

## Git Commits

All commits must follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/):

```
<type>(<scope>): <description>

feat: add cursor adapter
fix(emit): handle missing output directory
refactor(compile): remove manifest hashing
docs: update README with preset examples
test(fragments): add include_indented edge case
chore: bump serde to 1.0.210
```

Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `perf`, `style`. Scope is optional but helpful for module-level changes. Breaking changes: append `!` after type/scope (`feat!:`) and describe in the commit body with `BREAKING CHANGE:`.

## Clippy

Five lint groups (`complexity`, `pedantic`, `perf`, `style`, `suspicious`) are denied at `priority = -1` in `Cargo.toml`. Four `restriction` lints are additionally opted into: `expect_used`, `panic`, `unwrap_used`, `wildcard_enum_match_arm`.

Notable lints that affect everyday coding:

- `unwrap_used` — use `?`, `match`, or an explicit fallback instead of `.unwrap()`
- `expect_used` / `panic` — avoid in non-test code; tests are allowed via `clippy.toml` (`allow-expect-in-tests`, `allow-panic-in-tests`)
- `uninlined_format_args` — write `format!("{x}")` not `format!("{}", x)`
- `doc_markdown` — wrap identifiers like `NormalizedSpec`, `MiniJinja`, `OpenCode` in backticks in doc comments
- `unnecessary_wraps` — don't return `Result<T>` from functions that can't fail

Two pedantic lints are explicitly allowed: `similar_names` (flags unambiguous pairs like `dst`/`dest`) and `struct_field_names` (flags structs where fields share a suffix like `*_dir`).

Any `#[allow(clippy::...)]` should include a nearby comment explaining why it's needed and keep scope as narrow as possible (item-level over module-level).

Run `cargo fmt && cargo clippy --all-targets` before committing; CI enforces both.

## Serde

Prefer struct-level attributes over repeating the same attribute on every field:

- **`#[serde(rename_all = "...")]`** — use on the struct instead of per-field `#[serde(rename = "...")]` when all fields follow the same naming convention (e.g., `"kebab-case"`, `"camelCase"`).
- **`#[serde_with::skip_serializing_none]`** — use on the struct instead of `#[serde(skip_serializing_if = "Option::is_none")]` on every `Option` field. `serde_with` is already a dependency.
- **`#[serde(deny_unknown_fields)]`** — add to any struct deserialized from user-facing input (spec frontmatter, config files). This turns typos and unrecognized fields into parse errors instead of silent no-ops. Omit only for structs that intentionally allow extension (e.g., pass-through types).

The first two compose cleanly and can appear on the same struct.

## Module Layout

Prefer the modern Rust module file convention over `mod.rs`:

```
src/adapters.rs          ← module root (not src/adapters/mod.rs)
src/adapters/claude.rs
src/adapters/cursor.rs
src/plan.rs              ← CompilePlan/SyncPlan/RemovePlan + per-mode write structs (library)
```

`mod.rs` files are harder to navigate in editors (multiple open tabs all named `mod.rs`) and are the older convention. The exception is `main.rs` and `lib.rs`, which are standard entry points.

## Design Principles

### Colocate code with its consumer

Place functions in the module that calls them, not in a module named after an abstract category. If a function has one caller, it belongs next to that caller — even if it "sounds like" it belongs elsewhere. A function called `resolve_dest_dir` that's only used by `sync_plan` belongs in `sync.rs`, not in a separate `provider.rs`.

When a function is private to its module, colocating it also minimizes the public API surface — fewer `pub` items, fewer cross-module imports, less indirection to discover.

### Provider-specific logic belongs in adapters

Adapters own all provider-specific content decisions: which frontmatter fields to set, how to serialize them, how to construct output paths, and how to apply transforms like name prefixing or stripping. Emit should never need to know about provider-specific frontmatter structure.

If adding support for a new provider, implement one adapter — no changes to emit, plan, or sync should be needed.

### Operate on structs, not serialized strings

Never parse or transform YAML/frontmatter by manipulating serialized strings (line-by-line scanning, regex, etc.). Instead, modify typed structs before serialization. If a field needs to be conditionally included, make it `Option<T>` on the struct and let serde handle it via `#[serde_with::skip_serializing_none]`.

### Use config structs at module boundaries

When passing configuration across module boundaries (especially from the binary crate into the library crate), use a named struct — not loose parameters, tuples, or the raw `AgentspecConfig`. This pattern is established by `TemplatingResources`, `SpecDirs`, and `AdapterConfig`.

Key conventions:

- **Library-side structs** (`TemplatingResources`, `SpecDirs`, `AdapterConfig`) have no dependency on clap, serde, or the binary crate's config types. The binary constructs them from `AgentspecConfig`.
- **`Option<&Config>` means "use defaults"** — when a config is optional (like `AdapterConfig` for adapters), pass `Option<&Config>` where `None` produces canonical/default output.
- **Centralize construction** — if multiple call sites build the same config from the same source, extract a helper (e.g., `config.adapter_configs()`).

## Pipeline Stages

`main.rs` orchestrates these stages in order, each consuming the previous stage's output (typestate pattern — passing the wrong stage is a compile error):

1. **Load** — `specs.rs` (`Specs::load`) reads `.md` files from `spec/agents/`, `spec/skills/`, `spec/rules/`, and `spec/hooks/hooks.toml` (plus a recursive walk of `spec/hooks/scripts/`), parses frontmatter via `gray_matter` (markdown specs) and `serde_path_to_error` over `toml` (hooks) into typed structs → `(Specs, LoadReport)`. Files (and subtrees) matching any pattern in `[spec].ignore` are skipped before frontmatter parsing via `WalkDir::filter_entry` pruning; the `LoadReport` records what was filtered and which patterns matched zero files so `main.rs` can surface warnings and listings.
2. **Normalize** — `validate.rs` applies defaults → `NormalizedSpecs`
3. **Validate** — `validate.rs` runs semantic checks (duplicate IDs, unknown presets, hook event/matcher compatibility, etc.) → `ValidatedSpecs`
4. **Compile** — `compile.rs` resolves MiniJinja templates (rendering built-in variables like `specs` and `{% include %}` fragment references) and then dispatches each `(provider, spec)` pair to a provider adapter, passing `Option<&AdapterConfig>` for prefix/strip transforms (`prefix` controls file paths and frontmatter, `content_prefix` controls model-facing names) → `CompileResult`. After per-spec dispatch, a per-provider `synthesize_hooks` step emits `hooks/hooks.json` (Bundled/Path mode) plus the entire `hooks/scripts/` tree — Merged (User/Project) mode emits scripts only, leaving the JSON to the post-write patcher. `CompileResult.hooks` carries the canonical per-provider `Vec<EmittedHookEntry>` so the merge layer doesn't have to re-derive entries from emitted JSON. Template resolution is an internal step of compilation, not a separate pipeline stage. Adapters produce fully-formed output (paths, frontmatter, content) — no post-hoc transforms downstream.
5. **Plan** — Three sibling constructors each build their own typed plan from `CompileResult` and config: `agentspec::plan::compile_plan → CompilePlan` (library-side, for `compile`), the binary crate's `sync::sync_plan → SyncPlan` (for `sync`), and the binary crate's `remove::remove_plan → RemovePlan` (for `remove`). Each plan carries only the fields its mode actually uses — `CompilePlan` holds `Vec<CleanSlateWrite>`, `SyncPlan` holds `Vec<ManifestTrackedWrite>` plus post-write hooks, `RemovePlan` holds `Vec<RemoveWrite>` plus post-write hooks. `remove_plan` skips spec compilation — it only consults the manifest at execution time.
6. **Emit** — `emit.rs` exposes one entry point per plan type, each consuming only its corresponding plan: `emit_compile(&CompilePlan, dry_run)` deletes-and-rewrites `generated/<provider>/`; `emit_sync(&SyncPlan, dry_run, verbose)` does per-file ownership tracking, stale cleanup, and direct writes to tool config dirs; `emit_remove(&RemovePlan, dry_run)` deletes every manifest-recorded file, deletes the manifest, prunes empty agentspec-created subdirs, and rmdir's the dest dir if empty. `emit_sync` and `emit_remove` then run adapter-provided post-write hooks (`OpenCode` `opencode.json` patching, `ClaudeHooksPatch`/`CursorHooksPatch` CST-aware merge into `settings.json`/`hooks.json` for hooks at User/Project sync mode — backed by `jsonc-parser` so comments, trailing commas, and user-authored entries round-trip unchanged; the inverse `ClaudeRemoveHooksPatch` and `CursorRemoveHooksPatch` strip `_agentspec_id`-tagged entries from `settings.json`/`hooks.json`, while `OpenCodeRemoveInstructionsPatch` filters `instructions[]` by `rules_dest_dir` prefix; all three tidy emptied containers without ever deleting the host file). `emit_compile` carries no post-write hooks — `CompilePlan` has no field for them. Emit is purely file I/O, manifest tracking, and hook execution — no content transformation or provider-specific logic of its own.

## Integration Tests

`tests/pipeline.rs` runs the real `agentspec` binary against the sibling `agent-config/` directory. These tests:

- Use `tests/fixtures/agent-config/` as a self-contained fixture (copied per-test into a `TempDir`); no dependency on a sibling `agent-config/` checkout.
- Use `env!("CARGO_BIN_EXE_agentspec")` — they always test the binary built by the current `cargo test` invocation, not any previously installed version.
- Tests for hooks install fixture content programmatically via `install_hook_fixture()` after `setup()` (rather than committing hook content under the shared fixture) so non-hook tests aren't affected by the hooks pipeline's mode-specific behavior.

## graphify

This project has a graphify knowledge graph at graphify-out/.

Rules:

- Before answering architecture or codebase questions, read graphify-out/GRAPH_REPORT.md for god nodes and community structure
- If graphify-out/wiki/index.md exists, navigate it instead of reading raw files
- For cross-module "how does X relate to Y" questions, prefer `graphify query "<question>"`, `graphify path "<A>" "<B>"`, or `graphify explain "<concept>"` over grep — these traverse the graph's EXTRACTED + INFERRED edges instead of scanning files
- After modifying code files in this session, run `graphify update .` to keep the graph current (AST-only, no API cost)
