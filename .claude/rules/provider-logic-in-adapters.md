# Provider-Specific Logic Belongs in Adapters

Adapters own every provider-specific decision: which frontmatter fields to set, how to serialize them, what output paths look like, which file kinds the provider supports, how its config is patched, and how its hook JSON is shaped. Code outside `src/adapters/` should treat providers as opaque — iterate over them, dispatch to them, but never branch on which one it is.

A useful test when writing a function: **could a new provider be added by writing only its adapter?** If the answer requires touching `compile.rs`, `sync.rs`, `plan.rs`, `emit.rs`, or `hooks_merge.rs`, the abstraction is leaking and the function in question is the leak.

## Rules of thumb

- **`match provider { ... }` outside `src/adapters/` is a smell.** The arms almost always encode provider knowledge — directory names, supported file kinds, hook JSON shape, post-write behavior — that belongs in the adapter.
- **Provider name literals (`".claude"`, `"cursor"`, `"opencode"`) outside adapters are a smell.** If a path or string is provider-specific, the adapter should produce it.
- **Iterating providers is fine.** `Provider::VARIANTS`, `for provider in providers`, and `providers.map(...)` are all correct shapes — the violation is what happens _inside_ the loop body.
- **Tests are exempt.** Integration tests and unit tests routinely name a specific provider to set up a scenario; that's not a leak.

## Hardcoded paths via match

### Bad

Real example from `src/plan.rs:75-91` — directory names are baked into a non-adapter module:

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

Once an adapter trait exists (TODO #5), the dispatch arms collapse to `provider.adapter().user_dest_dir(home, kind)` and `plan.rs` no longer mentions specific providers at all. Until the trait lands, this dispatch-only `match` is acceptable: it knows _that_ there are three providers but nothing about _what_ they do.

## Hardcoded behavior via match

### Bad

Real example from `src/sync.rs:59-69` — a `match` block that calls three near-identically-shaped per-adapter functions:

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

```rust
let hook = provider.adapter().post_write_hook(
    kind, &dest, &config_dir, emit_mode, owned_entries,
);
```

The dispatch is a one-liner and `sync.rs` stops naming individual providers entirely.

## Hardcoded provider capabilities

### Bad

Real example from `src/plan.rs:54-69` — which file kinds each provider supports is encoded in `plan.rs`:

```rust
pub fn file_kinds(provider: Provider) -> Vec<FileKind> {
    match provider {
        Provider::Claude | Provider::Cursor => vec![
            FileKind::Agents, FileKind::Rules, FileKind::Skills, FileKind::Hooks,
        ],
        Provider::OpenCode => vec![
            FileKind::Agents, FileKind::Commands, FileKind::Rules, FileKind::Skills,
        ],
    }
}
```

OpenCode supports `Commands`. Claude and Cursor support `Hooks`. That's adapter knowledge.

### Good

```rust
// src/adapters/claude.rs
pub fn file_kinds() -> &'static [FileKind] {
    &[FileKind::Agents, FileKind::Rules, FileKind::Skills, FileKind::Hooks]
}

// src/plan.rs
pub fn file_kinds(provider: Provider) -> &'static [FileKind] {
    match provider {
        Provider::Claude => adapters::claude::file_kinds(),
        Provider::Cursor => adapters::cursor::file_kinds(),
        Provider::OpenCode => adapters::opencode::file_kinds(),
    }
}
```

Same pattern — `plan.rs` keeps a thin dispatch arm but no longer encodes which file kinds belong to which provider.

## When dispatch is unavoidable

Sometimes the calling module legitimately needs to dispatch by provider — that's what an adapter trait is for. Until TODO #5 lands, dispatch-only `match provider` blocks (where every arm calls the same-shaped per-adapter function) are an acceptable interim shape. What's never acceptable: a `match provider` block where each arm contains _different logic_, _different field names_, _different paths_, or _different JSON shapes_. That's not dispatch; that's provider knowledge embedded in the wrong file.

If you find yourself writing `if provider == Provider::X` to special-case behavior, stop and add the capability to the adapter (or its trait) instead.
