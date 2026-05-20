# Validation Locality

## Check config constraints at load time, not at the use site

When a constraint on parsed config can be checked at config-load or validation time, do that check there — not at the point where the config value is consumed. Use-site checks are defense-in-depth; the user-facing error is the load-time one.

### Where each kind of check belongs

- **Per-field constraints** (types, required-vs-optional, enum variants): serde parsing. A malformed field should fail deserialization, not silently pass through as a default.
- **Cross-field constraints** (if field A is X then field B must be Y): `validate_for_provider` on `SyncTargetConfig`, or `validate_semantics` for spec-level constraints. These run early enough that `agentspec validate` surfaces them.
- **Use-site checks**: `debug_assert!` or `unreachable!` for cases the load-time gate already covers. These catch wiring bugs in debug builds without duplicating user-facing error messages.

### Bad

A cross-field constraint that fires mid-pipeline:

```rust
// sync.rs — deep inside sync_plan
fn resolve_dest_dir(config: &SyncTargetConfig, ...) -> Result<PathBuf> {
    if config.mode == SyncMode::Plugin && config.dir.is_none() {
        bail!("plugin mode requires dir");  // user sees this only at sync time
    }
    // ...
}
```

### Good

The constraint fires at validation time; the use site is defense-in-depth:

```rust
// config.rs — called from `agentspec validate` and sync/remove target resolution
pub fn validate_for_provider(&self, provider: Provider) -> Result<()> {
    if self.mode == SyncMode::Plugin && self.dir.is_none() {
        bail!("[sync.{provider}] requires `dir` when mode is plugin");
    }
    Ok(())
}

// sync.rs — use site trusts the validation gate
fn resolve_dest_dir(config: &SyncTargetConfig, ...) -> PathBuf {
    debug_assert!(config.dir.is_some(), "should have been caught by validate_for_provider");
    // ...
}
```
