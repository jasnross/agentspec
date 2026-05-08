//! CST-aware merge of agentspec-emitted hook entries into hand-edited
//! `.claude/settings.json` and `.cursor/hooks.json` files.
//!
//! The Bundled (plugin-scope) emission path writes complete `hooks.json` files.
//! The Merged (Project/User-scope) path can't replace the host config — it
//! carries permissions, env config, allowlists, and user-authored hooks that
//! must survive the sync. The merge layer uses `jsonc-parser`'s CST so
//! comments, trailing commas, key ordering, and untouched whitespace
//! round-trip byte-identical.
//!
//! Ownership is identified by the `_agentspec_id` sentinel field on each
//! entry: agentspec-owned entries are replaced wholesale on each sync; entries
//! lacking the sentinel are user-authored and stay byte-identical. This is
//! the contingent design from `thoughts/research/2026-05-04-tool-managed-json-config-merge-patterns.md` —
//! empirical verification against real provider builds covers the case where
//! a provider rejects the unknown sub-field.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use jsonc_parser::ParseOptions;
use jsonc_parser::cst::{CstInputValue, CstObject, CstRootNode};
use serde_json::{Map, Value, json};

use crate::adapters::{ClaudeAdapter, CursorAdapter, HookAdapter};
use crate::compile::EmittedHookEntry;
use crate::plan::{RemovePatchReport, delete_host_file_and_rmdir_parent};

/// Outcome of a per-provider tidy closure. The closure mutates the post-tidy
/// CST root in place and reports both how many user-authored entries survived
/// (for the existing summary line) and whether the file is effectively empty
/// per the provider's predicate (the new delete-on-empty branch in
/// [`tidy_jsonc_file`]). `removed_owned` is informational here — the closure
/// itself uses it to gate `file_should_be_deleted`.
struct TidyOutcome {
    user_entries_remaining: usize,
    file_should_be_deleted: bool,
}

// ── serde_json::Value → CstInputValue bridge ────────────────────────────────

/// Converts a `serde_json` `Value` tree to jsonc-parser's `CstInputValue` tree.
///
/// The two enums are isomorphic; this is a mechanical recursive walk. Lives
/// here rather than in `compile.rs` because it's only needed for CST insertion
/// (the bundled emission path serializes `Value` directly via `serde_json`).
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
    // `force=true` mirrors the semantics of `--force` in `emit::write_manifest_tracked`:
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
    // fewer hook must remove the orphan from its old event too. Sync doesn't
    // care about the removed-count, so discard.
    let _ = remove_claude_owned_entries(&hooks_obj);

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
            .entry(ClaudeAdapter.event_name(e.event))
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
                .map(|e| ClaudeAdapter.entry_to_json(e))
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
///
/// Returns the count of `_agentspec_id`-tagged entries that were removed.
/// The remove-side tidy uses this as the "we actually owned something here"
/// guard before deciding whether the host file is effectively empty; sync
/// callers can ignore the count.
fn remove_claude_owned_entries(hooks_obj: &CstObject) -> usize {
    let mut removed = 0usize;
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
                    removed += 1;
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
    removed
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

    // Step 1 — remove every agentspec-owned entry under every event. Sync
    // doesn't care about the removed-count, so discard.
    let _ = remove_cursor_owned_entries(&hooks_obj);

    // Step 2 — append new entries directly under their event arrays.
    // `BTreeMap` sort order matches the Claude path (and the build_*_hooks_json
    // emission order), so newly-created event keys land alphabetically.
    let mut by_event: std::collections::BTreeMap<&'static str, Vec<&EmittedHookEntry>> =
        std::collections::BTreeMap::new();
    for e in owned_entries {
        by_event
            .entry(CursorAdapter.event_name(e.event))
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
            event_arr.append(value_to_cst_input(CursorAdapter.entry_to_json(e)));
        }
    }

    finish(&root, hooks_path, dry_run)
}

