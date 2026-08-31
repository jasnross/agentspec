# Design Principles

## Operate on Structs, Not Serialized Strings

Never parse or transform serialized data (TOML, JSON, YAML) by manipulating strings. Modify typed structs before serialization. If a field needs to be conditionally included, make it `Option<T>` on the struct and let serde handle it.

### Bad

```rust
let mut raw = toml::to_string(&config)?;
raw = raw.replace("old_key = true", "");  // brittle string surgery
```

### Good

```rust
config.old_key = None;  // serde skips None fields with skip_serializing_if
let raw = toml::to_string(&config)?;
```

## Colocate Code With Its Consumer

Place functions in the module that calls them, not in a module named after an abstract category. If a function has one caller, it belongs next to that caller.

### Bad

```rust
// utils.rs — grab-bag with one caller each
pub fn parse_timestamp(s: &str) -> Result<DateTime> { ... }  // only called by fetcher.rs
pub fn normalize_url(s: &str) -> String { ... }              // only called by client.rs
```

### Good

```rust
// fetcher.rs
fn parse_timestamp(s: &str) -> Result<DateTime> { ... }

// client.rs
fn normalize_url(s: &str) -> String { ... }
```

## Separate Data Collection From Presentation

Keep the code that gathers and computes data separate from the code that formats and renders it. Data collection produces typed structs; presentation consumes them. This boundary is the seam where alternative output modes — JSON, CSV, machine-readable — can plug in without touching collection logic.

### Bad

```rust
pub fn run(entries: &[Entry]) {
    for entry in entries {
        let state = entry.load();
        println!("{}: {}", entry.name, state.to_display_string());
    }
}
```

### Good

```rust
// collector.rs
pub fn collect(entries: &[Entry]) -> Vec<EntryState> { ... }

// presenter.rs
pub fn format_table(states: &[EntryState]) -> String { ... }
```

## Model Provider Settings as Typed Fields, Never as Passthrough

A provider setting gets its own config field with its own type. It never rides inside a field that means something else — a model id, a URL, a command string, a shell invocation.

Where a provider itself encodes a setting inside another value, agentspec becomes the sole writer of that composed format and rejects hand-composed input at validate time. Cursor is the standing case: it spells model options as a bracket suffix on the model id (`claude-opus-5[effort=high]`), so agentspec composes the bracket from typed preset fields and refuses a `model` that already carries one.

When agentspec cannot name every option a provider accepts, the remainder gets its **own explicit field**, validated under the same rules. It never gets handled by re-permitting passthrough in the modelled field.

### Why

Two spellings of one setting compose silently. A hand-written `model = "claude-opus-5[effort=high]"` beside a typed `effort = "medium"` renders `claude-opus-5[effort=high][effort=medium]` — and every provider agentspec targets degrades silently on a value it cannot parse. Cursor falls back to the parent conversation's model with no error, so the observable symptom is an agent running on an unrelated model, with a clean `agentspec validate` and no diagnostic anywhere in the chain.

### Bad

A modelled field that also accepts raw provider syntax:

```rust
pub struct CursorPreset {
    /// Accepts a bare id, or an id with `[k=v]` options appended.
    pub model: Option<String>,
    pub effort: Option<String>,
}
```

Anything agentspec has no field for is reachable, but so is a second spelling of everything it does — and the two compose into output no provider parses.

### Good

One modelled field per option, plus a dedicated field for the rest:

```rust
pub struct CursorPreset {
    /// A bare model id. `validate` rejects `[`, `]`, `,`, and `=`.
    pub model: Option<String>,
    pub effort: Option<String>,
    pub fast: Option<bool>,
    pub context: Option<String>,
    /// Options with no named field. A key duplicating a named field is
    /// rejected, so an option still has exactly one spelling.
    pub params: BTreeMap<String, String>,
}
```

`params` is an escape hatch, but a modelled one: agentspec still writes every byte of the bracket, and the same delimiter and collision rules apply to it as to the named fields. What is ruled out is the _unparsed_ field — "write whatever you like here and we will pass it through."

### The heuristic that produces this mistake

**When a provider documents a _value_ space as runtime-discoverable, check whether its _key_ space is open too.** Cursor's is: its SDK documentation says parameter **ids** as well as values vary by model, and the catalog is account- and team-specific. An earlier revision of the execution-preset design read only the first half — typing `effort` as a free `String` because the values were open, while assuming the set of option _ids_ was closed and trackable by agentspec's release cadence. On that assumption an unconditional ban on hand-written brackets deletes capability instead of relocating it. `params` exists because the assumption was false.
