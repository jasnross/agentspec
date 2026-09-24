mod context;
mod environment;
mod fragments;
mod validation;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
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
    ///
    /// That last check reads `sources_dir`, so it also fails on an entry there
    /// that will not resolve — see [`collect_top_level_dirs`]. It runs only
    /// when `extra_dirs` is non-empty, since with no name to compare against
    /// there is nothing the scan could decide.
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

        // Scanned only when there is a name to compare against, so a spec
        // library with no extra dirs never fails on a `sources_dir` entry it
        // had no reason to read.
        if !extra_dirs.is_empty() {
            let top_level = collect_top_level_dirs(sources_dir)?;
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

/// The names of the directories at the top level of `sources_dir`, which an
/// `extra_include_dirs` name may not collide with.
///
/// Each entry is resolved through any symlink, so a linked directory is in the
/// set — a name matching one would otherwise pass the gate and then resolve
/// includes against the wrong tree. An entry that will not resolve is an error
/// rather than an omission, for the same reason: a set quietly missing a name
/// is a gate that quietly admits it.
///
/// A name that is not valid UTF-8 is the one entry still left out. It cannot
/// mask a collision, because the names it would be compared against come from
/// TOML and are always valid UTF-8.
///
/// `[spec].ignore` does not reach here. A pattern decides membership in the
/// loaded set, and the top level of `sources_dir` is not part of that set —
/// only the four spec roots beneath it are walked, each reached by a join
/// rather than through this scan. So a broken entry here has no pattern that
/// exempts it, and needs repairing or removing.
fn collect_top_level_dirs(sources_dir: &Path) -> Result<HashSet<String>> {
    let entries = std::fs::read_dir(sources_dir).with_context(|| {
        format!(
            "failed to read {} to check it for a name an extra include directory collides with",
            sources_dir.display()
        )
    })?;
    let mut names = HashSet::new();
    for entry in entries {
        let entry = entry
            .with_context(|| format!("failed to read an entry of {}", sources_dir.display()))?;
        let path = entry.path();
        if !crate::symlink::resolve_dir_entry(&path)?.is_dir() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            names.insert(name.to_string());
        }
    }
    Ok(names)
}
