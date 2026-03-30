mod fragments;

use std::path::PathBuf;

use anyhow::Result;

use crate::compile::{CompileResult, compile_specs};
use crate::presets::ProviderPresetsMap;
use crate::provider::Provider;
use crate::spec::NormalizedSpec;
use crate::specs::ValidatedSpecs;

use fragments::{build_environment, load_fragments, resolve_fragments};

/// Configuration for the template resolution pass.
///
/// Kept separate from the binary's `AgentspecConfig` so the library crate
/// has no dependency on binary-specific config types.
pub struct TemplatingConfig {
    pub fragments_dir: PathBuf,
}

/// Specs with all template expressions resolved; ready to compile.
///
/// Produced by [`resolve`] from a [`ValidatedSpecs`]. This is the final
/// stage before compilation — calling [`ResolvedSpecs::compile`] dispatches
/// each spec to a provider adapter.
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
    let specs = resolve_fragments(validated.into_specs(), &env)?;
    Ok(ResolvedSpecs { specs })
}

impl ResolvedSpecs {
    /// Compile resolved specs into provider-specific generated files.
    pub fn compile(
        &self,
        presets: &ProviderPresetsMap,
        providers: &[Provider],
    ) -> Result<CompileResult> {
        compile_specs(&self.specs, presets, providers)
    }

    /// Access the resolved specs directly.
    pub fn specs(&self) -> &[NormalizedSpec] {
        &self.specs
    }
}
