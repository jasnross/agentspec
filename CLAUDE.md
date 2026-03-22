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
agentspec compile --profile home  # with machine profile overlay
agentspec check                 # verify generated files match compile output
```

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

All clippy lint groups are set to `deny` in `Cargo.toml`. Notable ones that
affect everyday coding:

- `unwrap_used` — use `?`, `match`, or an explicit fallback instead of `.unwrap()`
- `expect_used` — avoid `.expect()` in non-test code; in tests, prefer explicit assertions and only allow `clippy::expect_used` at the test-module/file level when it materially improves readability
- `uninlined_format_args` — write `format!("{x}")` not `format!("{}", x)`
- `doc_markdown` — wrap identifiers like `CanonicalSpec`, `MiniJinja`, `OpenCode`
  in backticks in doc comments
- `unnecessary_wraps` — don't return `Result<T>` from functions that can't fail

Run `cargo fmt && cargo clippy --all-targets` before committing; CI enforces both.

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
   splits frontmatter from body, produces `Vec<CanonicalSpec>`
2. **Fragment resolution** — `fragments.rs` renders MiniJinja `{% include %}`
   and `{% with %}` tags in spec bodies
3. **Schema validation** — `validate.rs` checks frontmatter against
   `schemas/canonical.schema.json` (embedded at compile time via `include_str!`)
4. **Normalization** — `validate.rs` applies defaults, deduplicates/sorts tools,
   resolves targets → `Vec<NormalizedSpec>`
5. **Preset resolution** — `config.rs` merges machine profile overlays into presets
6. **Semantic validation** — `validate.rs` checks preset references, tool names, etc.
7. **Compile** — `compile.rs` dispatches each `(spec, target)` pair to a provider
   adapter → `CompileResult { files, warnings }`
8. **Emit** — `emit.rs` writes files to disk (`compile`) or diffs against disk
   (`check`)

## Module Map

| Module         | Role                                                                                                                   |
| -------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `types.rs`     | All shared data types: `CanonicalSpec`, `NormalizedSpec`, `Execution`, `PresetsMap`, `CompileResult`, `Provider`, etc. |
| `cli.rs`       | `clap` argument parsing; `CommonArgs` reads `AGENTSPEC_PROFILE` env var                                                |
| `config.rs`    | `agentspec.toml` discovery and parsing; `resolve_presets()` merges machine profile overlays                            |
| `parse.rs`     | Loads `.md` spec files from disk                                                                                       |
| `fragments.rs` | MiniJinja environment setup and fragment rendering                                                                     |
| `schema.rs`    | Embeds `canonical.schema.json` via `include_str!`; parses it once at startup                                           |
| `validate.rs`  | Schema validation, normalization, semantic checks                                                                      |
| `compile.rs`   | Provider dispatch loop; sorts output by path                                                                           |
| `emit.rs`      | Writes files to disk; `check_generated_state` diffs expected vs actual                                                 |
| `format.rs`    | YAML frontmatter serializer; hand-rolled to match js-yaml plain-string style                                           |
| `model.rs`     | Resolves a spec's `execution.preset` name to a `ModelConfig` for a specific provider                                   |
| `tools.rs`     | Canonical → provider-specific tool name mapping table                                                                  |
| `adapters/`    | One file per provider: `claude.rs`, `opencode.rs`, `codex.rs`, `cursor.rs`                                             |

## Key Types

- **`CanonicalSpec`** — raw parsed spec (path + frontmatter JSON + body string)
- **`NormalizedSpec`** — post-validation spec with all defaults resolved and fields typed
- **`PresetsMap`** — `HashMap<preset_name, HashMap<provider, serde_json::Value>>`;
  produced by `resolve_presets()`, consumed by adapters
- **`ModelConfig`** — resolved `{ model, variant, reasoning_effort }` for one provider
- **`GeneratedFile`** — a single output file with provider, relative path, and bytes
- **`CompileResult`** — `{ files: Vec<GeneratedFile>, warnings: Vec<CompileWarning> }`

## Provider Support Matrix

| Feature                  | Claude | OpenCode           | Codex | Cursor    |
| ------------------------ | ------ | ------------------ | ----- | --------- |
| Agents                   | ✓      | ✓                  | —     | —         |
| Skills (user-invocable)  | ✓      | ✓ (commands/)      | ✓     | ✓         |
| Skills (agent-invocable) | ✓      | ✓ (skills/)        | —     | —         |
| Rules                    | ✓      | ✓ (instructions/)  | ✓     | ✓         |
| Tool map                 | list   | boolean object     | list  | inherited |

## Tool Name Mapping

Canonical tool names (used in spec frontmatter) → provider-specific names live
in `tools.rs`. Three return values from `tool_name()`:

- `None` — unknown canonical name; emits `MissingMapping` warning
- `Some(None)` — intentionally unsupported on this provider (silently dropped)
- `Some(Some(name))` — the provider-specific string to emit

`ls` is Claude-only; all other providers return `Some(None)`.
To add a new tool, add rows to the `match` table in `tools.rs` and update the
`CANONICAL` slice in `all_tool_names`.

## Schema Embedding

`schema.rs` uses `include_str!("../schemas/canonical.schema.json")` to embed
the schema at compile time. After changing the schema, you must rebuild and
reinstall the binary (`cargo install --path .`) before the updated schema takes
effect — the installed `agentspec` binary will otherwise still enforce the old
schema.

The authoritative schema is `schemas/canonical.schema.json` in this repository.
There is no separate copy in `agent-config/` — the binary is the single source of truth.

## Integration Tests

`tests/dotfiles_spec.rs` runs the real `agentspec` binary against the sibling
`agent-config/` directory. These tests:

- Are skipped automatically if `agent-config/` doesn't exist (e.g., in CI without
  the full dotfiles checkout)
- Use `env!("CARGO_BIN_EXE_agentspec")` — they always test the binary built by
  the current `cargo test` invocation, not any previously installed version
- Assert on hardcoded spec counts (8 agents, 27 skills); update those constants
  when adding or removing specs

## Presets and Profiles

**Presets** (`[presets.*]` in `agentspec.toml`) are named model config bundles
for different task types (e.g., `deep_review`, `balanced`). Specs reference
them via `execution.preset:` in frontmatter.

**Profiles** (`[profiles.<name>.*]`) are per-machine overlays that merge over
presets at compile time. Selected via `--profile <name>` or
`AGENTSPEC_PROFILE=<name>`. Values at the same `preset → provider` key replace
the base preset value entirely.
