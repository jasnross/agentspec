# agentspec

Rust binary that compiles provider-neutral agent/skill spec files (Markdown
with YAML frontmatter) into ready-to-use configurations for Claude Code,
OpenCode, and Cursor.

## Commands

```sh
# From this directory (agentspec/)
cargo build
cargo test
cargo fmt                       # format all source files
cargo fmt --check               # verify formatting without writing (what CI runs)
cargo clippy --all-targets      # lint all targets (including tests)
cargo install --path .          # reinstall binary after schema changes (see below)

# From agent-config/ (the spec library that exercises this compiler)
agentspec validate              # schema + semantic checks only
agentspec compile               # full pipeline; writes generated/
agentspec sync                  # compile + distribute to tool config dirs
agentspec sync --dry-run        # preview sync operations without writing
```

## Release Runbook

GitHub Actions policy:

- All workflow `uses:` references must be pinned to full-length commit SHAs.
- Do not use floating tags like `@v4` or `@stable` in this repository.
- When adding or updating an action, resolve the current tag to a commit SHA and
  include the human-readable tag in an inline comment (for example `# v4`).

Standard release flow:

1. Merge conventional-commit changes to `main`.
2. Review and merge the `release-please` PR (version + changelog).
3. Verify tag-triggered `release.yml` jobs succeed:
   - all target builds and archive smoke tests
   - checksum generation/verification
   - SBOM + attestation publication
   - Homebrew gate
4. Update `jasnross/homebrew-tap` `Formula/agentspec.rb` with the new version
   and SHA256 entries from `SHA256SUMS`.
5. Confirm install paths (`brew` and `mise`) and announce release.

Hotfix exception path:

- Use `release.yml` `workflow_dispatch` only when waiting for the normal
  `release-please` PR is unacceptable (e.g., urgent breakage/security fix).
- Record the reason in release notes and follow up with a normal release PR to
  return to the default protected-branch flow.

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

Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `perf`, `style`.
Scope is optional but helpful for module-level changes.
Breaking changes: append `!` after type/scope (`feat!:`) and describe in the
commit body with `BREAKING CHANGE:`.

## Clippy

Five lint groups (`complexity`, `pedantic`, `perf`, `style`, `suspicious`) are
denied at `priority = -1` in `Cargo.toml`. Four `restriction` lints are
additionally opted into: `expect_used`, `panic`, `unwrap_used`,
`wildcard_enum_match_arm`.

Notable lints that affect everyday coding:

- `unwrap_used` — use `?`, `match`, or an explicit fallback instead of `.unwrap()`
- `expect_used` / `panic` — avoid in non-test code; tests are allowed via
  `clippy.toml` (`allow-expect-in-tests`, `allow-panic-in-tests`)
- `uninlined_format_args` — write `format!("{x}")` not `format!("{}", x)`
- `doc_markdown` — wrap identifiers like `NormalizedSpec`, `MiniJinja`,
  `OpenCode` in backticks in doc comments
- `unnecessary_wraps` — don't return `Result<T>` from functions that can't fail

Two pedantic lints are explicitly allowed: `similar_names` (flags unambiguous
pairs like `dst`/`dest`) and `struct_field_names` (flags structs where fields
share a suffix like `*_dir`).

Any `#[allow(clippy::...)]` should include a nearby comment explaining why it's
needed and keep scope as narrow as possible (item-level over module-level).

Run `cargo fmt && cargo clippy --all-targets` before committing; CI enforces both.

## Serde

Prefer struct-level attributes over repeating the same attribute on every field:

- **`#[serde(rename_all = "...")]`** — use on the struct instead of per-field
  `#[serde(rename = "...")]` when all fields follow the same naming convention
  (e.g., `"kebab-case"`, `"camelCase"`).
- **`#[serde_with::skip_serializing_none]`** — use on the struct instead of
  `#[serde(skip_serializing_if = "Option::is_none")]` on every `Option` field.
  `serde_with` is already a dependency.
- **`#[serde(deny_unknown_fields)]`** — add to any struct deserialized from
  user-facing input (spec frontmatter, config files). This turns typos and
  unrecognized fields into parse errors instead of silent no-ops. Omit only
  for structs that intentionally allow extension (e.g., pass-through types).

The first two compose cleanly and can appear on the same struct.

## Module Layout

Prefer the modern Rust module file convention over `mod.rs`:

```
src/adapters.rs          ← module root (not src/adapters/mod.rs)
src/adapters/claude.rs
src/adapters/cursor.rs
src/plan.rs              ← WritePlan, FileWrite, WriteMode, ConfigPatch (library)
```

