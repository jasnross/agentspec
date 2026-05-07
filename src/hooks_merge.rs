//! CST-aware merge of agentspec-emitted hook entries into hand-edited
//! `.claude/settings.json` and `.cursor/hooks.json` files.
//!
//! Phase 1's plugin-scope path writes complete `hooks.json` files. Phase 2's
//! Project/User-scope path can't replace the host config — it carries
//! permissions, env config, allowlists, and user-authored hooks that must
//! survive the sync. The merge layer uses `jsonc-parser`'s CST so comments,
//! trailing commas, key ordering, and untouched whitespace round-trip
//! byte-identical.
//!
//! Ownership is identified by the `_agentspec_id` sentinel field on each
//! entry: agentspec-owned entries are replaced wholesale on each sync; entries
//! lacking the sentinel are user-authored and stay byte-identical. This is
//! the contingent design from `thoughts/research/2026-05-04-tool-managed-json-config-merge-patterns.md` —
//! Phase 2 step 5's empirical validation gate covers the case where a provider
//! rejects the unknown sub-field.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use jsonc_parser::ParseOptions;
use jsonc_parser::cst::{CstInputValue, CstObject, CstRootNode};
use serde_json::{Map, Value, json};

use crate::adapters::{
    claude_event_name, cursor_event_name, entry_to_claude_json, entry_to_cursor_json,
};
use crate::compile::EmittedHookEntry;

// ── serde_json::Value → CstInputValue bridge ────────────────────────────────

/// Converts a `serde_json` `Value` tree to jsonc-parser's `CstInputValue` tree.
///
/// The two enums are isomorphic; this is a mechanical recursive walk. Lives
/// here rather than in `compile.rs` because it's only needed for CST insertion
/// (Phase 1's emission path serializes `Value` directly via `serde_json`).
///
/// Caller contract: `Number` values reaching this function today are integers
/// only (the `timeout` field on `EmittedHookEntry` is `u32`). `n.to_string()`
/// preserves integer formatting losslessly. If a future caller introduces
/// floats, revisit the `Number` arm — `serde_json::Number`'s `Display` impl
/// can produce non-canonical forms (`1e10` vs `10000000000.0`) that may
/// surprise downstream JSON consumers.
fn value_to_cst_input(v: Value) -> CstInputValue {
    match v {
        Value::Null => CstInputValue::Null,
        Value::Bool(b) => CstInputValue::Bool(b),
        Value::Number(n) => CstInputValue::Number(n.to_string()),
        Value::String(s) => CstInputValue::String(s),
        Value::Array(arr) => {
            CstInputValue::Array(arr.into_iter().map(value_to_cst_input).collect())
        }
        Value::Object(obj) => CstInputValue::Object(
            obj.into_iter()
                .map(|(k, v)| (k, value_to_cst_input(v)))
                .collect(),
        ),
    }
}

// ── Claude merge ────────────────────────────────────────────────────────────

