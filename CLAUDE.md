# agentspec

Rust binary that compiles provider-neutral agent/skill spec files (Markdown
with YAML frontmatter) into ready-to-use configurations for Claude Code,
OpenCode, Codex, and Cursor.

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
agentspec check                 # verify generated files match compile output
agentspec sync                  # compile + distribute to tool config dirs
agentspec sync --dry-run        # preview sync operations without writing
agentspec sync --no-compile     # sync from existing generated output
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
- `doc_markdown` — wrap identifiers like `NormalizedSpecNew`, `MiniJinja`,
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

These compose cleanly — both can appear on the same struct.

- **`#[serde(deny_unknown_fields)]`** — add to any struct deserialized from
  user-facing input (spec frontmatter, config files). This turns typos and
  unrecognized fields into parse errors instead of silent no-ops. Omit only
  for structs that intentionally allow extension (e.g., pass-through types).

## Module Layout

Prefer the modern Rust module file convention over `mod.rs`:

```
src/adapters.rs          ← module root (not src/adapters/mod.rs)
src/adapters/claude.rs
src/adapters/cursor.rs
```

`mod.rs` files are harder to navigate in editors (multiple open tabs all named `mod.rs`) and are the older convention. The exception is `main.rs` and `lib.rs`, which are standard entry points.

## Pipeline Stages

`main.rs` runs these in order:

1. **Load** — `parse.rs` reads `.md` files from `spec/agents/`, `spec/skills/`, and `spec/rules/`,
   parses frontmatter via `gray_matter` into typed structs, produces `Vec<Spec>`
2. **Fragment resolution** — `fragments.rs` renders MiniJinja `{% include %}`
   and `{% with %}` tags in spec bodies
3. **Normalization** — `validate.rs` applies defaults → `Vec<NormalizedSpecNew>`
4. **Semantic validation** — `validate.rs` checks preset references, etc.
5. **Compile** — `compile.rs` dispatches each `(spec, target)` pair to a provider
   adapter → `CompileResult { files }`
6. **Emit** — `emit.rs` writes files to disk (`compile`) or diffs against disk
   (`check`)
7. **Sync** — `sync.rs` distributes generated files to each tool's config directory
   via symlink or copy strategy; patches `opencode.json` for rules (`sync` command only)

## Integration Tests

`tests/dotfiles_spec.rs` runs the real `agentspec` binary against the sibling
`agent-config/` directory. These tests:

- Are skipped automatically if `agent-config/` doesn't exist (e.g., in CI without
  the full dotfiles checkout)
- Use `env!("CARGO_BIN_EXE_agentspec")` — they always test the binary built by
  the current `cargo test` invocation, not any previously installed version
- Assert on hardcoded spec counts (8 agents, 27 skills); update those constants
  when adding or removing specs
