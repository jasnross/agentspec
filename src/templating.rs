mod context;
mod fragments;

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
pub use context::TemplateContext;
pub use fragments::resolve_fragments;
use fragments::{build_environment, load_fragments};
use minijinja::Environment;

use crate::provider::Provider;

/// Reusable templating infrastructure: owns the fragment map and builds
/// `MiniJinja` environments on demand.
///
/// This replaces the former `TemplatingConfig` struct. Unlike `TemplatingConfig`,
/// which bundled fragments with a pre-built context, `TemplatingResources` owns
/// only the reusable fragment data. The `TemplateContext` is built at the point
/// of use (inside the compile loop) where provider-specific information is
/// available.
pub struct TemplatingResources {
    fragment_map: HashMap<String, String>,
}

impl TemplatingResources {
    /// Load fragment files from the given directory.
    pub fn load(fragments_dir: &Path) -> Result<Self> {
        let fragment_map = load_fragments(fragments_dir)?;
        Ok(Self { fragment_map })
    }

    /// Build a `MiniJinja` environment with all loaded fragments available as
    /// templates. The returned environment borrows from `self`, so it cannot
    /// outlive the `TemplatingResources`.
    ///
    /// When `provider` is `Some`, the environment's `tool()` function resolves
    /// canonical tool names to provider-specific body-level names. When `None`
    /// (e.g., during `agentspec validate`), `tool()` passes the canonical name
    /// through unchanged after verifying it is a known tool.
    pub fn build_environment(&self, provider: Option<Provider>) -> Result<Environment<'_>> {
        build_environment(&self.fragment_map, provider)
    }
}
