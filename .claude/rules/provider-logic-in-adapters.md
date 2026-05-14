# Provider-Specific Logic Belongs in Adapters

Adapters own every provider-specific decision: which frontmatter fields to set, how to serialize them, what output paths look like, which file kinds the provider supports, how its config is patched, and how its hook JSON is shaped. Code outside `src/adapters/` should treat providers as opaque — iterate over them, dispatch to them, but never branch on which one it is.

A useful test when writing a function: **could a new provider be added by writing only its adapter?** If the answer requires touching `compile.rs`, `sync.rs`, `plan.rs`, `emit.rs`, or `hooks_merge.rs`, the abstraction is leaking and the function in question is the leak.

## Rules of thumb

- **`match provider { ... }` outside `src/adapters/` is a smell.** The arms almost always encode provider knowledge — directory names, supported file kinds, hook JSON shape, post-write behavior — that belongs in the adapter.
- **Provider name literals (`".claude"`, `"cursor"`, `"opencode"`) outside adapters are a smell.** If a path or string is provider-specific, the adapter should produce it.
- **Iterating providers is fine.** `Provider::VARIANTS`, `for provider in providers`, and `providers.map(...)` are all correct shapes — the violation is what happens _inside_ the loop body.
- **Tests are exempt.** Integration tests and unit tests routinely name a specific provider to set up a scenario; that's not a leak.

## Shared helpers under `src/adapters/` vs. adapter implementations

`src/adapters/` contains two kinds of files, and the "provider-specific logic belongs in adapters" rule applies differently to each:

- **Adapter implementations** (`claude.rs`, `cursor.rs`, `opencode.rs`): one file per provider, each containing the `impl Adapter for X { ... }` block plus the closures and helpers that encode that provider's specifics — frontmatter shapes, JSON wrapping, manifest emission, post-write patches. These files own provider knowledge.
- **Shared helpers** (`hook_compile.rs`, `hooks_merge.rs`, the `Adapter` trait in `src/adapters.rs`): agentspec-pipeline code that adapters call. These take `Provider` as a parameter (or accept adapter-supplied closures) and produce per-provider output, but they don't _own_ provider knowledge — they dispatch on inputs supplied by the calling adapter or the orchestrator.

A useful litmus test: **does this code change require modifying `src/adapters/<provider>.rs`?**

- **Yes** → the work is provider-implementation-specific. Put the logic in the adapter file.
- **No, but the work is provider-parameterized** → it belongs in a shared helper. The shared helper can take `Provider` as a parameter and produce per-provider output; that's not provider knowledge leaking, it's normal dispatch on a parameter.
- **No, and the work is provider-agnostic** → it belongs further up the pipeline (`compile.rs`, `sync.rs`, `plan.rs`, etc.) and should treat providers as opaque.

The distinction matters because "code inside `src/adapters/`" is not synonymous with "adapter code." The shared helpers under `src/adapters/` are agentspec-pipeline machinery that lives there for organizational proximity to the adapter implementations they support, not because they're owned by any specific adapter.

### Concrete examples from current code

- `build_hook_script_files(provider, specs) -> Vec<GeneratedFile>` in `hook_compile.rs`: shared helper. Takes `Provider` as a parameter, emits per-provider files, contains no `match provider { ... }` block with different code per arm. Adding a sibling helper of the same shape (e.g., a future `build_shim_files(provider, specs)`) is also shared-helper work, not adapter-implementation work.
- `hook_command_anchor(plugin_root_env_var, ...)` in `hook_compile.rs`: shared helper. Takes the per-provider plugin-root env var name as a `&'static str` supplied by the calling adapter; produces a command string. The string varies by provider only because the parameter varies — the helper itself contains no provider knowledge.
- `merge_owned(...)` in `hooks_merge.rs`: shared helper. Takes closures supplied by the per-adapter `ConfigPatch` impl; the closures encapsulate the provider-specific JSON-shape decisions. The helper itself does file I/O and CST plumbing.
- `Adapter::adapt_hook_spec` impl in `src/adapters/claude.rs`: adapter implementation. Contains Claude-specific JSON shape and field names. Provider knowledge lives here, not in shared helpers.

### Common failure modes

