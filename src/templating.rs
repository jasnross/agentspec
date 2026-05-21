mod context;
mod environment;
mod fragments;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
pub use context::TemplateContext;
use environment::build_environment;
use fragments::load_all_fragments;
pub use fragments::resolve_fragments;
use minijinja::Environment;

use crate::provider::Provider;
use crate::spec::Spec;

/// Reusable templating infrastructure: owns the fragment map and builds
/// `MiniJinja` environments on demand.
///
/// This replaces the former `TemplatingConfig` struct. Unlike `TemplatingConfig`,
/// which bundled fragments with a pre-built context, `TemplatingResources` owns
/// only the reusable fragment data. The `TemplateContext` is built at the point
/// of use (inside the compile loop) where provider-specific information is
/// available.
pub struct Templating {
    fragment_map: HashMap<String, String>,
}

impl Templating {
    /// Load fragment files from the local directory plus any extra directories.
    pub fn load(fragments_dir: &Path, extra_fragment_dirs: &[PathBuf]) -> Result<Self> {
        let fragment_map = load_all_fragments(fragments_dir, extra_fragment_dirs)?;
        Ok(Self { fragment_map })
    }

    /// Build a `MiniJinja` environment for `spec` with all loaded fragments
    /// available as templates. See [`environment::build_environment`] for the
    /// full contract, including `script()` gating.
    pub fn build_environment(
        &self,
        provider: Option<Provider>,
        spec: &Spec,
    ) -> Result<Environment<'_>> {
        build_environment(&self.fragment_map, provider, spec)
    }

    #[cfg(test)]
    pub(crate) fn from_fragments(fragment_map: HashMap<String, String>) -> Self {
        Self { fragment_map }
    }
}
