//! Generic CST-aware merge shell for adapter-supplied hook patches.
//!
//! The Bundled (plugin-scope) emission path writes complete `hooks.json` files.
//! The Merged (Project/User-scope) path can't replace the host config — it
//! carries permissions, env config, allowlists, and user-authored hooks that
//! must survive the sync. This module is the generic plumbing for that path:
//! file I/O, CST parse, top-level open, atomic write, and the delete-on-empty
//! tail. All provider-specific JSON-shape decisions (top-level extras,
//! per-event nesting depth, owned-entry pruning, the delete predicate, host
//! filename) live behind the [`HookAdapter::merge_into`] /
//! [`HookAdapter::tidy_after_remove`] / [`HookAdapter::host_filename`] trait
//! methods in `src/adapters/*.rs`.
//!
//! Ownership is identified by the `_agentspec_id` sentinel field on each
//! entry: agentspec-owned entries are replaced wholesale on each sync; entries
//! lacking the sentinel are user-authored and stay byte-identical. Comments,
//! trailing commas, key ordering, and untouched whitespace round-trip
//! byte-identical via `jsonc-parser`'s CST API.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use jsonc_parser::ParseOptions;
use jsonc_parser::cst::CstRootNode;

use crate::adapters::HookAdapter;
use crate::compile::EmittedHookEntry;
use crate::plan::{PostWriteHook, RemovePatchReport, delete_host_file_and_rmdir_parent};

/// Merge agentspec-owned hook entries into the host config file at
/// `host_path`. The provider-specific shape decisions are delegated to
/// `adapter.merge_into`; this function only handles file I/O, CST parse, the
/// no-op-skip guard, and the atomic write.
pub fn merge_owned(
    adapter: &dyn HookAdapter,
    host_path: &Path,
    owned_entries: &[EmittedHookEntry],
    force: bool,
    dry_run: bool,
) -> Result<()> {
    // Skip when the file is absent and we have no entries to add — avoids
    // creating a spurious config file on a fresh project.
    if !host_path.is_file() && owned_entries.is_empty() {
        return Ok(());
    }
    let content = read_or_empty_object(host_path)?;
    let root = CstRootNode::parse(&content, &ParseOptions::default())
        .with_context(|| format!("failed to parse {}", host_path.display()))?;

    let top = root.object_value_or_create().with_context(|| {
        format!(
            "{} has a non-object root; agentspec hooks merge requires a JSON object",
            host_path.display()
        )
    })?;

    // No-op skip: when there are no entries to add and no existing `hooks`
    // key to clean orphans from, don't touch the file. Adapters that inject
    // top-level extras (e.g. Cursor's `version: 1`) intentionally don't run
    // when this guard fires — the file would otherwise be modified for no
    // observable benefit.
    if owned_entries.is_empty() && top.get("hooks").is_none() {
        return Ok(());
    }

    adapter
        .merge_into(&top, owned_entries, force)
        .with_context(|| format!("merge into {} failed", host_path.display()))?;

    finish(&root, host_path, dry_run)
}

/// Strip agentspec-owned hook entries from the host config file at
/// `host_path` and either delete the file (when the adapter's predicate says
/// it's effectively empty) or write the tidied CST back.
pub fn remove_owned(
    adapter: &dyn HookAdapter,
    host_path: &Path,
    dry_run: bool,
) -> Result<RemovePatchReport> {
    if !host_path.is_file() {
        return Ok(RemovePatchReport::default());
    }
    let content = read_or_empty_object(host_path)?;
    let root = CstRootNode::parse(&content, &ParseOptions::default())
        .with_context(|| format!("failed to parse {}", host_path.display()))?;

    let Some(top) = root.object_value_or_create() else {
        let prefix = if dry_run { "[dry-run] " } else { "" };
        eprintln!(
            "{prefix}warning: {} has a non-object root; skipping tidy",
            host_path.display()
        );
        return Ok(RemovePatchReport {
            host_path: host_path.to_path_buf(),
            user_entries_remaining: 0,
            host_file_deleted: false,
            parent_rmdir: false,
        });
    };

    let outcome = adapter.tidy_after_remove(&top);

    if outcome.file_should_be_deleted {
        let parent_rmdir = delete_host_file_and_rmdir_parent(host_path, dry_run)?;
        return Ok(RemovePatchReport {
            host_path: host_path.to_path_buf(),
            user_entries_remaining: 0,
            host_file_deleted: true,
            parent_rmdir,
        });
    }

    finish(&root, host_path, dry_run)?;

    Ok(RemovePatchReport {
        host_path: host_path.to_path_buf(),
        user_entries_remaining: outcome.user_entries_remaining,
        host_file_deleted: false,
        parent_rmdir: false,
    })
}

