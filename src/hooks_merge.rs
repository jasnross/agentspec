//! Generic CST-aware merge shell for adapter-supplied hook patches.
//!
//! The Bundled (plugin-scope) emission path writes complete `hooks.json` files.
//! The Merged (Project/User-scope) path can't replace the host config — it
//! carries permissions, env config, allowlists, and user-authored hooks that
//! must survive the sync. This module is the generic plumbing for that path:
//! file I/O, CST parse, top-level open, atomic write, and the delete-on-empty
//! tail. All provider-specific JSON-shape decisions (top-level extras,
//! per-event nesting depth, owned-entry pruning, the delete predicate, host
//! filename) live behind the per-adapter `ForwardPatch` / `ReversePatch`
//! impls in `src/adapters/*.rs`, which call into [`merge_owned`] /
//! [`remove_owned`] supplying their own merge / tidy / no-op-skip closures.
//!
//! Ownership is identified by the `_agentspec_id` sentinel field on each
//! entry: agentspec-owned entries are replaced wholesale on each sync; entries
//! lacking the sentinel are user-authored and stay byte-identical. Comments,
//! trailing commas, key ordering, and untouched whitespace round-trip
//! byte-identical via `jsonc-parser`'s CST API.

use std::path::Path;

use anyhow::{Context, Result};
use jsonc_parser::ParseOptions;
use jsonc_parser::cst::{CstObject, CstRootNode};

use crate::adapters::TidyOutcome;
use crate::cst_io::{finish, read_or_empty_object};
use crate::plan::{RemovePatchReport, delete_host_file_and_rmdir_parent};