/// Cursor analog of [`remove_claude_owned_entries`]. Cursor's shape is one
/// nesting level shallower (no matcher-group wrapper), so this walks
/// `hooks.<event>[]` directly. Returns the count of `_agentspec_id`-tagged
/// entries removed; sync callers can ignore the count.
fn remove_cursor_owned_entries(hooks_obj: &CstObject) -> usize {
    let mut removed = 0usize;
    let event_props: Vec<_> = hooks_obj.properties();
    for event_prop in event_props {
        let Some(event_arr) = event_prop.array_value() else {
            continue;
        };
        let entries: Vec<_> = event_arr.elements();
        for entry in entries {
            if is_owned_entry(&entry) {
                entry.remove();
                removed += 1;
            }
        }
    }
    removed
}

// ── remove (tidy) entry points ──────────────────────────────────────────────
//
// Every `.remove()` call below lands on a value-bearing CST node
// (`CstObjectProp::remove`, `CstObject::remove`); none touch `CstWhitespace`,
// `CstNewline`, or `CstToken` directly. That preserves jsonc-parser's
// trivia-aware delete semantics — comments and surrounding whitespace stay
// attached to whatever survives.

/// Reverses `merge_claude_settings`'s effect on `<config_dir>/settings.json`.
///
/// Removes every `_agentspec_id`-tagged entry, then performs maximum-depth
/// tidy: empty matcher-group wrappers (handled by `remove_claude_owned_entries`),
/// empty event arrays, and the top-level `hooks` key when it becomes `{}`.
/// User-authored entries, comments, and surrounding formatting round-trip
/// unchanged via the `jsonc-parser` CST API.
///
/// The host file is **deleted** when (a) tidy actually removed at least one
/// agentspec-owned entry, and (b) no top-level keys survive the tidy. After
/// a delete, the host file's parent directory is best-effort `rmdir`'d. If
/// the user had any non-hook top-level keys (e.g. `permissions`, `env`),
/// the file survives untouched.
///
/// Tidy contract diverges from sync's `remove_claude_owned_entries` (which
/// deliberately leaves empty event arrays alone — locked by
/// `test_merge_claude_leaves_empty_event_array_after_removing_all_owned_entries`).
/// Sync's behavior is preserved unchanged; the additional pruning lives in
/// this dedicated path so the merge contract is unaffected.
pub fn remove_claude_settings(settings_path: &Path, dry_run: bool) -> Result<RemovePatchReport> {
    tidy_jsonc_file(settings_path, dry_run, tidy_claude_settings_after_remove)
}

/// Reverses `merge_cursor_hooks`'s effect on `<config_dir>/hooks.json`.
///
/// Cursor's hooks shape is one nesting level shallower than Claude's (no
/// matcher-group wrapper), so tidy is correspondingly simpler: drop owned
/// entries, drop empty event arrays, drop the top-level `hooks` key if it
/// becomes `{}`. User-authored top-level keys (other than `version`) survive
/// untouched.
///
/// The host file is **deleted** when (a) tidy actually removed at least one
/// agentspec-owned entry, and (b) the residual file is either empty or
/// contains only a `version` key (any value). After a delete, the host
/// file's parent directory is best-effort `rmdir`'d. The `version`-only
/// carve-out is Cursor-specific — sync injects `version: 1` if absent and
/// never overwrites a user value, so a residual `{version: <n>}` carries no
/// information beyond file existence and is informationally equivalent to
/// no file.
pub fn remove_cursor_hooks(hooks_path: &Path, dry_run: bool) -> Result<RemovePatchReport> {
    tidy_jsonc_file(hooks_path, dry_run, tidy_cursor_hooks_after_remove)
}

