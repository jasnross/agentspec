# Style Guidelines

## Module Files Over `mod.rs`

Prefer the modern Rust module file convention. Declare the module root as `module_name.rs` and place submodules in a `module_name/` directory alongside it. Avoid `mod.rs`.

### Bad

```
src/client/mod.rs
src/client/remote.rs
src/client/auth.rs
```

### Good

```
src/client.rs
src/client/remote.rs
src/client/auth.rs
```

## Public Before Private

Public items are the API surface — what callers see and depend on. Private items are implementation details. Placing public items first means a reader scanning from the top encounters the interface before the internals, the same way they would read documentation.

### Bad

```rust
impl Client {
    fn build_url(&self, id: u64) -> String { ... }
    fn parse_response(&self, body: &str) -> Result<Record> { ... }

    pub fn fetch(&self, id: u64) -> Result<Record> { ... }
    pub fn save(&self, record: &Record) -> Result<()> { ... }
}
```

### Good

```rust
impl Client {
    pub fn fetch(&self, id: u64) -> Result<Record> { ... }
    pub fn save(&self, record: &Record) -> Result<()> { ... }

    fn build_url(&self, id: u64) -> String { ... }
    fn parse_response(&self, body: &str) -> Result<Record> { ... }
}
```

## Avoid Section Comments

Avoid using comments to organize code. Instead order the code in a logical order, grouping related things together via naming. Consider splitting out a child module if warranted.

### Bad

```rust
impl Store {
    // --- Entry operations ------------------------------------------------

    pub fn load(path: &Path) -> Result<Self> { ... }
    pub fn save(&self, path: &Path) -> Result<()> { ... }
    pub fn add(&mut self, entry: Entry) -> Result<()> { ... }
    pub fn remove(&mut self, id: &str) -> bool { ... }
    pub fn list(&self) -> &[Entry] { ... }

    // --- Group operations ------------------------------------------------

    pub fn add_to_group(&mut self, group: &str, ids: &[String]) -> ... { ... }
    pub fn remove_group(&mut self, group: &str) -> usize { ... }
    pub fn list_groups(&self) -> BTreeMap<String, Vec<&Entry>> { ... }
    pub fn entries_for_group(&self, name: &str) -> Vec<&Entry> { ... }
}
```

### Good

The `_group` suffix already communicates the grouping — no dividers needed:

```rust
impl Store {
    pub fn load(path: &Path) -> Result<Self> { ... }
    pub fn save(&self, path: &Path) -> Result<()> { ... }
    pub fn add(&mut self, entry: Entry) -> Result<()> { ... }
    pub fn remove(&mut self, id: &str) -> bool { ... }
    pub fn list(&self) -> &[Entry] { ... }
    pub fn add_to_group(&mut self, group: &str, ids: &[String]) -> ... { ... }
    pub fn remove_group(&mut self, group: &str) -> usize { ... }
    pub fn list_groups(&self) -> BTreeMap<String, Vec<&Entry>> { ... }
    pub fn entries_for_group(&self, name: &str) -> Vec<&Entry> { ... }
}
```

When sections represent genuinely distinct responsibilities, split into a child module instead of reaching for dividers.

### Bad

```rust
// client.rs

// --- URL parsing -----------------------------------------

pub fn parse_endpoint(url: &str) -> Option<Endpoint> { ... }
fn normalize_url(url: &str) -> Option<(String, String)> { ... }
pub fn resolve_endpoints(entries: &[&Entry]) -> ... { ... }

// --- Token resolution ------------------------------------

pub trait TokenResolver { ... }
pub struct EnvTokenResolver;
fn resolve_tokens(...) { ... }

// --- HTTP executor ---------------------------------------

pub trait HttpExecutor: Send + Sync { ... }
pub struct UreqExecutor { ... }
fn api_endpoint_for(host: &str) -> String { ... }

// --- Query execution -------------------------------------

struct ResponseNode { ... }
fn query_host(...) { ... }
pub fn fetch_records(...) { ... }
```

### Good

```rust
// client.rs
mod remote;   // URL parsing and endpoint resolution
mod auth;     // token resolution
mod http;     // HttpExecutor trait and implementation
mod query;    // query types, pagination, and fetch_records
```

With submodule files alongside the module root:

```
src/client.rs
src/client/remote.rs
src/client/auth.rs
src/client/http.rs
src/client/query.rs
```