/// Merge agentspec-owned hook entries into `.claude/settings.json` (or whatever
/// `settings_path` points at). User-authored entries — including any
/// `permissions`, `env`, allowlists, and hand-written hooks lacking the
/// sentinel — round-trip unchanged.
pub fn merge_claude_settings(
    settings_path: &Path,
    owned_entries: &[EmittedHookEntry],
    force: bool,
    dry_run: bool,
) -> Result<()> {
    // Skip the write entirely when the file doesn't exist and we have no
    // entries to add — avoids creating a spurious `{"hooks": {}}` settings.json
    // on a fresh project where no hooks are configured.
    if !settings_path.is_file() && owned_entries.is_empty() {
        return Ok(());
    }
    let content = read_or_empty_object(settings_path)?;
    let root = CstRootNode::parse(&content, &ParseOptions::default())
        .with_context(|| format!("failed to parse {}", settings_path.display()))?;

    // The root must be a JSON object. `_or_create` errors on non-object roots
    // even with `force` — replacing the entire file body is too destructive
    // for a guard that exists to protect user content.
    let top = root.object_value_or_create().with_context(|| {
        format!(
            "{} has a non-object root; agentspec hooks merge requires a JSON object",
            settings_path.display()
        )
    })?;
    // No entries to add and no existing `hooks` key to clean orphans from —
    // don't inject a spurious `"hooks": {}` into the user's settings.json.
    if owned_entries.is_empty() && top.get("hooks").is_none() {
        return Ok(());
    }
    // `force=true` mirrors the semantics of `--force` in `emit::write_batch`:
    // allow agentspec to overwrite user-authored content. Here that means
    // replacing a non-object `hooks` value (e.g., `null` or `"string"`) with
    // an empty object before merging. `force=false` keeps the protective
    // guard; the bail message points users at the recovery flag.
    let hooks_obj = if force {
        top.object_value_or_set("hooks")
    } else {
        top.object_value_or_create("hooks").with_context(|| {
            format!(
                "{}: top-level `hooks` exists but is not an object; refusing to overwrite. Pass --force to replace.",
                settings_path.display()
            )
        })?
    };

    // Step 1 — Remove every agentspec-owned entry under every event. We can't
    // restrict to events present in `owned_entries` because re-syncing with one
    // fewer hook must remove the orphan from its old event too.
    remove_claude_owned_entries(&hooks_obj);

    // Step 2 — Append new entries grouped by `(event, matcher)`. Each new
    // matcher group is appended fresh; we don't merge into existing user-owned
    // groups so user formatting in those groups is untouched.
    //
    // `BTreeMap` sort order matters: when no event key exists yet,
    // `array_value_or_create` creates it at end-of-object, so new keys land in
    // alphabetical order. This matches `build_claude_hooks_json`'s emission
    // ordering — observable in user diffs of `settings.json`.
    let mut grouped: std::collections::BTreeMap<&'static str, Vec<&EmittedHookEntry>> =
        std::collections::BTreeMap::new();
    for e in owned_entries {
        grouped
            .entry(claude_event_name(e.event))
            .or_default()
            .push(e);
    }
    for (event_name, entries) in &grouped {
        // Mirrors the outer `hooks` guard: `force=true` replaces a non-array
        // per-event value (e.g., `"PreToolUse": "string"`) with `[]` and
        // proceeds; `force=false` errors with `--force` as the recovery.
        let event_arr = if force {
            hooks_obj.array_value_or_set(event_name)
        } else {
            hooks_obj
                .array_value_or_create(event_name)
                .with_context(|| {
                    format!(
                        "{}: `hooks.{event_name}` exists but is not an array; refusing to overwrite. Pass --force to replace.",
                        settings_path.display()
                    )
                })?
        };
        // Within an event, group consecutive entries by matcher into one
        // matcher-wrapper object (Claude's documented shape). Insertion order
        // is preserved from the spec list (which preserves IndexMap order from
        // hooks.toml).
        let mut by_matcher: indexmap::IndexMap<Option<String>, Vec<&EmittedHookEntry>> =
            indexmap::IndexMap::new();
        for &e in entries {
            by_matcher.entry(e.matcher.clone()).or_default().push(e);
        }
        for (matcher, group_entries) in by_matcher {
            let mut wrapper = Map::new();
            if let Some(m) = matcher {
                wrapper.insert("matcher".to_string(), json!(m));
            }
            let inner: Vec<Value> = group_entries
                .iter()
                .map(|e| entry_to_claude_json(e))
                .collect();
            wrapper.insert("hooks".to_string(), Value::Array(inner));
            event_arr.append(value_to_cst_input(Value::Object(wrapper)));
        }
    }

    finish(&root, settings_path, dry_run)
}

