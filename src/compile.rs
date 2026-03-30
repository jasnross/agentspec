use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::adapters::{adapt_claude, adapt_cursor, adapt_opencode};
use crate::presets::ProviderPresetsMap;
use crate::provider::Provider;
use crate::spec::NormalizedSpec;
use crate::templating::ResolvedSpecs;

/// A single file produced by a provider adapter.
#[derive(Clone, Debug)]
pub struct GeneratedFile {
    /// Provider that produced this file
    pub provider: Provider,
    /// Relative path from the provider root (e.g., "agents/foo.md", "skills/commit/SKILL.md")
    pub path: PathBuf,
    /// File content
    pub content: Vec<u8>,
    /// Optional file mode (e.g., 0o755 for executable scripts)
    pub mode: Option<u32>,
}

impl GeneratedFile {
    /// Create a text file with no special permissions.
    pub fn text(provider: Provider, path: impl AsRef<Path>, content: String) -> Self {
        Self {
            provider,
            path: path.as_ref().to_path_buf(),
            content: content.into_bytes(),
            mode: None,
        }
    }

    /// Create a binary file, optionally with a mode.
    pub fn binary(
        provider: Provider,
        path: impl AsRef<Path>,
        content: Vec<u8>,
        mode: Option<u32>,
    ) -> Self {
        Self {
            provider,
            path: path.as_ref().to_path_buf(),
            content,
            mode,
        }
    }
}

/// Result of compiling all specs for all target providers.
#[derive(Debug)]
pub struct CompileResult {
    pub files: Vec<GeneratedFile>,
}

impl CompileResult {
    /// Iterate over generated files for a specific provider.
    pub fn files_for(&self, provider: Provider) -> impl Iterator<Item = &GeneratedFile> {
        self.files.iter().filter(move |f| f.provider == provider)
    }
}

/// Compile resolved specs into provider-specific generated files.
///
/// This is the single entry point for the compile stage. Takes ownership of
/// [`ResolvedSpecs`] — the final stage of the pipeline before output is written.
pub fn run(
    resolved: ResolvedSpecs,
    presets: &ProviderPresetsMap,
    providers: &[Provider],
) -> Result<CompileResult> {
    compile_specs(&resolved.into_specs(), presets, providers)
}

pub(crate) fn compile_specs(
    specs: &[NormalizedSpec],
    presets: &ProviderPresetsMap,
    providers: &[Provider],
) -> Result<CompileResult> {
    let mut files: Vec<GeneratedFile> = Vec::new();

    let mut sorted_providers: Vec<Provider> = providers.to_vec();
    sorted_providers.sort_by_key(ToString::to_string);

    for spec in specs {
        for &provider in &sorted_providers {
            let mut adapter_files = match provider {
                Provider::Claude => adapt_claude(spec.clone(), presets)?,
                Provider::Cursor => adapt_cursor(spec.clone(), presets)?,
                Provider::OpenCode => adapt_opencode(spec.clone(), presets)?,
            };

            files.append(&mut adapter_files);
        }
    }

    // Sort output files by path for deterministic ordering
    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(CompileResult { files })
}
