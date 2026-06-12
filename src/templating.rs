mod context;
mod environment;
mod fragments;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
pub use context::TemplateContext;
use environment::build_environment;
pub use fragments::resolve_fragments;
use fragments::{load_all_fragments, load_templates};
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
#[derive(Debug)]
pub struct Templating {
    fragment_map: HashMap<String, String>,
    template_map: HashMap<String, String>,
}

impl Templating {
    /// Load fragment and template files, detecting name collisions between them.
    pub fn load(
        fragments_dir: &Path,
        extra_fragment_dirs: &[PathBuf],
        templates_dir: &Path,
    ) -> Result<Self> {
        let fragment_map = load_all_fragments(fragments_dir, extra_fragment_dirs)?;
        let template_map = load_templates(templates_dir)?;

        for key in template_map.keys() {
            if fragment_map.contains_key(key) {
                let rel = key.strip_prefix("templates/").unwrap_or(key);
                bail!(
                    "template-fragment name collision: \"{key}\"\n  \
                     --> template at {}\n  \
                     --> a fragment also resolves to the key \"{key}\"\n  \
                     = rename one to disambiguate",
                    templates_dir.join(rel).display(),
                );
            }
        }

        Ok(Self {
            fragment_map,
            template_map,
        })
    }

    /// Build a `MiniJinja` environment for `spec` with all loaded fragments
    /// and templates available. See [`environment::build_environment`] for the
    /// full contract, including `script()` gating.
    pub fn build_environment(
        &self,
        provider: Option<Provider>,
        spec: &Spec,
    ) -> Result<Environment<'_>> {
        build_environment(&self.fragment_map, &self.template_map, provider, spec)
    }

    #[cfg(test)]
    pub(crate) fn from_fragments(fragment_map: HashMap<String, String>) -> Self {
        Self {
            fragment_map,
            template_map: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_fragments_and_templates(
        fragment_map: HashMap<String, String>,
        template_map: HashMap<String, String>,
    ) -> Self {
        Self {
            fragment_map,
            template_map,
        }
    }
}