/// Post-write hook that merges agentspec-owned hook entries into a provider's
/// hand-edited host config (Claude's `settings.json`, Cursor's `hooks.json`)
/// via the CST patcher. Constructed once per `(provider, FileKind::Hooks)`
/// sync call when the emit mode is `MergedUser` or `MergedProject`. Path mode
/// owns the entire `hooks/hooks.json` file directly so no patcher is created.
#[derive(Debug)]
pub struct HooksPatch {
    pub adapter: &'static dyn HookAdapter,
    pub host_path: PathBuf,
    pub owned_entries: Vec<EmittedHookEntry>,
    /// `--force`/`overwrite=true`: replace a non-object `hooks` (or non-array
    /// per-event) value with `{}`/`[]` before merging, instead of erroring.
    pub force: bool,
}

impl PostWriteHook for HooksPatch {
    fn run(&self, dry_run: bool) -> Result<()> {
        merge_owned(
            self.adapter,
            &self.host_path,
            &self.owned_entries,
            self.force,
            dry_run,
        )
    }
}

/// Post-write hook that strips agentspec-owned hook entries from a provider's
/// host config and tidies emptied containers, paralleling [`HooksPatch`] but
/// in reverse. Ownership is identified by the on-disk `_agentspec_id`
/// sentinel — no in-memory owned-entries list is needed.
#[derive(Debug)]
pub struct RemoveHooksPatch {
    pub adapter: &'static dyn HookAdapter,
    pub host_path: PathBuf,
}

impl PostWriteHook for RemoveHooksPatch {
    fn run(&self, dry_run: bool) -> Result<()> {
        let report = remove_owned(self.adapter, &self.host_path, dry_run)?;
        report.print_summary(dry_run);
        Ok(())
    }
}

