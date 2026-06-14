mod context;
mod environment;
mod fragments;
mod validation;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
pub use context::TemplateContext;
use environment::build_environment;
pub use fragments::resolve_fragments;
use minijinja::Environment;
use serde::Deserialize;

use crate::provider::Provider;
use crate::spec::Spec;

/// Named external include directory: `{ name = "shared", path = "path/to/dir" }`.
///
/// The `name` becomes the path prefix in includes — a file `foo.md` in the
/// directory is included via `{% include "shared/foo.md" %}`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtraIncludeDir {
    pub name: String,
    pub path: PathBuf,
}

/// Reusable templating infrastructure: owns the source directory path and
/// extra include dirs, building `MiniJinja` environments on demand with a
/// lazy loader.
#[derive(Debug)]
pub struct Templating {
    sources_dir: PathBuf,
    extra_dirs: Vec<ExtraIncludeDir>,
}

impl Templating {
    /// Validate configuration and construct a [`Templating`] instance.
    ///
    /// Checks that extra dir paths exist and are directories, that extra dir
    /// names are unique, and that no extra dir name collides with a top-level
    /// directory under `sources_dir`.
    pub fn new(sources_dir: &Path, extra_dirs: &[ExtraIncludeDir]) -> Result<Self> {
        for extra in extra_dirs {
            if extra.name.trim().is_empty() {
                bail!("extra include directory name must not be empty or whitespace-only");
            }
            if extra.name.contains('/') || extra.name.contains('\\') {
                bail!(
                    "extra include directory name must not contain path separators: \"{}\"",
                    extra.name,
                );
            }
            if extra.name == ".." || extra.name == "." {
                bail!(
                    "extra include directory name must not be \".\" or \"..\": \"{}\"",
                    extra.name,
                );
            }
        }

        for extra in extra_dirs {
            if !extra.path.is_dir() {
                bail!(
                    "extra include directory does not exist: {} (name: \"{}\")",
                    extra.path.display(),
                    extra.name,
                );
            }
        }

        let mut seen_names = HashSet::new();
        for extra in extra_dirs {
            if !seen_names.insert(&extra.name) {
                bail!("duplicate extra include directory name: \"{}\"", extra.name);
            }
        }

        let top_level = collect_top_level_dirs(sources_dir);
        for extra in extra_dirs {
            if top_level.contains(&extra.name) {
                bail!(
                    "extra include directory name \"{}\" collides with top-level \
                     directory under {}",
                    extra.name,
                    sources_dir.display(),
                );
            }
        }

        Ok(Self {
            sources_dir: sources_dir.to_path_buf(),
            extra_dirs: extra_dirs.to_vec(),
        })
    }

    /// Build a `MiniJinja` environment for `spec` with all includes resolved
    /// lazily via the loader. See [`environment::build_environment`] for the
    /// full contract, including `script()` gating.
    pub fn build_environment(&self, provider: Option<Provider>, spec: &Spec) -> Environment<'_> {
        build_environment(&self.sources_dir, &self.extra_dirs, provider, spec)
    }

    pub fn sources_dir(&self) -> &Path {
        &self.sources_dir
    }

    pub fn extra_dirs(&self) -> &[ExtraIncludeDir] {
        &self.extra_dirs
    }

    #[cfg(test)]
    pub(crate) fn from_sources(sources_dir: PathBuf, extra_dirs: Vec<ExtraIncludeDir>) -> Self {
        Self {
            sources_dir,
            extra_dirs,
        }
    }
}

fn collect_top_level_dirs(sources_dir: &Path) -> HashSet<String> {
    let Ok(entries) = std::fs::read_dir(sources_dir) else {
        return HashSet::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|ft| ft.is_dir()))
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .collect()
}
