use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::adapters::{adapt_claude, adapt_cursor, adapt_opencode};
use crate::presets::ProviderPresetsMap;
use crate::provider::Provider;
use crate::spec::NormalizedSpec;
use crate::specs::ValidatedSpecs;
use crate::templating::{TemplateContext, TemplatingResources, resolve_fragments};

/// Per-provider configuration passed to adapters at compile time.
///
/// Adapters use this to apply prefix/strip transforms to output paths and
/// frontmatter fields. When `None` is passed, adapters produce canonical
/// (unprefixed, unstripped) output.
#[derive(Clone, Debug, Default)]
pub struct AdapterConfig {
    /// Namespace prefix for file paths and frontmatter names.
    pub prefix: Option<String>,
}

impl AdapterConfig {
    /// Returns the file path prefix string (e.g., `"tw-"`), if a prefix is configured.
    /// This is shared across all providers — the file path convention is the same.
    pub fn file_prefix(&self) -> Option<String> {
        self.prefix.as_ref().map(|p| format!("{p}-"))
    }
}

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

/// Compile validated specs into provider-specific generated files.
///
/// Template resolution is performed internally: the `MiniJinja` environment is
/// built from `templating`, the context is constructed from the specs, and each
/// spec body is rendered before being handed to the provider adapters.
pub fn run(
    validated: &ValidatedSpecs,
    templating: &TemplatingResources,
    presets: &ProviderPresetsMap,
    providers: &[Provider],
    adapter_configs: &HashMap<Provider, AdapterConfig>,
) -> Result<CompileResult> {
    compile_specs(
        validated.specs(),
        templating,
        presets,
        providers,
        adapter_configs,
    )
}

/// Takes `&[NormalizedSpec]` (borrowed) even though `resolve_fragments` needs
/// ownership — the slice is cloned once per provider so that each provider gets
/// its own template-resolved copy with the correct prefix-aware names.
pub(crate) fn compile_specs(
    specs: &[NormalizedSpec],
    templating: &TemplatingResources,
    presets: &ProviderPresetsMap,
    providers: &[Provider],
    adapter_configs: &HashMap<Provider, AdapterConfig>,
) -> Result<CompileResult> {
    let env = templating.build_environment()?;
    let mut files: Vec<GeneratedFile> = Vec::new();

    let mut sorted_providers: Vec<Provider> = providers.to_vec();
    sorted_providers.sort_by_key(ToString::to_string);

    for &provider in &sorted_providers {
        let adapter_config = adapter_configs.get(&provider);
        let context = TemplateContext::from_specs_for_provider(specs, provider, adapter_config);
        let resolved = resolve_fragments(specs.to_vec(), &env, &context)?;

        for spec in &resolved {
            let mut adapter_files = match provider {
                Provider::Claude => adapt_claude(spec.clone(), presets, adapter_config)?,
                Provider::Cursor => adapt_cursor(spec.clone(), presets, adapter_config)?,
                Provider::OpenCode => adapt_opencode(spec.clone(), presets, adapter_config)?,
            };
            files.append(&mut adapter_files);
        }
    }

    // Sort output files by path for deterministic ordering
    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(CompileResult { files })
}