fn read_or_empty_object(path: &Path) -> Result<String> {
    if !path.is_file() {
        return Ok("{}".to_string());
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    // Treat empty or whitespace-only files as `{}`. A zero-byte settings.json
    // (e.g., from a partial write or `touch`) shouldn't fail the merge — it's
    // equivalent to "no settings yet."
    if raw.trim().is_empty() {
        Ok("{}".to_string())
    } else {
        Ok(raw)
    }
}

/// Serializes umask reads across concurrent `finish` callers.
///
/// `umask(2)` is process-global state and the only way to read it is the
/// set-then-restore dance below — there's a brief window where umask is 0.
/// Production agentspec is single-threaded, so the window is harmless. Under
/// `cargo test`'s default parallel execution, two `finish` calls overlap;
/// without this lock, one test's transient `umask=0` would leak overly
/// permissive modes to another test's concurrent file creates.
#[cfg(unix)]
static UMASK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
fn finish(root: &CstRootNode, path: &Path, dry_run: bool) -> Result<()> {
    let output = root.to_string();

    if dry_run {
        eprintln!(
            "[dry-run] would merge {} bytes into {}",
            output.len(),
            path.display()
        );
        return Ok(());
    }

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
                0o666 & !(u32::from(prev))
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

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::adapters::{ClaudeAdapter, CursorAdapter};
    use crate::spec::HookEvent;

    fn entry(id: &str, event: HookEvent, command: &str) -> EmittedHookEntry {
        EmittedHookEntry {
            event,
            matcher: None,
            command: command.to_string(),
            timeout: None,
            agentspec_id: id.to_string(),
        }
    }

    fn entry_with_matcher(
        id: &str,
        event: HookEvent,
        matcher: &str,
        command: &str,
    ) -> EmittedHookEntry {
        EmittedHookEntry {
            event,
            matcher: Some(matcher.to_string()),
            command: command.to_string(),
            timeout: None,
            agentspec_id: id.to_string(),
        }
    }

    #[test]
    fn test_merge_claude_creates_settings_when_absent() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("settings.json");
        let entries = vec![entry(
            "init",
            HookEvent::SessionStart,
            "$HOME/.claude/hooks/scripts/init.sh",
        )];

        merge_owned(&ClaudeAdapter, &path, &entries, false, false).expect("merge");

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).expect("read");
        assert!(content.contains("\"hooks\""));
        assert!(content.contains("\"SessionStart\""));
        assert!(content.contains("\"_agentspec_id\""));
    }

    #[test]
    fn test_merge_claude_preserves_user_top_level_keys_and_comments() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("settings.json");
        // JSONC with a comment and a non-hooks top-level key. After merging,
        // comments and the `permissions` value must round-trip unchanged.
        let initial = r#"{
  // user comment
  "permissions": {
    "allow": ["Read", "Bash"]
  }
}
"#;
        std::fs::write(&path, initial).expect("write initial");

        let entries = vec![entry(
            "init",
            HookEvent::SessionStart,
            "$HOME/.claude/hooks/scripts/init.sh",
        )];
        merge_owned(&ClaudeAdapter, &path, &entries, false, false).expect("merge");

        let after = std::fs::read_to_string(&path).expect("read");
        assert!(
            after.contains("// user comment"),
            "comment should be preserved, got:\n{after}"
        );
        assert!(
            after.contains("\"allow\""),
            "permissions key should be preserved, got:\n{after}"
        );
        assert!(after.contains("\"_agentspec_id\""), "got:\n{after}");
    }

    #[test]
    fn test_merge_claude_replaces_owned_entry_leaves_user_entry() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("settings.json");
        let initial = r#"{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          { "type": "command", "command": "/usr/local/bin/user-script.sh" },
          { "type": "command", "command": "OLD", "_agentspec_id": "init" }
        ]
      }
    ]
  }
}
"#;
        std::fs::write(&path, initial).expect("write initial");

        let entries = vec![entry("init", HookEvent::SessionStart, "NEW")];
        merge_owned(&ClaudeAdapter, &path, &entries, false, false).expect("merge");

        let after = std::fs::read_to_string(&path).expect("read");
        assert!(
            after.contains("/usr/local/bin/user-script.sh"),
            "user-authored entry must survive, got:\n{after}"
        );
        assert!(
            after.contains("\"NEW\""),
            "new owned entry must be present, got:\n{after}"
        );
        assert!(
            !after.contains("\"OLD\""),
            "stale owned entry must be removed, got:\n{after}"
        );
    }

    #[test]
    fn test_merge_claude_leaves_empty_event_array_after_removing_all_owned_entries() {
        // Claude's merge_into deliberately does NOT prune empty event arrays —
        // the user might still have entries to add later, and touching the
        // event key shape is more invasive than necessary.
        // Lock that contract: if all `_agentspec_id` entries under an event
        // are removed and no replacements arrive, the event key remains as
        // an empty array rather than getting deleted.
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("settings.json");

        // Seed with one owned entry under PreToolUse, then re-sync with no
        // PreToolUse entries (only a SessionStart entry under a different event).
        let entries_v1 = vec![entry_with_matcher(
            "audit",
            HookEvent::PreToolUse,
            "Bash",
            "AUDIT",
        )];
        merge_owned(&ClaudeAdapter, &path, &entries_v1, false, false).expect("merge v1");
        let entries_v2 = vec![entry("init", HookEvent::SessionStart, "INIT")];
        merge_owned(&ClaudeAdapter, &path, &entries_v2, false, false).expect("merge v2");

        let after = std::fs::read_to_string(&path).expect("read");
        assert!(
            !after.contains("\"AUDIT\""),
            "owned entry must be removed, got:\n{after}"
        );
        // Empty event array stays — contract documented in source comment.
        assert!(
            after.contains("\"PreToolUse\""),
            "PreToolUse event key should remain (intentionally not pruned), got:\n{after}"
        );
    }

    #[test]
    fn test_merge_claude_orphan_removed_when_owned_list_shrinks() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("settings.json");

        // First sync: two owned entries.
        let entries_v1 = vec![
            entry("init", HookEvent::SessionStart, "INIT"),
            entry("audit", HookEvent::SessionStart, "AUDIT"),
        ];
        merge_owned(&ClaudeAdapter, &path, &entries_v1, false, false).expect("merge v1");
        let after_v1 = std::fs::read_to_string(&path).expect("read v1");
        assert!(after_v1.contains("\"INIT\"") && after_v1.contains("\"AUDIT\""));

        // Re-sync with `audit` removed — it must disappear.
        let entries_v2 = vec![entry("init", HookEvent::SessionStart, "INIT")];
        merge_owned(&ClaudeAdapter, &path, &entries_v2, false, false).expect("merge v2");
        let after_v2 = std::fs::read_to_string(&path).expect("read v2");
        assert!(after_v2.contains("\"INIT\""));
        assert!(
            !after_v2.contains("\"AUDIT\""),
            "stale audit entry must be removed, got:\n{after_v2}"
        );
    }

    #[test]
    fn test_merge_cursor_sets_version_when_absent() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("hooks.json");
        let entries = vec![entry(
            "init",
            HookEvent::SessionStart,
            "$HOME/.cursor/hooks/scripts/init.sh",
        )];
        merge_owned(&CursorAdapter, &path, &entries, false, false).expect("merge");

        let content = std::fs::read_to_string(&path).expect("read");
        assert!(
            content.contains("\"version\": 1") || content.contains("\"version\":1"),
            "version: 1 should be set, got:\n{content}"
        );
    }

    #[test]
    fn test_merge_cursor_places_matcher_per_entry() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("hooks.json");
        let entries = vec![entry_with_matcher(
            "audit",
            HookEvent::PreToolUse,
            "Bash",
            "$HOME/.cursor/hooks/scripts/audit.sh",
        )];
        merge_owned(&CursorAdapter, &path, &entries, false, false).expect("merge");

        let content = std::fs::read_to_string(&path).expect("read");
        // Cursor's shape: `{"type": "command", "matcher": "...", "command": "..."}`.
        // Look for matcher field directly inside the entry — not as a wrapper.
        assert!(
            content.contains("\"matcher\""),
            "matcher should appear per-entry, got:\n{content}"
        );
        assert!(
            content.contains("\"Bash\""),
            "matcher value preserved, got:\n{content}"
        );
    }

    #[test]
    fn test_merge_handles_empty_file() {
        // A zero-byte settings.json (e.g., from a partial write or `touch`) must
        // be treated as `{}`, not passed straight to `CstRootNode::parse("")`
        // which has undocumented behavior.
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "").expect("touch empty file");

        let entries = vec![entry(
            "init",
            HookEvent::SessionStart,
            "$HOME/.claude/hooks/scripts/init.sh",
        )];
        merge_owned(&ClaudeAdapter, &path, &entries, false, false)
            .expect("merge should succeed on empty file");

        let content = std::fs::read_to_string(&path).expect("read");
        assert!(content.contains("\"_agentspec_id\""));
    }

    #[test]
    fn test_merge_skips_when_no_file_and_no_entries() {
        // Fresh project, no hooks configured: the merge should not create a
        // spurious `{"hooks": {}}` settings.json.
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("settings.json");
        merge_owned(&ClaudeAdapter, &path, &[], false, false).expect("merge");
        assert!(
            !path.exists(),
            "no file + no entries must not create settings.json"
        );
    }

    #[test]
    fn test_merge_cursor_skips_when_file_exists_no_hooks_key_and_no_entries() {
        // Symmetric to the Claude case: a `.cursor/hooks.json` that exists
        // but lacks a `hooks` key (e.g., user has only set `version`) must
        // not be re-written when there are no agentspec hook specs.
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("hooks.json");
        let initial = "{\n  \"version\": 1\n}\n";
        std::fs::write(&path, initial).expect("write initial");

        merge_owned(&CursorAdapter, &path, &[], false, false).expect("merge");

        let after = std::fs::read_to_string(&path).expect("read");
        assert_eq!(after, initial, "file must round-trip byte-identical");
    }

    #[test]
    fn test_merge_skips_when_file_exists_no_hooks_key_and_no_entries() {
        // A user who has a `settings.json` with `permissions` etc. but no
        // hooks block — and a project with no hook specs — must not have
        // `"hooks": {}` injected on every sync. The file should round-trip
        // byte-identical.
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("settings.json");
        let initial = "{\n  \"permissions\": { \"allow\": [\"Read\"] }\n}\n";
        std::fs::write(&path, initial).expect("write initial");

        merge_owned(&ClaudeAdapter, &path, &[], false, false).expect("merge");

        let after = std::fs::read_to_string(&path).expect("read");
        assert_eq!(after, initial, "file must round-trip byte-identical");
    }

    #[test]
    fn test_merge_refuses_to_overwrite_non_object_hooks_value() {
        // Sentinel-based ownership relies on `hooks` being an object. A user
        // who hand-wrote `"hooks": null` should get a clear error pointing at
        // `--force` as the recovery, not a silent overwrite.
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "{\"hooks\": null}").expect("write malformed");

        let entries = vec![entry(
            "init",
            HookEvent::SessionStart,
            "$HOME/.claude/hooks/scripts/init.sh",
        )];
        let err = merge_owned(&ClaudeAdapter, &path, &entries, false, false)
            .expect_err("expected refusal-to-overwrite error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("not an object"),
            "expected non-object refusal, got: {msg}"
        );
        assert!(
            msg.contains("--force"),
            "error must point at --force as the recovery, got: {msg}"
        );
        // File must be untouched after refusal.
        let after = std::fs::read_to_string(&path).expect("read");
        assert_eq!(after, "{\"hooks\": null}");
    }

    #[test]
    fn test_merge_force_replaces_non_object_hooks_value() {
        // `--force` must let agentspec replace a non-object `hooks` value
        // (e.g., the user's `null`) with `{}` and proceed with the merge.
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "{\"hooks\": null}").expect("write malformed");

        let entries = vec![entry(
            "init",
            HookEvent::SessionStart,
            "$HOME/.claude/hooks/scripts/init.sh",
        )];
        merge_owned(&ClaudeAdapter, &path, &entries, true, false).expect("force merge");

        let after = std::fs::read_to_string(&path).expect("read");
        assert!(
            after.contains("\"_agentspec_id\""),
            "force merge must inject the entry, got: {after}"
        );
        assert!(
            !after.contains("\"hooks\": null"),
            "non-object hooks value must be gone, got: {after}"
        );
    }

    #[test]
    fn test_merge_force_replaces_non_array_per_event_value() {
        // Inner-array symmetry with the outer hooks guard: --force must also
        // replace a non-array per-event value (e.g., user wrote
        // `"PreToolUse": "weird"`) with `[]` before merging entries into it.
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("settings.json");
        std::fs::write(
            &path,
            "{\n  \"hooks\": {\n    \"PreToolUse\": \"weird\"\n  }\n}\n",
        )
        .expect("write malformed");

        let entries = vec![entry_with_matcher(
            "audit",
            HookEvent::PreToolUse,
            "Bash",
            "$HOME/.claude/hooks/scripts/audit.sh",
        )];
        merge_owned(&ClaudeAdapter, &path, &entries, true, false).expect("force merge");

        let after = std::fs::read_to_string(&path).expect("read");
        assert!(
            after.contains("\"_agentspec_id\""),
            "force merge must inject the entry, got: {after}"
        );
        assert!(
            !after.contains("\"PreToolUse\": \"weird\""),
            "non-array per-event value must be gone, got: {after}"
        );
    }

    #[test]
    fn test_merge_inner_non_array_errors_without_force() {
        // Without --force, a non-array per-event value must error and the
        // message must point at --force as the recovery.
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("settings.json");
        std::fs::write(
            &path,
            "{\n  \"hooks\": {\n    \"PreToolUse\": \"weird\"\n  }\n}\n",
        )
        .expect("write malformed");

        let entries = vec![entry_with_matcher(
            "audit",
            HookEvent::PreToolUse,
            "Bash",
            "$HOME/.claude/hooks/scripts/audit.sh",
        )];
        let err = merge_owned(&ClaudeAdapter, &path, &entries, false, false)
            .expect_err("expected refusal-to-overwrite error at inner level");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("not an array"),
            "expected non-array refusal, got: {msg}"
        );
        assert!(
            msg.contains("--force"),
            "inner error must point at --force as recovery, got: {msg}"
        );
    }

    #[test]
    fn test_merge_cursor_force_replaces_non_object_hooks_value() {
        // Cursor mirror of the Claude force test.
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("hooks.json");
        std::fs::write(&path, "{\"version\": 1, \"hooks\": \"oops\"}").expect("write malformed");

        let entries = vec![entry(
            "audit",
            HookEvent::PreToolUse,
            "$HOME/.cursor/hooks/scripts/audit.sh",
        )];
        merge_owned(&CursorAdapter, &path, &entries, true, false).expect("force merge");

        let after = std::fs::read_to_string(&path).expect("read");
        assert!(
            after.contains("\"_agentspec_id\""),
            "force merge must inject the entry, got: {after}"
        );
        assert!(
            !after.contains("\"hooks\": \"oops\""),
            "non-object hooks value must be gone, got: {after}"
        );
    }

    #[test]
    fn test_merge_dry_run_does_not_write() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("settings.json");
        let entries = vec![entry(
            "init",
            HookEvent::SessionStart,
            "$HOME/.claude/hooks/scripts/init.sh",
        )];
        merge_owned(&ClaudeAdapter, &path, &entries, false, true).expect("dry-run merge");
        assert!(!path.exists(), "dry-run must not create the file");
    }

    #[test]
    fn test_merge_idempotent_round_trip() {
        // Running the merge twice with the same entries produces the same output.
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("settings.json");
        let entries = vec![entry(
            "init",
            HookEvent::SessionStart,
            "$HOME/.claude/hooks/scripts/init.sh",
        )];
        merge_owned(&ClaudeAdapter, &path, &entries, false, false).expect("merge 1");
        let after_1 = std::fs::read_to_string(&path).expect("read 1");

        merge_owned(&ClaudeAdapter, &path, &entries, false, false).expect("merge 2");
        let after_2 = std::fs::read_to_string(&path).expect("read 2");

        assert_eq!(after_1, after_2, "merge must be idempotent");
    }

    #[cfg(unix)]
    #[test]
    fn test_merge_fresh_file_honors_umask() {
        // Fresh-file creation must honor the process umask rather than
        // leaving tempfile's restrictive 0600 default — otherwise a fresh
        // ~/.claude/settings.json would land at 0600, more restrictive than
        // every other dotfile under $HOME.
        //
        // Note: `umask(2)` is per-process; this test serially sets and restores
        // it, so running with `--test-threads=1` is safest if this becomes
        // flaky alongside other Unix-mode-asserting tests (none exist today).
        use std::os::unix::fs::PermissionsExt;
        let original = unsafe { libc::umask(0o022) };
        let restore = scopeguard_restore_umask(original);

        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("settings.json");
        let entries = vec![entry(
            "init",
            HookEvent::SessionStart,
            "$HOME/.claude/hooks/scripts/init.sh",
        )];
        merge_owned(&ClaudeAdapter, &path, &entries, false, false).expect("merge");

        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o644,
            "fresh-file mode should equal 0o666 & !0o022 = 0o644 (got {mode:o})"
        );
        drop(restore);
    }

    #[cfg(unix)]
    fn scopeguard_restore_umask(original: libc::mode_t) -> impl Drop {
        struct Restore(libc::mode_t);
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe { libc::umask(self.0) };
            }
        }
        Restore(original)
    }

    #[test]
    fn test_entry_to_claude_json_includes_sentinel() {
        let e = entry("init", HookEvent::SessionStart, "/path/to/script.sh");
        let v = ClaudeAdapter.entry_to_json(&e);
        assert_eq!(v["type"], "command");
        assert_eq!(v["command"], "/path/to/script.sh");
        assert_eq!(v["_agentspec_id"], "init");
        assert!(
            v.get("matcher").is_none(),
            "claude entry: matcher is on the wrapper, not the entry"
        );
    }

    #[test]
    fn test_entry_to_cursor_json_places_matcher_on_entry() {
        let e = entry_with_matcher("audit", HookEvent::PreToolUse, "Bash", "/path/to/script.sh");
        let v = CursorAdapter.entry_to_json(&e);
        assert_eq!(v["matcher"], "Bash");
        assert_eq!(v["_agentspec_id"], "audit");
    }

    // ── delete-on-empty tidy tests ─────────────────────────────────────────

    #[test]
    fn test_tidy_claude_deletes_settings_when_only_agentspec_content_was_present() {
        // Sync seeds one owned entry; remove now finds the file effectively
        // empty after tidy and deletes it. The parent directory is rmdir'd
        // when it becomes empty.
        let tmp = TempDir::new().expect("tmp");
        let parent = tmp.path().join("claude");
        std::fs::create_dir_all(&parent).expect("mkdir");
        let path = parent.join("settings.json");
        let entries = vec![entry("init", HookEvent::SessionStart, "INIT")];
        merge_owned(&ClaudeAdapter, &path, &entries, false, false).expect("seed");

        let report = remove_owned(&ClaudeAdapter, &path, false).expect("tidy");

        assert!(
            !path.exists(),
            "host file should be deleted when only agentspec content was present"
        );
        assert!(!parent.exists(), "parent should be rmdir'd when empty");
        assert!(report.host_file_deleted);
        assert!(report.parent_rmdir);
        assert_eq!(report.user_entries_remaining, 0);
    }

    #[test]
    fn test_tidy_claude_keeps_settings_when_user_content_remains() {
        // User has a `permissions` key alongside agentspec hooks. After tidy,
        // permissions survives and the file stays.
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("settings.json");
        let initial = r#"{
  "permissions": { "allow": ["Read"] },
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          { "type": "command", "command": "OWNED", "_agentspec_id": "init" }
        ]
      }
    ]
  }
}
"#;
        std::fs::write(&path, initial).expect("write");

        let report = remove_owned(&ClaudeAdapter, &path, false).expect("tidy");

        assert!(
            path.exists(),
            "host file must survive when user-authored top-level keys remain"
        );
        assert!(!report.host_file_deleted);
        assert!(!report.parent_rmdir);
        let after = std::fs::read_to_string(&path).expect("read");
        assert!(
            after.contains("\"permissions\""),
            "permissions must round-trip, got:\n{after}"
        );
        assert!(
            !after.contains("\"_agentspec_id\""),
            "owned entry must be stripped, got:\n{after}"
        );
    }

    #[test]
    fn test_tidy_claude_keeps_settings_when_no_owned_content_was_removed() {
        // No agentspec entries to remove → file unchanged byte-for-byte.
        // Pins the `removed_owned > 0` guard for Claude.
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("settings.json");
        let initial = "{\n  \"permissions\": { \"allow\": [\"Read\"] }\n}\n";
        std::fs::write(&path, initial).expect("write");

        let report = remove_owned(&ClaudeAdapter, &path, false).expect("tidy");

        assert!(
            path.exists(),
            "host file must not be deleted when no owned entries were removed"
        );
        assert!(!report.host_file_deleted);
        let after = std::fs::read_to_string(&path).expect("read");
        assert_eq!(after, initial, "no-op tidy must round-trip byte-identical");
    }

    #[test]
    fn test_tidy_cursor_deletes_hooks_when_only_agentspec_content_was_present_with_default_version()
    {
        // Standard fresh-Cursor case: sync injected version: 1 + a hook
        // entry. Remove tidy deletes the file.
        let tmp = TempDir::new().expect("tmp");
        let parent = tmp.path().join("cursor");
        std::fs::create_dir_all(&parent).expect("mkdir");
        let path = parent.join("hooks.json");
        let entries = vec![entry("init", HookEvent::SessionStart, "INIT")];
        merge_owned(&CursorAdapter, &path, &entries, false, false).expect("seed");

        let report = remove_owned(&CursorAdapter, &path, false).expect("tidy");

        assert!(
            !path.exists(),
            "host file should be deleted with default version: 1"
        );
        assert!(!parent.exists(), "parent should be rmdir'd");
        assert!(report.host_file_deleted);
        assert!(report.parent_rmdir);
    }

    #[test]
    fn test_tidy_cursor_deletes_hooks_when_only_agentspec_content_was_present_with_custom_version()
    {
        // User hand-set version: 2 before sync. After remove, residual is
        // {version: 2} — the `removed_owned > 0` guard makes deletion safe
        // even with a non-default version.
        let tmp = TempDir::new().expect("tmp");
        let parent = tmp.path().join("cursor");
        std::fs::create_dir_all(&parent).expect("mkdir");
        let path = parent.join("hooks.json");
        std::fs::write(
            &path,
            r#"{
  "version": 2,
  "hooks": {
    "SessionStart": [
      { "type": "command", "command": "OWNED", "_agentspec_id": "init" }
    ]
  }
}
"#,
        )
        .expect("write");

        let report = remove_owned(&CursorAdapter, &path, false).expect("tidy");

        assert!(
            !path.exists(),
            "host file should be deleted even with version: 2"
        );
        assert!(!parent.exists(), "parent should be rmdir'd");
        assert!(report.host_file_deleted);
    }

    #[test]
    fn test_tidy_cursor_keeps_hooks_when_no_owned_content_was_removed() {
        // No agentspec entries at all → file unchanged byte-for-byte.
        // Pins the `removed_owned > 0` guard for Cursor — without it, the
        // version-only carve-out would delete a file the user authored.
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("hooks.json");
        let initial = "{\n  \"version\": 2\n}\n";
        std::fs::write(&path, initial).expect("write");

        let report = remove_owned(&CursorAdapter, &path, false).expect("tidy");

        assert!(
            path.exists(),
            "host file must not be deleted when no owned entries were removed"
        );
        assert!(!report.host_file_deleted);
        let after = std::fs::read_to_string(&path).expect("read");
        assert_eq!(after, initial, "no-op tidy must round-trip byte-identical");
    }

    #[test]
    fn test_tidy_cursor_keeps_hooks_when_user_keys_remain() {
        // Cursor file has version + a custom user key + agentspec hooks.
        // After tidy, version + user key remain → predicate fails (more
        // than one surviving key) → file stays.
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("hooks.json");
        std::fs::write(
            &path,
            r#"{
  "version": 1,
  "customKey": "value",
  "hooks": {
    "SessionStart": [
      { "type": "command", "command": "OWNED", "_agentspec_id": "init" }
    ]
  }
}
"#,
        )
        .expect("write");

        let report = remove_owned(&CursorAdapter, &path, false).expect("tidy");

        assert!(
            path.exists(),
            "host file must survive when user keys remain"
        );
        assert!(!report.host_file_deleted);
        let after = std::fs::read_to_string(&path).expect("read");
        assert!(
            after.contains("\"customKey\""),
            "custom user key must round-trip, got:\n{after}"
        );
        assert!(
            after.contains("\"version\""),
            "version must round-trip, got:\n{after}"
        );
        assert!(
            !after.contains("\"_agentspec_id\""),
            "owned entry must be stripped, got:\n{after}"
        );
    }

    #[test]
    fn test_tidy_dry_run_does_not_delete_or_rmdir() {
        // Dry-run must not touch the filesystem, but the returned report
        // should still carry `host_file_deleted: true` so the caller can
        // see what the live run would do.
        let tmp = TempDir::new().expect("tmp");
        let parent = tmp.path().join("claude");
        std::fs::create_dir_all(&parent).expect("mkdir");
        let path = parent.join("settings.json");
        let entries = vec![entry("init", HookEvent::SessionStart, "INIT")];
        merge_owned(&ClaudeAdapter, &path, &entries, false, false).expect("seed");
        let pre = std::fs::read_to_string(&path).expect("read pre");

        let report = remove_owned(&ClaudeAdapter, &path, true).expect("dry-run tidy");

        assert!(path.exists(), "dry-run must not delete the host file");
        assert!(parent.exists(), "dry-run must not rmdir the parent");
        let post = std::fs::read_to_string(&path).expect("read post");
        assert_eq!(post, pre, "dry-run must not modify the file");
        assert!(
            report.host_file_deleted,
            "dry-run report should still carry host_file_deleted: true"
        );
    }
}