`mod.rs` files are harder to navigate in editors (multiple open tabs all named `mod.rs`) and are the older convention. The exception is `main.rs` and `lib.rs`, which are standard entry points.

## Design Principles

### Colocate code with its consumer

Place functions in the module that calls them, not in a module named after an
abstract category. If a function has one caller, it belongs next to that caller
— even if it "sounds like" it belongs elsewhere. A function called
`resolve_dest_dir` that's only used by `sync_plan` belongs in `sync.rs`, not in
a separate `provider.rs`.

When a function is private to its module, colocating it also minimizes the
public API surface — fewer `pub` items, fewer cross-module imports, less
indirection to discover.

### Provider-specific logic belongs in adapters

Adapters own all provider-specific content decisions: which frontmatter fields
to set, how to serialize them, how to construct output paths, and how to apply
transforms like name prefixing or stripping. Emit should never need to know
about provider-specific frontmatter structure.

If adding support for a new provider, implement one adapter — no changes to
emit, plan, or sync should be needed.

### Operate on structs, not serialized strings

Never parse or transform YAML/frontmatter by manipulating serialized strings
(line-by-line scanning, regex, etc.). Instead, modify typed structs before
serialization. If a field needs to be conditionally included, make it
`Option<T>` on the struct and let serde handle it via
`#[serde_with::skip_serializing_none]`.

### Use config structs at module boundaries

When passing configuration across module boundaries (especially from the binary
crate into the library crate), use a named struct — not loose parameters, tuples,
or the raw `AgentspecConfig`. This pattern is established by `TemplatingConfig`,
`SpecDirs`, and `AdapterConfig`.

Key conventions:

- **Library-side structs** (`TemplatingConfig`, `SpecDirs`, `AdapterConfig`) have
  no dependency on clap, serde, or the binary crate's config types. The binary
  constructs them from `AgentspecConfig`.
- **`Option<&Config>` means "use defaults"** — when a config is optional (like
  `AdapterConfig` for adapters), pass `Option<&Config>` where `None` produces
  canonical/default output.
- **Centralize construction** — if multiple call sites build the same config from
  the same source, extract a helper (e.g., `config.adapter_configs()`).

## Pipeline Stages

`main.rs` orchestrates these stages in order, each consuming the previous stage's
output (typestate pattern — passing the wrong stage is a compile error):

1. **Load** — `specs.rs` (`Specs::load`) reads `.md` files from `spec/agents/`,
   `spec/skills/`, and `spec/rules/`, parses frontmatter via `gray_matter` into
   typed structs → `Specs`
2. **Normalize** — `validate.rs` applies defaults → `NormalizedSpecs`
3. **Validate** — `validate.rs` runs semantic checks (duplicate IDs, unknown
   presets, etc.) → `ValidatedSpecs`
4. **Template resolution** — `templating.rs` renders MiniJinja templates in
   spec bodies with a context containing built-in variables (e.g., `specs`)
   and resolves `{% include %}` fragment references → `ResolvedSpecs`
5. **Compile** — `compile.rs` dispatches each `(spec, provider)` pair to a
   provider adapter, passing `Option<&AdapterConfig>` for prefix/strip
   transforms → `CompileResult`. Adapters produce fully-formed output
   (paths, frontmatter, content) — no post-hoc transforms downstream.
6. **Plan** — `plan.rs` + `sync.rs` build a `WritePlan` from `CompileResult` and
   config: `compile_plan` (for `compile`) or `sync_plan` (for `sync`)
7. **Emit** — `emit.rs` executes the `WritePlan`: `CleanSlate` mode for `compile`
   (delete-and-rewrite `generated/<provider>/`), `ManifestTracked` mode for `sync`
   (per-file ownership tracking, stale cleanup, direct write to tool config dirs);
   runs adapter-provided post-write hooks (e.g., `OpenCode` `opencode.json`
   patching). Emit is purely file I/O, manifest tracking, and hook execution —
   no content transformation or provider-specific logic of its own.

## Integration Tests

`tests/pipeline.rs` runs the real `agentspec` binary against the sibling
`agent-config/` directory. These tests:

- Are skipped automatically if `agent-config/` doesn't exist (e.g., in CI without
  the full dotfiles checkout)
- Use `env!("CARGO_BIN_EXE_agentspec")` — they always test the binary built by
  the current `cargo test` invocation, not any previously installed version
- Assert on hardcoded spec counts (8 agents, 27 skills); update those constants
  when adding or removing specs