/// Walks `hooks.<event>[<matcher_group>].hooks[]` removing any entry tagged
/// with `_agentspec_id`. If a matcher group ends up with an empty `hooks`
/// array, the group itself is removed (no point keeping a wrapper around
/// nothing). Empty event arrays are left alone — the user might still have
/// entries to add later.
fn remove_claude_owned_entries(hooks_obj: &CstObject) {
    let event_props: Vec<_> = hooks_obj.properties();
    for event_prop in event_props {
        let Some(event_arr) = event_prop.array_value() else {
            continue;
        };
        let groups: Vec<_> = event_arr.elements();
        for group_node in groups {
            // Each event-array element should be a matcher-group object. If
            // it's anything else (malformed or a future schema we don't
            // recognize), leave it alone.
            let Some(group_obj) = node_as_object(&group_node) else {
                continue;
            };

            let Some(inner) = group_obj.array_value("hooks") else {
                continue;
            };
            let inner_entries: Vec<_> = inner.elements();
            for entry in inner_entries {
                if is_owned_entry(&entry) {
                    entry.remove();
                }
            }
            // If the group's hooks array is now empty, prune the wrapper.
            if group_obj
                .array_value("hooks")
                .is_some_and(|a| a.elements().is_empty())
            {
                group_obj.remove();
            }
        }
    }
}

fn is_owned_entry(node: &jsonc_parser::cst::CstNode) -> bool {
    node_as_object(node)
        .and_then(|o| o.get("_agentspec_id"))
        .is_some()
}

/// Cast a `CstNode` to a `CstObject` if it represents one. Returns `None` for
/// arrays, scalars, comments, and whitespace tokens. (jsonc-parser exposes the
/// shape via the container-node enum; this helper hides the pattern-match.)
fn node_as_object(node: &jsonc_parser::cst::CstNode) -> Option<CstObject> {
    use jsonc_parser::cst::{CstContainerNode, CstNode};
    match node {
        CstNode::Container(CstContainerNode::Object(obj)) => Some(obj.clone()),
        CstNode::Container(
            CstContainerNode::Root(_)
            | CstContainerNode::Array(_)
            | CstContainerNode::ObjectProp(_),
        )
        | CstNode::Leaf(_) => None,
    }
}

// ── Cursor merge ────────────────────────────────────────────────────────────

/// Merge agentspec-owned hook entries into `.cursor/hooks.json`.
///
/// Cursor's shape is flatter than Claude's: each event maps directly to a
/// list of entries, and `matcher` lives on each entry. Top-level requires a
/// `version: 1` field — we set it if absent.
pub fn merge_cursor_hooks(
    hooks_path: &Path,
    owned_entries: &[EmittedHookEntry],
    force: bool,
    dry_run: bool,
) -> Result<()> {
    if !hooks_path.is_file() && owned_entries.is_empty() {
        return Ok(());
    }
    let content = read_or_empty_object(hooks_path)?;
    let root = CstRootNode::parse(&content, &ParseOptions::default())
        .with_context(|| format!("failed to parse {}", hooks_path.display()))?;

    let top = root.object_value_or_create().with_context(|| {
        format!(
            "{} has a non-object root; agentspec hooks merge requires a JSON object",
            hooks_path.display()
        )
    })?;
    // No entries to add and no existing `hooks` key to clean orphans from —
    // don't inject `version: 1` or `"hooks": {}` into a file we have no
    // business touching. Mirrors the Claude `merge_claude_settings` guard.
    if owned_entries.is_empty() && top.get("hooks").is_none() {
        return Ok(());
    }

    // Set `version: 1` if missing. Don't overwrite a user-authored value, even
    // if it's a different version — the user's intent wins.
    if top.get("version").is_none() {
        top.append("version", CstInputValue::Number("1".to_string()));
    }

    // Mirrors `merge_claude_settings`: `force=true` lets agentspec replace a
    // non-object `hooks` value with `{}`; `force=false` errors with a pointer
    // to `--force` as the recovery.
    let hooks_obj = if force {
        top.object_value_or_set("hooks")
    } else {
        top.object_value_or_create("hooks").with_context(|| {
            format!(
                "{}: top-level `hooks` exists but is not an object; refusing to overwrite. Pass --force to replace.",
                hooks_path.display()
            )
        })?
    };

    // Step 1 — remove every agentspec-owned entry under every event.
    remove_cursor_owned_entries(&hooks_obj);

    // Step 2 — append new entries directly under their event arrays.
    // `BTreeMap` sort order matches the Claude path (and the build_*_hooks_json
    // emission order), so newly-created event keys land alphabetically.
    let mut by_event: std::collections::BTreeMap<&'static str, Vec<&EmittedHookEntry>> =
        std::collections::BTreeMap::new();
    for e in owned_entries {
        by_event
            .entry(cursor_event_name(e.event))
            .or_default()
            .push(e);
    }
    for (event_name, entries) in &by_event {
        // Mirrors the Claude path: `force=true` replaces a non-array per-event
        // value with `[]`; `force=false` errors with `--force` as recovery.
        let event_arr = if force {
            hooks_obj.array_value_or_set(event_name)
        } else {
            hooks_obj
                .array_value_or_create(event_name)
                .with_context(|| {
                    format!(
                        "{}: `hooks.{event_name}` exists but is not an array; refusing to overwrite. Pass --force to replace.",
                        hooks_path.display()
                    )
                })?
        };
        for &e in entries {
            event_arr.append(value_to_cst_input(entry_to_cursor_json(e)));
        }
    }

    finish(&root, hooks_path, dry_run)
}