/// Shared body for [`remove_claude_settings`] and [`remove_cursor_hooks`]:
/// read the host file, run the provider-specific tidy, then either delete
/// the host file (when the closure reports `file_should_be_deleted`) or
/// write back atomically. After a delete, the host file's parent directory
/// is best-effort `rmdir`'d via [`try_rmdir_if_empty`]; the parent stays if
/// it has any other content.
///
/// Returns the default report when the host file is absent, or a
/// zero-entry-count report (with a warning to stderr) when the file's root
/// is not an object — both states are tolerated as "nothing to clean".
fn tidy_jsonc_file<F>(path: &Path, dry_run: bool, tidy: F) -> Result<RemovePatchReport>
where
    F: FnOnce(&CstObject) -> TidyOutcome,
{
    if !path.is_file() {
        return Ok(RemovePatchReport::default());
    }
    let content = read_or_empty_object(path)?;
    let root = CstRootNode::parse(&content, &ParseOptions::default())
        .with_context(|| format!("failed to parse {}", path.display()))?;

    let Some(top) = root.object_value_or_create() else {
        let prefix = if dry_run { "[dry-run] " } else { "" };
        eprintln!(
            "{prefix}warning: {} has a non-object root; skipping tidy",
            path.display()
        );
        return Ok(RemovePatchReport {
            host_path: path.to_path_buf(),
            user_entries_remaining: 0,
            host_file_deleted: false,
            parent_rmdir: false,
        });
    };

    let outcome = tidy(&top);

    if outcome.file_should_be_deleted {
        let parent_rmdir = delete_host_file_and_rmdir_parent(path, dry_run)?;
        return Ok(RemovePatchReport {
            host_path: path.to_path_buf(),
            user_entries_remaining: 0,
            host_file_deleted: true,
            parent_rmdir,
        });
    }

    finish(&root, path, dry_run)?;

    Ok(RemovePatchReport {
        host_path: path.to_path_buf(),
        user_entries_remaining: outcome.user_entries_remaining,
        host_file_deleted: false,
        parent_rmdir: false,
    })
}

/// Removes every `_agentspec_id`-tagged Claude hook entry, then prunes empty
/// containers up the tree (matcher groups → event arrays → top-level `hooks`).
/// Returns a [`TidyOutcome`] describing surviving user entries and whether
/// the host file is now effectively empty (the predicate that drives the
/// delete-on-empty branch in [`tidy_jsonc_file`]).
///
/// `top` mutates via `jsonc-parser`'s interior-mutability CST API — the
/// `&CstObject` signature reads as inspection-only, but `.remove()` calls on
/// child nodes propagate through to the shared root.
fn tidy_claude_settings_after_remove(top: &CstObject) -> TidyOutcome {
    let Some(hooks_obj) = top.object_value("hooks") else {
        return TidyOutcome {
            user_entries_remaining: 0,
            file_should_be_deleted: false,
        };
    };

    // Step 1 — defer to the existing sync helper for the inner-most layer
    // (entry removal + matcher-group pruning when the inner `hooks` array
    // empties). Reusing the helper keeps sync's contract — and its locked
    // test — unchanged.
    let removed_owned = remove_claude_owned_entries(&hooks_obj);

    // Step 2 — drop event arrays that are now empty. Sync deliberately
    // doesn't do this (so user keys survive cross-sync), but on remove there
    // are no further sync writes coming, so leaving empty arrays is just
    // visual clutter.
    //
    // Materialize the property list before mutating, mirroring
    // `remove_claude_owned_entries`. `properties()` returns an owned `Vec`
    // today, but the explicit collect keeps the pattern consistent and
    // immune to a future jsonc-parser API change.
    let event_props: Vec<_> = hooks_obj.properties();
    for event_prop in event_props {
        if event_prop
            .array_value()
            .is_some_and(|arr| arr.elements().is_empty())
        {
            event_prop.remove();
        }
    }

    // Step 3 — if the `hooks` object itself is now empty, drop the top-level
    // key. `top.get("hooks")` returns the CstObjectProp; `.remove()` is the
    // trivia-aware variant.
    if hooks_obj.properties().is_empty()
        && let Some(hooks_prop) = top.get("hooks")
    {
        hooks_prop.remove();
    }

    // Claude predicate: delete the host file iff we actually removed at
    // least one agentspec-owned entry AND no top-level keys survive. Claude
    // settings.json doesn't use a `version` key, so there's no carve-out —
    // any surviving top-level key (e.g. `permissions`, `env`) keeps the file.
    let file_should_be_deleted = removed_owned > 0 && top.properties().is_empty();

    TidyOutcome {
        user_entries_remaining: count_claude_user_entries(top),
        file_should_be_deleted,
    }
}

