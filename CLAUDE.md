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
just check                      # lint + cargo check + shell gates + test + licenses + format + probe summary
just fmt                        # format: prettier + cargo fix + cargo +nightly fmt + cargo sort-derives
just lint                       # clippy on all targets
just test                       # run tests
just build                      # build only
just licenses                   # cargo deny check licenses
just install                    # install binary locally

just shellcheck                 # lint the probe shell under experiments/
just bats-test                  # run the probe harness test suite
just probe-run                  # run the free probes; costlier ones are listed with the flag that frees them
just probe-run --billed         # additionally run the probes that spend model quota
just probe-run --manual         # additionally run the probes that block on a live provider session
just probe-run --all            # every driver; flags stack, so --billed --manual is the same thing
just probe-run --stale          # narrow to the probes owed a run: never recorded, or version drift
just probe-status               # report on committed probe records; invokes no probe

# Or without just:
cargo build
cargo test
cargo fmt                       # format all source files
cargo fmt --check               # verify formatting without writing (what CI runs)
cargo clippy --all-targets      # lint all targets (including tests)
cargo install --path .          # reinstall binary after schema changes (see below)

# From dotfiles/agent-config/ (the spec library that exercises this compiler)
agentspec validate              # schema + semantic checks only
agentspec inspect               # report configured values that reached no generated file
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

### Breaking means breaking for the shipped binary

The semver signal describes the released `agentspec` artifact — the binary, its CLI surface, the spec/config formats it parses, and the library crate's public API. A commit is breaking only when an existing user of that artifact has to change something.

**Probe-harness changes are never breaking.** Nothing under `experiments/` is compiled into the binary, packaged in the crate, or run by CI; it is a local-only measurement apparatus. Renaming a `just probe-run` flag or changing a `probe.json` enum breaks a workflow that only this repository's contributors run, and no release consumer can observe it. Marking such a commit breaking inflates the version and puts a `⚠ BREAKING CHANGES` banner in the user-facing changelog for a change users cannot see.

Concretely, for a commit that touches only `experiments/`, the `justfile`'s probe recipes, `TODO.md`, or the probe sections of this file:

- Do not append `!` to the type/scope.
- Do not write a `BREAKING CHANGE:` footer either — release-please treats that footer as a breaking marker on its own, with or without the `!`.
- Do still record the migration. Put it in the body, or under a footer token that carries no semver meaning, e.g. `Probe-harness-change: a manifest declaring script must migrate to billed.`

For the same reason, a probes-only commit does not use `feat` or `fix` — those are the two types the spec binds to a version bump, and neither a new probe package nor a repaired probe assertion changes the released artifact. Reach for the semver-inert types instead:

- `test(probes)` — new probe packages, recorded measurements, harness assertions. This is the default for probe work.
- `chore(probes)` — harness maintenance that measures nothing: recipe wiring, tooling, cleanup.
- `refactor` / `docs` / `style` — as they normally apply; these already carry no version weight.

The same reasoning applies in reverse: a commit scoped to `probes` that _also_ changes `src/`, `Cargo.toml`, or the spec format is judged on that part, and marks itself breaking if the shipped artifact broke.

## Clippy

Five lint groups (`complexity`, `pedantic`, `perf`, `style`, `suspicious`) are denied at `priority = -1` in `Cargo.toml`. Four `restriction` lints are additionally opted into: `expect_used`, `panic`, `unwrap_used`, `wildcard_enum_match_arm`.

Notable lints that affect everyday coding:

- `unwrap_used` — use `?`, `match`, or an explicit fallback instead of `.unwrap()`
- `expect_used` / `panic` — avoid in non-test code; tests are allowed via `clippy.toml` (`allow-expect-in-tests`, `allow-panic-in-tests`)
- `uninlined_format_args` — write `format!("{x}")` not `format!("{}", x)`
- `doc_markdown` — wrap identifiers like `ValidatedSpecs`, `MiniJinja`, `OpenCode` in backticks in doc comments
- `unnecessary_wraps` — don't return `Result<T>` from functions that can't fail

Two pedantic lints are explicitly allowed: `similar_names` (flags unambiguous pairs like `dst`/`dest`) and `struct_field_names` (flags structs where fields share a suffix like `*_dir`).