/// Merge agentspec-owned hook entries into the host config file at
/// `host_path`. The provider-specific shape decisions are delegated to the
/// supplied closures: `no_op_skip` decides whether the merge should bail
/// without touching the file (e.g., a fresh `settings.json` with no `hooks`
/// key when the entry list is empty), and `merge_into` mutates the parsed
/// top-level object in place. This function only handles file I/O, CST
/// parse, the entry-list-empty short-circuit on a missing file, and the
/// atomic write.
pub(crate) fn merge_owned(
    host_path: &Path,
    entries_empty: bool,
    no_op_skip: impl FnOnce(&CstObject) -> bool,
    merge_into: impl FnOnce(&CstObject) -> Result<()>,
    dry_run: bool,
) -> Result<()> {
    // Skip when the file is absent and we have no entries to add — avoids
    // creating a spurious config file on a fresh project.
    if !host_path.is_file() && entries_empty {
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

    // No-op skip predicate is provider-supplied. Today every adapter passes
    // `entries_empty && top.get("hooks").is_none()` — the previous hardcoded
    // guard — but adapters that inject top-level extras (e.g. Cursor's
    // `version: 1`) might evolve different predicates without touching this
    // shell.
    if no_op_skip(&top) {
        return Ok(());
    }

    merge_into(&top).with_context(|| format!("merge into {} failed", host_path.display()))?;

    if dry_run {
        eprintln!("[dry-run] would merge hooks into {}", host_path.display());
        return Ok(());
    }

    finish(&root, host_path)
}

/// Strip agentspec-owned hook entries from the host config file at
/// `host_path` and either delete the file (when the adapter's predicate says
/// it's effectively empty) or write the tidied CST back. The
/// provider-specific `tidy_after_remove` closure mutates the parsed top
/// in place and reports whether the residual file should be deleted plus
/// how many user-authored entries survived.
pub(crate) fn remove_owned(
    host_path: &Path,
    tidy_after_remove: impl FnOnce(&CstObject) -> TidyOutcome,
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

    let outcome = tidy_after_remove(&top);

    if outcome.file_should_be_deleted {
        let parent_rmdir = delete_host_file_and_rmdir_parent(host_path, dry_run)?;
        return Ok(RemovePatchReport {
            host_path: host_path.to_path_buf(),
            user_entries_remaining: 0,
            host_file_deleted: true,
            parent_rmdir,
        });
    }

    if dry_run {
        eprintln!("[dry-run] would tidy hooks in {}", host_path.display());
        return Ok(RemovePatchReport {
            host_path: host_path.to_path_buf(),
            user_entries_remaining: outcome.user_entries_remaining,
            host_file_deleted: false,
            parent_rmdir: false,
        });
    }

    finish(&root, host_path)?;

    Ok(RemovePatchReport {
        host_path: host_path.to_path_buf(),
        user_entries_remaining: outcome.user_entries_remaining,
        host_file_deleted: false,
        parent_rmdir: false,
    })
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::adapters::{ClaudeAdapter, CursorAdapter};
    use crate::compile::EmittedHookEntry;
    use crate::spec::HookEvent;

    /// Test-only adapter shim that mirrors what each adapter's `ForwardPatch`
    /// impl supplies to `merge_owned`. Calls into the adapter's inherent
    /// `merge_into_settings` / `tidy_settings` helpers so tests exercise the
    /// real per-provider closures end-to-end (no parallel shape).
    fn merge_with_claude(
        host_path: &Path,
        entries: &[EmittedHookEntry],
        force: bool,
        dry_run: bool,
    ) -> Result<()> {
        merge_owned(
            host_path,
            entries.is_empty(),
            |top| entries.is_empty() && top.get("hooks").is_none(),
            |top| ClaudeAdapter::merge_into_settings(top, entries, force),
            dry_run,
        )
    }

    fn remove_with_claude(host_path: &Path, dry_run: bool) -> Result<RemovePatchReport> {
        remove_owned(host_path, ClaudeAdapter::tidy_settings, dry_run)
    }

    fn merge_with_cursor(
        host_path: &Path,
        entries: &[EmittedHookEntry],
        force: bool,
        dry_run: bool,
    ) -> Result<()> {
        merge_owned(
            host_path,
            entries.is_empty(),
            |top| entries.is_empty() && top.get("hooks").is_none(),
            |top| CursorAdapter::merge_into_hooks(top, entries, force),
            dry_run,
        )
    }

    fn remove_with_cursor(host_path: &Path, dry_run: bool) -> Result<RemovePatchReport> {
        remove_owned(host_path, CursorAdapter::tidy_hooks, dry_run)
    }

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

        merge_with_claude(&path, &entries, false, false).expect("merge");

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
        merge_with_claude(&path, &entries, false, false).expect("merge");

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
        merge_with_claude(&path, &entries, false, false).expect("merge");

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
        merge_with_claude(&path, &entries_v1, false, false).expect("merge v1");
        let entries_v2 = vec![entry("init", HookEvent::SessionStart, "INIT")];
        merge_with_claude(&path, &entries_v2, false, false).expect("merge v2");

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
        merge_with_claude(&path, &entries_v1, false, false).expect("merge v1");
        let after_v1 = std::fs::read_to_string(&path).expect("read v1");
        assert!(after_v1.contains("\"INIT\"") && after_v1.contains("\"AUDIT\""));

        // Re-sync with `audit` removed — it must disappear.
        let entries_v2 = vec![entry("init", HookEvent::SessionStart, "INIT")];
        merge_with_claude(&path, &entries_v2, false, false).expect("merge v2");
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
        merge_with_cursor(&path, &entries, false, false).expect("merge");

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
        merge_with_cursor(&path, &entries, false, false).expect("merge");

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
        merge_with_claude(&path, &entries, false, false)
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
        merge_with_claude(&path, &[], false, false).expect("merge");
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

        merge_with_cursor(&path, &[], false, false).expect("merge");

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

        merge_with_claude(&path, &[], false, false).expect("merge");

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
        let err = merge_with_claude(&path, &entries, false, false)
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
        merge_with_claude(&path, &entries, true, false).expect("force merge");

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
        merge_with_claude(&path, &entries, true, false).expect("force merge");

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
        let err = merge_with_claude(&path, &entries, false, false)
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
        merge_with_cursor(&path, &entries, true, false).expect("force merge");

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
        merge_with_claude(&path, &entries, false, true).expect("dry-run merge");
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
        merge_with_claude(&path, &entries, false, false).expect("merge 1");
        let after_1 = std::fs::read_to_string(&path).expect("read 1");

        merge_with_claude(&path, &entries, false, false).expect("merge 2");
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
        merge_with_claude(&path, &entries, false, false).expect("merge");

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

    // Per-adapter `entry_to_json` shape tests live in the respective adapter
    // modules (`adapters/claude.rs::tests` / `adapters/cursor.rs::tests`)
    // alongside the function being tested.

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
        merge_with_claude(&path, &entries, false, false).expect("seed");

        let report = remove_with_claude(&path, false).expect("tidy");

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

        let report = remove_with_claude(&path, false).expect("tidy");

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

        let report = remove_with_claude(&path, false).expect("tidy");

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
        merge_with_cursor(&path, &entries, false, false).expect("seed");

        let report = remove_with_cursor(&path, false).expect("tidy");

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

        let report = remove_with_cursor(&path, false).expect("tidy");

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

        let report = remove_with_cursor(&path, false).expect("tidy");

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

        let report = remove_with_cursor(&path, false).expect("tidy");

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
        merge_with_claude(&path, &entries, false, false).expect("seed");
        let pre = std::fs::read_to_string(&path).expect("read pre");

        let report = remove_with_claude(&path, true).expect("dry-run tidy");

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