/// Cursor analog of [`tidy_claude_settings_after_remove`]. The structure is
/// shallower (each event maps directly to a list of entries — no
/// matcher-group wrapper), so tidy is one level less deep.
///
/// The Cursor predicate is **the only place** where `version: <n>` residue
/// is tolerated. Sync injects `version: 1` if absent and never overwrites a
/// user value, so a file containing only `{version: <n>}` after tidy is
/// either agentspec's injection or a near-empty hand-edit. The
/// `removed_owned > 0` guard makes deletion safe in both cases — the user
/// invited cleanup by running remove, and the residual `version` key
/// carries no information beyond file existence.
///
/// `top` mutates via interior mutability; see the Claude variant's docstring
/// for the contract.
fn tidy_cursor_hooks_after_remove(top: &CstObject) -> TidyOutcome {
    let Some(hooks_obj) = top.object_value("hooks") else {
        return TidyOutcome {
            user_entries_remaining: 0,
            file_should_be_deleted: false,
        };
    };

    let removed_owned = remove_cursor_owned_entries(&hooks_obj);

    let event_props: Vec<_> = hooks_obj.properties();
    for event_prop in event_props {
        if event_prop
            .array_value()
            .is_some_and(|arr| arr.elements().is_empty())
        {
            event_prop.remove();
        }
    }

    if hooks_obj.properties().is_empty()
        && let Some(hooks_prop) = top.get("hooks")
    {
        hooks_prop.remove();
    }

    // Cursor predicate: delete the host file iff we actually removed at
    // least one agentspec-owned entry AND the residual content is either
    // empty OR exactly one `version` key (any value). Cursor-exclusive —
    // Claude/OpenCode predicates don't tolerate residue.
    let surviving = top.properties();
    let only_version_remains = surviving.len() == 1 && top.get("version").is_some();
    let file_should_be_deleted =
        removed_owned > 0 && (surviving.is_empty() || only_version_remains);

    TidyOutcome {
        user_entries_remaining: count_cursor_user_entries(top),
        file_should_be_deleted,
    }
}

/// Counts user-authored Claude hook entries: walks every surviving matcher
/// group's inner `hooks` array and counts entries lacking `_agentspec_id`.
/// Owned entries should be gone by the time this runs, but the
/// non-owned-only check is defensive.
fn count_claude_user_entries(top: &CstObject) -> usize {
    let Some(hooks_obj) = top.object_value("hooks") else {
        return 0;
    };
    let mut count = 0;
    for event_prop in hooks_obj.properties() {
        let Some(event_arr) = event_prop.array_value() else {
            continue;
        };
        for group_node in event_arr.elements() {
            let Some(group_obj) = node_as_object(&group_node) else {
                continue;
            };
            let Some(inner) = group_obj.array_value("hooks") else {
                continue;
            };
            for entry in inner.elements() {
                if !is_owned_entry(&entry) {
                    count += 1;
                }
            }
        }
    }
    count
}

/// Counts user-authored Cursor hook entries: walks every surviving event
/// array and counts elements lacking `_agentspec_id`.
fn count_cursor_user_entries(top: &CstObject) -> usize {
    let Some(hooks_obj) = top.object_value("hooks") else {
        return 0;
    };
    let mut count = 0;
    for event_prop in hooks_obj.properties() {
        let Some(event_arr) = event_prop.array_value() else {
            continue;
        };
        for entry in event_arr.elements() {
            if !is_owned_entry(&entry) {
                count += 1;
            }
        }
    }
    count
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
        merge_claude_settings(&path, &entries, false, false).expect("seed");

        let report = remove_claude_settings(&path, false).expect("tidy");

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

        let report = remove_claude_settings(&path, false).expect("tidy");

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

        let report = remove_claude_settings(&path, false).expect("tidy");

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
        merge_cursor_hooks(&path, &entries, false, false).expect("seed");

        let report = remove_cursor_hooks(&path, false).expect("tidy");

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

        let report = remove_cursor_hooks(&path, false).expect("tidy");

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

        let report = remove_cursor_hooks(&path, false).expect("tidy");

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

        let report = remove_cursor_hooks(&path, false).expect("tidy");

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
        merge_claude_settings(&path, &entries, false, false).expect("seed");
        let pre = std::fs::read_to_string(&path).expect("read pre");

        let report = remove_claude_settings(&path, true).expect("dry-run tidy");

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
