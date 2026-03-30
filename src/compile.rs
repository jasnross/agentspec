use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::adapters::{adapt_claude, adapt_cursor, adapt_opencode};
use crate::config::Provider;
use crate::presets::ProviderPresetsMap;
use crate::spec::NormalizedSpec;

/// A single file produced by a provider adapter.
#[derive(Clone, Debug)]
pub struct GeneratedFile {
    /// Provider that produced this file
    #[allow(dead_code)] // FIXME: consider removing if unused
    pub provider: Provider,
    /// Relative path from project root (e.g., "generated/claude/skills/commit/SKILL.md")
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

/// Compile normalized specs into provider-specific generated files.
pub fn compile_specs(
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
