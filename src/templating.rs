mod context;
mod fragments;

use std::path::PathBuf;

use anyhow::Result;
use fragments::{build_environment, load_fragments, resolve_fragments};

use crate::spec::NormalizedSpec;
use crate::specs::ValidatedSpecs;
pub use context::TemplateContext;

/// Configuration for the template resolution pass.
///
/// Kept separate from the binary's `AgentspecConfig` so the library crate
/// has no dependency on binary-specific config types.
pub struct TemplatingConfig {
    pub fragments_dir: PathBuf,
    pub context: TemplateContext,
}

/// Specs with all template expressions resolved; ready to compile.
///
/// Produced by [`resolve`] from a [`ValidatedSpecs`]. Advance to the compile
/// stage by passing this to [`compile::run`](crate::compile::run).
pub struct ResolvedSpecs {
    specs: Vec<NormalizedSpec>,
}

/// Resolve all template expressions in validated specs.
///
/// This is the single entry point for the templating pipeline. Currently
/// handles fragment includes via `MiniJinja`; designed to accommodate
/// additional templating capabilities in the future.
pub fn resolve(validated: ValidatedSpecs, config: &TemplatingConfig) -> Result<ResolvedSpecs> {
    let fragment_map = load_fragments(&config.fragments_dir)?;
    let env = build_environment(&fragment_map)?;
    let specs = resolve_fragments(validated.into_specs(), &env, &config.context)?;
    Ok(ResolvedSpecs { specs })
}

impl ResolvedSpecs {
    /// Consume self and return the inner specs.
    ///
    /// Used by the compile module to take ownership of the resolved data.
    pub fn into_specs(self) -> Vec<NormalizedSpec> {
        self.specs
    }

    /// Access the resolved specs directly.
    pub fn specs(&self) -> &[NormalizedSpec] {
        &self.specs
    }
}