- **Reading "adapter's `synthesize_hooks`" and concluding `synthesize_hooks` is an adapter method.** `synthesize_hooks` is a shared helper in `hook_compile.rs` that adapters _call_. Each provider's adapter passes its own parameters (`dotdir`, `plugin_root_env_var`, closures), but the helper itself has no provider knowledge to own.
- **Concluding that taking `Provider` as a parameter is automatically a rule violation.** It isn't — dispatch on a parameter is fine. The violation would be `match provider` arms with different logic, different field names, or different output shapes.
- **Putting agentspec-pipeline logic in adapter implementations because "it touches provider-specific output."** Provider-specific _content_ belongs in the adapter; cross-cutting _orchestration_ belongs in the shared helpers that all adapters call. If two adapter implementations would have nearly-identical code for the same orchestration, that's the shared-helper case.

## Hardcoded paths via match

### Bad

Directory names baked into a non-adapter module:

```rust
pub fn user_dest_dir(provider: Provider, kind: FileKind, home: &Path) -> PathBuf {
    match provider {
        Provider::Claude => home.join(".claude").join(kind.dir_name()),
        Provider::Cursor => home.join(".cursor").join(kind.dir_name()),
        Provider::OpenCode => home.join(".config").join("opencode").join(kind.dir_name()),
    }
}
```

Adding a fourth provider means editing `plan.rs`. The directory name (`.claude`, `.cursor`, `.config/opencode`) is provider knowledge that lives outside the adapter.

### Good

Push the path-shape decision into each adapter and let `plan.rs` ask:

```rust
// src/adapters/claude.rs
pub fn user_dest_dir(home: &Path, kind: FileKind) -> PathBuf {
    home.join(".claude").join(kind.dir_name())
}

// src/plan.rs
pub fn user_dest_dir(provider: Provider, kind: FileKind, home: &Path) -> PathBuf {
    match provider {
        Provider::Claude => adapters::claude::user_dest_dir(home, kind),
        Provider::Cursor => adapters::cursor::user_dest_dir(home, kind),
        Provider::OpenCode => adapters::opencode::user_dest_dir(home, kind),
    }
}
```

The `Adapter` trait collapses these dispatch arms entirely: each provider exposes `compile(specs, ctx) -> AdapterOutput` and the orchestrator calls `provider.adapter().compile(...)` once per provider. `plan.rs` no longer names specific providers at all.

## Hardcoded behavior via match

### Bad

A `match` block that calls near-identically-shaped per-adapter functions:

```rust
let hook = match *provider {
    Provider::Claude => {
        claude_post_write_hook(kind, &dest, &config_dir, emit_mode, owned_entries)
    }
    Provider::Cursor => {
        cursor_post_write_hook(kind, &dest, &config_dir, emit_mode, owned_entries)
    }
    Provider::OpenCode => {
        opencode_post_write_hook(kind, &dest, &config_dir, emit_mode, owned_entries)
    }
};
```

The signatures are identical; only the function name varies by provider. This is the textbook case for a trait.

### Good

Adapter-built patches flow back through `Adapter::compile`'s return value as `Vec<Box<dyn ConfigPatch>>` — the orchestrator never asks for them by name:

```rust
let output = provider.adapter().compile(&specs, &ctx)?;
files.extend(output.files);
patches.entry(provider).or_default().extend(output.patches);
```

`compile.rs`, `sync.rs`, and `remove.rs` stop naming individual providers entirely.

## Hardcoded provider capabilities

### Bad

Which file kinds each provider supports, encoded as a behavior switch:

```rust
fn supports_hooks(provider: Provider) -> bool {
    matches!(provider, Provider::Claude | Provider::Cursor)
}
```

That's adapter knowledge masquerading as a `plan.rs`/`compile.rs` concern.

### Good

Capability accessors on the trait — adapters declare what they support; callers iterate without branching on `Provider`:

```rust
// src/adapters/claude.rs
impl Adapter for ClaudeAdapter {
    fn emits_hooks(&self) -> bool { true }
    // ...
}

// src/compile.rs
if !provider.adapter().emits_hooks() {
    diagnostics.skipped_hooks.push(...);
}
```

Same pattern — capability lookup via the trait, not a `match provider`.

## When dispatch is unavoidable

Sometimes the calling module legitimately needs to dispatch by provider — that's what an adapter trait is for. Until that trait lands, dispatch-only `match provider` blocks (where every arm calls the same-shaped per-adapter function) are an acceptable interim shape. What's never acceptable: a `match provider` block where each arm contains _different logic_, _different field names_, _different paths_, or _different JSON shapes_. That's not dispatch; that's provider knowledge embedded in the wrong file.

If you find yourself writing `if provider == Provider::X` to special-case behavior, stop and add the capability to the adapter (or its trait) instead.