fn remove_cursor_owned_entries(hooks_obj: &CstObject) {
    let event_props: Vec<_> = hooks_obj.properties();
    for event_prop in event_props {
        let Some(event_arr) = event_prop.array_value() else {
            continue;
        };
        let entries: Vec<_> = event_arr.elements();
        for entry in entries {
            if is_owned_entry(&entry) {
                entry.remove();
            }
        }
    }
}

// ── shared file I/O ─────────────────────────────────────────────────────────

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
        eprintln!("would merge {} bytes into {}", output.len(), path.display());
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

        merge_claude_settings(&path, &entries, false, false).expect("merge");

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
        merge_claude_settings(&path, &entries, false, false).expect("merge");

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
        merge_claude_settings(&path, &entries, false, false).expect("merge");

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
        // `remove_claude_owned_entries` deliberately does NOT prune empty
        // event arrays — the user might still have entries to add later, and
        // touching the event key shape is more invasive than necessary.
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
        merge_claude_settings(&path, &entries_v1, false, false).expect("merge v1");
        let entries_v2 = vec![entry("init", HookEvent::SessionStart, "INIT")];
        merge_claude_settings(&path, &entries_v2, false, false).expect("merge v2");

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
        merge_claude_settings(&path, &entries_v1, false, false).expect("merge v1");
        let after_v1 = std::fs::read_to_string(&path).expect("read v1");
        assert!(after_v1.contains("\"INIT\"") && after_v1.contains("\"AUDIT\""));

        // Re-sync with `audit` removed — it must disappear.
        let entries_v2 = vec![entry("init", HookEvent::SessionStart, "INIT")];
        merge_claude_settings(&path, &entries_v2, false, false).expect("merge v2");
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
        merge_cursor_hooks(&path, &entries, false, false).expect("merge");

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
        merge_cursor_hooks(&path, &entries, false, false).expect("merge");

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
        merge_claude_settings(&path, &entries, false, false)
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
        merge_claude_settings(&path, &[], false, false).expect("merge");
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

        merge_cursor_hooks(&path, &[], false, false).expect("merge");

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

        merge_claude_settings(&path, &[], false, false).expect("merge");

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
        let err = merge_claude_settings(&path, &entries, false, false)
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
        merge_claude_settings(&path, &entries, true, false).expect("force merge");

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
        merge_claude_settings(&path, &entries, true, false).expect("force merge");

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
        let err = merge_claude_settings(&path, &entries, false, false)
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
        merge_cursor_hooks(&path, &entries, true, false).expect("force merge");

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
        merge_claude_settings(&path, &entries, false, true).expect("dry-run merge");
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
        merge_claude_settings(&path, &entries, false, false).expect("merge 1");
        let after_1 = std::fs::read_to_string(&path).expect("read 1");

        merge_claude_settings(&path, &entries, false, false).expect("merge 2");
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
        merge_claude_settings(&path, &entries, false, false).expect("merge");

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
        let v = entry_to_claude_json(&e);
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
        let v = entry_to_cursor_json(&e);
        assert_eq!(v["matcher"], "Bash");
        assert_eq!(v["_agentspec_id"], "audit");
    }
}
