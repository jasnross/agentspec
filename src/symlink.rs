//! Symlink resolution, and the diagnostics that name a link which does not
//! resolve rather than whatever the caller was reaching for through it.
//!
//! A message lives here so that every layer following a link reports a broken
//! one in the same words.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow};

/// The diagnostic message for a symlink whose target could not be stat'd, named
/// on the link rather than on whatever the caller was reaching for through it.
pub(crate) fn unresolvable_symlink(path: &Path, e: &std::io::Error) -> String {
    let source = path.display();
    if e.kind() == std::io::ErrorKind::NotFound {
        return dangling_symlink(path);
    }
    // A cycle lands here as the platform's `ELOOP`, whose `ErrorKind` is still
    // unstable, as do `EACCES` and `ENOTDIR`. Let the OS supply the reason.
    format!("{source}: symlink target could not be resolved: {e}")
}

/// The diagnostic message for a symlink with nothing at its target.
pub(crate) fn dangling_symlink(path: &Path) -> String {
    format!("{}: symlink target does not exist", path.display())
}

/// Resolves the `read_dir` entry at `path` through any symlink, returning the
/// file type of the target.
///
/// [`fs::DirEntry::file_type`] describes the link itself, so a symlinked skill
/// directory or spec file reports neither `is_dir` nor `is_file` and would drop
/// out of a filter written against it. Resolving through the link also means an
/// unresolvable target surfaces as an error here rather than as a silent skip,
/// matching the contract `walk_spec_tree` in `crate::specs` holds for the
/// `WalkDir` passes. Both paths share [`unresolvable_symlink`], so a cycle reads
/// the same either way — except where `walkdir` catches one through its own
/// ancestor tracking, which can also name the directory the link cycles back to.
pub(crate) fn resolve_dir_entry(path: &Path) -> Result<fs::FileType> {
    let e = match path.metadata() {
        Ok(metadata) => return Ok(metadata.file_type()),
        Err(e) => e,
    };
    if is_symlink(path) {
        return Err(anyhow!(unresolvable_symlink(path, &e)));
    }
    Err(e).with_context(|| format!("failed to read {}", path.display()))
}

pub(crate) fn is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
}
