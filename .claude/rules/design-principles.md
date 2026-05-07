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
