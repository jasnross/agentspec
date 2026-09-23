pub mod adapters;
pub mod compile;
pub mod cst_io;
pub mod hooks_canonical;
pub mod hooks_merge;
pub mod plan;
pub mod presets;
pub mod provider;
pub mod setting;
pub mod spec;
pub mod specs;
// Crate-private: every item is `pub(crate)` filesystem plumbing, with no
// consumer outside this crate.
mod symlink;
pub mod templating;
pub mod validate;
