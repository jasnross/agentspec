//! Generic CST file I/O helpers shared across CST-aware merge/tidy paths.
//!
//! Two `pub(crate)` functions:
//!
//! - [`read_or_empty_object`] — read a host config file or treat empty/missing
//!   as `{}` so the parser never sees a zero-byte input.
//! - [`finish`] — atomic write of a parsed CST back to disk, with umask-aware
//!   mode resolution on Unix.
//!
//! Both helpers are deliberately domain-agnostic: they know nothing about
//! hooks, instructions, or any provider-specific shape. The hook merge pipeline
//! (`src/hooks_merge.rs`) and `OpenCode`'s instructions tidy
//! (`src/adapters/opencode.rs`) both call into them, and each caller emits its
//! own dry-run message before invoking [`finish`] — this module never short-
//! circuits on dry-run state.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use jsonc_parser::cst::CstRootNode;

/// Serializes umask reads across concurrent [`finish`] callers.
///
/// `umask(2)` is process-global state and the only way to read it is the
/// set-then-restore dance below — there's a brief window where umask is 0.
/// Production agentspec is single-threaded, so the window is harmless. Under
/// `cargo test`'s default parallel execution, two `finish` calls overlap;
/// without this lock, one test's transient `umask=0` would leak overly
/// permissive modes to another test's concurrent file creates.
#[cfg(unix)]
static UMASK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Read `path` or return `"{}"` for missing/empty files.
///
/// Treat empty or whitespace-only files as `{}`. A zero-byte host config (e.g.,
/// from a partial write or `touch`) shouldn't fail the merge — it's
/// equivalent to "no settings yet."
pub(crate) fn read_or_empty_object(path: &Path) -> Result<String> {
    if !path.is_file() {
        return Ok("{}".to_string());
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    if raw.trim().is_empty() {
        Ok("{}".to_string())
    } else {
        Ok(raw)
    }
}

/// Atomic write: serialize the CST, write to a sibling tempfile, rename into
/// place. A dropped or crashed write leaves the original untouched.
///
/// `tempfile::NamedTempFile::new_in` creates files with mode 0600. We resolve
/// the target mode once (preserving the original mode for an existing file,
/// or honoring the process umask for a fresh file) and apply it to the
/// tempfile *before* `persist`, so the rename delivers the file at the right
/// mode atomically — no observable 0600 window.
///
/// # Multi-thread safety
///
/// The fresh-file branch reads the process umask. Because `umask(2)` mutates
/// process-global state, concurrent `finish` calls are serialized via
/// [`UMASK_LOCK`] — see its docstring for the failure mode this prevents.
///
/// # Dry-run
///
/// Callers own their dry-run handling; this function unconditionally writes.
/// Skip the call upstream when `dry_run` is set so each caller can emit a
/// message tailored to its domain.
pub(crate) fn finish(root: &CstRootNode, path: &Path) -> Result<()> {
    let output = root.to_string();

    let parent = path.parent().with_context(|| {
        format!(
            "destination path {} has no parent directory",
            path.display()
        )
    })?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create dir {}", parent.display()))?;

    // Resolve target mode: existing file → preserve; fresh file → honor umask
    // (the conventional shell behavior, matching how a user-authored
    // `settings.json` would land).
    #[cfg(unix)]
    let target_mode: u32 = {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).ok().map_or_else(
            || {
                // `umask(2)` is the only way to read the current process
                // umask — there is no stdlib accessor. Set-then-restore
                // briefly flips umask to 0; serialize via UMASK_LOCK so
                // overlapping callers don't leak modes to each other.
                // `into_inner` recovers from a poisoned mutex — we don't
                // hold any state inside the lock other than the umask
                // syscall itself, so poison is harmless.
                let _guard = UMASK_LOCK
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let prev = unsafe { libc::umask(0) };
                unsafe { libc::umask(prev) };
                // mode_t is u16 on macOS, u32 on Linux
                #[allow(clippy::cast_lossless)]
                let prev = prev as u32;
                0o666 & !prev
            },
            |m| m.permissions().mode(),
        )
    };

    let tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create tempfile in {}", parent.display()))?;
    fs::write(tmp.path(), output.as_bytes())
        .with_context(|| format!("failed to write tempfile {}", tmp.path().display()))?;

    // Apply target mode to the tempfile before persist so the rename
    // delivers the file at the right mode atomically.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(target_mode))
            .with_context(|| format!("failed to set tempfile mode for {}", path.display()))?;
    }

    tmp.persist(path)
        .with_context(|| format!("failed to atomically rename into {}", path.display()))?;

    Ok(())
}