Any `#[allow(clippy::...)]` should include a nearby comment explaining why it's needed and keep scope as narrow as possible (item-level over module-level).

Run `just check` before committing to ensure linting, tests, and formatting pass. CI enforces the cargo gates — it invokes `cargo fmt --check`, `cargo clippy`, `cargo test`, and `cargo deny check licenses` directly rather than going through `just`. The shell gates `just check` adds (`shellcheck` and `bats-test`) are therefore **local-only**, as is the probe summary.

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
src/setting.rs           ← provider-neutral setting vocabulary (SettingKey/SettingKind/Carries)
```

`mod.rs` files are harder to navigate in editors (multiple open tabs all named `mod.rs`) and are the older convention. The exception is `main.rs` and `lib.rs`, which are standard entry points.

## Probes: Measured Provider Behavior

When adding a rendering agentspec will emit, check whether a probe already answers the question: `jq -r '.question' experiments/*/probe.json`. Every package under [`experiments/`](experiments/README.md) carries a manifest, so that list is complete; `just probe-status` shows what each one measured and how stale it is. Write and run a probe if nothing covers it, then design against the measurement. **A `billed` probe spends model quota and a `manual` one spends a live session**, so `just probe-run` withholds both, naming the flag that authorizes each (`--billed`, `--manual`, or `--all`). Probes verify the _provider's_ contract using hand-authored provider config, so a probe can precede the feature it de-risks rather than gating it afterward.

**Read a record's `depth` before designing on it.** `resolved-config` proves the provider _read_ the field, not that it acts on it — OpenCode collapses an unrecognized variant at request-build time, well after `opencode debug agent` has printed it. Only `outbound-request` reaches the model, and no Cursor probe can ever get there. The contract's Depth section states what each value licenses.

**A capability accessor is a claim about a provider, so a probe is the thing to consult before changing one.** `fully_implements_canonical_output` and `session_start_fires_on_resume` name the probes their values came from, which is worth doing where a value was measured — but the manifest's `expected` is the binding record of that belief, not the doc comment. When a probe refutes one, correct whatever encoded the old belief _and_ the manifest's `expected` — see the contract's "life of a refutation," which exists because leaving them disagreeing is the drift this harness was built to stop.

**Probe first when the outcome could change the design; defer when it can only confirm.** agentspec's tests compare emitted bytes against agentspec's own belief about what each provider reads, and that belief has been wrong — a design revision once asserted an OpenCode `provider/model#variant` suffix no parser would have accepted.

All three providers degrade silently on a rendering they do not understand, so a probe asserts on a positive signal in the provider's own resolved view, never on absence of an error. The full contract is `experiments/README.md`.

Probes are explicitly invoked and **never run in CI** — what goes stale is the third-party tool, not any file in this repository.

## Design Principles

### Colocate code with its consumer

Place functions in the module that calls them, not in a module named after an abstract category. If a function has one caller, it belongs next to that caller — even if it "sounds like" it belongs elsewhere. A function called `resolve_dest_dir` that's only used by `sync_plan` belongs in `sync.rs`, not in a separate `provider.rs`.

When a function is private to its module, colocating it also minimizes the public API surface — fewer `pub` items, fewer cross-module imports, less indirection to discover.

### Provider-specific logic belongs in adapters

Adapters own all provider-specific content decisions: which frontmatter fields to set, how to serialize them, how to construct output paths, and how to apply transforms like name prefixing or stripping. Emit should never need to know about provider-specific frontmatter structure.

If adding support for a new provider, implement one adapter — no changes to emit, plan, or sync should be needed.

Note that `src/adapters/` contains both adapter implementations (`claude.rs`, `cursor.rs`, `opencode.rs`) and shared helpers (`hook_compile.rs`, `hooks_merge.rs`, the `Adapter` trait in `adapters.rs`). The "provider-specific logic belongs in adapters" property applies to the implementations; the shared helpers are agentspec-pipeline code that takes `Provider` as a parameter without owning provider knowledge. See `.claude/rules/provider-logic-in-adapters.md` for the full distinction with concrete examples.

### Operate on structs, not serialized strings

Never parse or transform YAML/frontmatter by manipulating serialized strings (line-by-line scanning, regex, etc.). Instead, modify typed structs before serialization. If a field needs to be conditionally included, make it `Option<T>` on the struct and let serde handle it via `#[serde_with::skip_serializing_none]`.

### Use config structs at module boundaries

When passing configuration across module boundaries (especially from the binary crate into the library crate), use a named struct — not loose parameters, tuples, or the raw `AgentspecConfig`. This pattern is established by `TemplatingResources`, `SpecDirs`, and `AdapterConfig`.

Key conventions:

- **Library-side structs** (`TemplatingResources`, `SpecDirs`, `AdapterConfig`) have no dependency on clap, serde, or the binary crate's config types. The binary constructs them from `AgentspecConfig`.
- **`Option<&Config>` means "use defaults"** — when a config is optional (like `AdapterConfig` for adapters), pass `Option<&Config>` where `None` produces canonical/default output.
- **Centralize construction** — if multiple call sites build the same config from the same source, extract a helper (e.g., `config.adapter_configs()`).

### Model provider settings as typed fields, never as passthrough

A provider setting gets its own config field with its own type. It never rides inside a field that means something else — a model id, a URL, a command string. Where a provider itself encodes a setting inside another value (Cursor's `model[effort=high]`), agentspec becomes the sole writer of that format and rejects hand-composed input at validate time.

When agentspec cannot name every option a provider accepts, the remainder gets its **own explicit field** — `CursorPreset.params`, validated under the same delimiter and collision rules — never a re-permitted passthrough in the modelled field. `params` is an escape hatch, but a modelled one: agentspec still writes every byte of the composed value. What is ruled out is the unparsed field.

The reason is silent failure. Two spellings of one setting compose into output no provider parses, and all three providers degrade silently rather than erroring — Cursor falls back to the parent conversation's model — so the symptom is an agent running on an unrelated model with a clean `agentspec validate`.

One heuristic falls out of getting this wrong once: **when a provider documents a value space as runtime-discoverable, check whether its key space is open too.** See `.claude/rules/design-principles.md` for the worked example.

## Pipeline Stages

`main.rs` orchestrates these stages in order, each consuming the previous stage's output (typestate pattern — passing the wrong stage is a compile error):

1. **Load** — `specs.rs` (`Specs::load`) reads `.md` files from `spec/agents/`, `spec/skills/`, `spec/rules/`, and `spec/hooks/hooks.toml` (plus a recursive walk of `spec/hooks/scripts/`), parses frontmatter via `gray_matter` (markdown specs) and `serde_path_to_error` over `toml` (hooks) into typed structs → `(Specs, LoadReport)`. Files (and subtrees) matching any pattern in `[spec].ignore` are skipped before frontmatter parsing via `WalkDir::filter_entry` pruning; the `LoadReport` records what was filtered and which patterns matched zero files so `main.rs` can surface warnings and listings. The recursive walks of `spec/skills/<id>/` and `spec/hooks/scripts/` follow symlinks (`WalkDir::follow_links(true)`), resolving them to their target content at load time and emitting that content as regular files in compiled output. Symlink targets must fall within `sources_dir` (validated via `fs::canonicalize`); dangling symlinks, out-of-tree targets, and symlink loops all produce clear compile-time errors. Cross-platform note: macOS/Linux are supported; Windows users on git clones with `core.symlinks = false` will see symlinks as regular files containing target paths (not handled).
2. **Validate** — `specs.rs` (`Specs::validate`) runs semantic checks via `validate::validate_semantics` (duplicate IDs, unknown presets, hook event/matcher compatibility, etc.) → `ValidatedSpecs`. `Spec` is the single canonical spec type — there is no separate normalized representation.
3. **Compile** — `compile.rs` resolves MiniJinja templates (rendering built-in variables like `specs` and `{% include %}` fragment references) and then makes one dispatch per provider through `Adapter::compile(specs, ctx) -> AdapterOutput { files, patches, dest_root, degradations, deliveries }`. Per-spec adaptation and cross-spec aggregation (per-provider `hooks.json` synthesis, per-provider hook-shim emission via `hook_compile.rs::build_shim_files` called from inside `synthesize_hooks` alongside `build_hook_script_files`, `OpenCode` `instructions[]` registration, etc.) are sequenced inside the adapter — the orchestrator does not re-implement two-phase ordering. `CompileCtx` carries `Option<&AdapterConfig>` (`prefix` controls file paths and frontmatter, `content_prefix` controls model-facing names) plus the sync mode, dest root inputs, presets, and overwrite flag. `CompileResult` collects per-provider `files`, `patches: HashMap<Provider, Vec<Box<dyn ForwardPatch>>>`, and `dest_roots: HashMap<Provider, PathBuf>`. Template resolution is an internal step of compilation, not a separate pipeline stage. Adapters produce fully-formed output (paths, frontmatter, content, post-write patches) — no post-hoc transforms downstream. Each adapter records what it _carried_ — one `Delivery` per setting that reached an emitted file, taken from the frontmatter struct it serialized — and declares what it _can_ carry, as the static `carriable(FileKind) -> &[SettingKind]` table. `compile_specs` derives the losses by subtraction: it builds an intent set from what the author configured (per spec for `SettingKey::Body`, per emitted file kind for every other setting, so an `OpenCode` command file carrying `model` does not mask the same spec's skill file dropping it), subtracts the deliveries, and collects the remainder into a `BTreeSet<Loss>`. No adapter can construct a `Loss` and `compile_specs` cannot construct a `Delivery`, so the arithmetic is the only route between the two. `Degradation` survives, narrowed to provider limitations — claims about a provider's runtime acting on bytes agentspec delivered successfully, which no comparison of intent against deliveries could produce. `Degradation`'s, `Delivery`'s, and `Loss`'s constructors are each module-private to the module that owns them, so the post-loop spec re-scan this replaced cannot return. `compile_specs` retains a single gate of its own, the `ParityWarning` that reads `Adapter::session_start_fires_on_resume`, because it compares adapters against each other rather than describing any one of them. The shim's runtime contract and the canonical payload schema are documented in [`docs/hooks-canonical.md`](docs/hooks-canonical.md).
4. **Plan** — Three sibling constructors each build their own typed plan from `CompileResult` and config: `agentspec::plan::compile_plan → CompilePlan` (library-side, for `compile`), the binary crate's `sync::sync_plan → SyncPlan` (for `sync`), and the binary crate's `remove::remove_plan → RemovePlan` (for `remove`). Each plan carries only the fields its mode actually uses — `CompilePlan` holds `Vec<CleanSlateWrite>`, `SyncPlan` holds `Vec<ManifestTrackedWrite>` plus `Vec<Box<dyn ForwardPatch>>` post-write patches, `RemovePlan` holds `Vec<RemoveWrite>` plus reverse-direction patches. `sync_plan` drains pre-built patches per provider from `CompileResult.patches`; `remove_plan` calls `Adapter::removal_patches(&RemoveCtx) -> RemovalOutput { dest_root, patches }` and skips spec compilation — it only consults the manifest at execution time.
5. **Emit** — `emit.rs` exposes one entry point per plan type, each consuming only its corresponding plan: `emit_compile(&CompilePlan, dry_run)` deletes-and-rewrites `generated/<provider>/`; `emit_sync(&SyncPlan, dry_run, verbose)` does per-file ownership tracking, stale cleanup, and direct writes to tool config dirs; `emit_remove(&RemovePlan, dry_run)` deletes every manifest-recorded file, deletes the manifest, prunes empty agentspec-created subdirs, and rmdir's the dest dir if empty. `emit_sync` and `emit_remove` then run adapter-provided post-write hooks (`OpenCode` `opencode.json` patching, `ClaudeHooksPatch`/`CursorHooksPatch` CST-aware merge into `settings.json`/`hooks.json` for hooks at User/Project sync mode — backed by `jsonc-parser` so comments, trailing commas, and user-authored entries round-trip unchanged; the inverse `ClaudeRemoveHooksPatch` and `CursorRemoveHooksPatch` strip `_agentspec_id`-tagged entries from `settings.json`/`hooks.json`, while `OpenCodeRemoveInstructionsPatch` filters `instructions[]` by `rules_dest_dir` prefix; all three tidy emptied containers without ever deleting the host file). `emit_compile` carries no post-write hooks — `CompilePlan` has no field for them. Emit is purely file I/O, manifest tracking, and hook execution — no content transformation or provider-specific logic of its own.

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
