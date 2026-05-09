//! Shared CST helpers for adapter-side hook merge/tidy implementations.
//!
//! Visibility is `pub(super)` — these helpers compose into the per-provider
//! `merge_into` and `tidy_after_remove` impls in `claude.rs` and `cursor.rs`.
//! No code outside `crate::adapters` should reach for them: the generic
//! shell in `hooks_merge.rs` consumes the trait, not these primitives.

use anyhow::{Context, Result};
use jsonc_parser::cst::{CstArray, CstContainerNode, CstInputValue, CstNode, CstObject};
use serde_json::Value;

/// Converts a `serde_json::Value` tree to `jsonc-parser`'s `CstInputValue` tree.
///
/// The two enums are isomorphic; this is a mechanical recursive walk. Caller
/// contract: `Number` values reaching this function today are integers only
/// (the `timeout` field on `EmittedHookEntry` is `u32`). `n.to_string()`
/// preserves integer formatting losslessly. If a future caller introduces
/// floats, revisit the `Number` arm — `serde_json::Number`'s `Display` impl
/// can produce non-canonical forms (`1e10` vs `10000000000.0`).
pub(super) fn value_to_cst_input(v: Value) -> CstInputValue {
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

/// Returns true when `node` is a JSON object carrying the `_agentspec_id`
/// sentinel field — agentspec's marker for an owned hook entry.
pub(super) fn is_owned_entry(node: &CstNode) -> bool {
    node_as_object(node)
        .and_then(|o| o.get("_agentspec_id"))
        .is_some()
}

/// Cast a `CstNode` to a `CstObject` if it represents one. Returns `None` for
/// arrays, scalars, comments, and whitespace tokens.
pub(super) fn node_as_object(node: &CstNode) -> Option<CstObject> {
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

/// `force`-aware open of an inner object property.
///
/// `force=true` mirrors `--force` semantics: replace a non-object existing
/// value with `{}` and return the new object. `force=false` errors with a
/// `--force`-pointing recovery hint. `error_label` is a JSON-pointer-shaped
/// string used inside the error (e.g. `"hooks"`); file-path context is added
/// by the caller's outer `.with_context(...)` wrapper.
pub(super) fn open_or_create_object(
    parent: &CstObject,
    key: &str,
    force: bool,
    error_label: &str,
) -> Result<CstObject> {
    if force {
        Ok(parent.object_value_or_set(key))
    } else {
        parent.object_value_or_create(key).with_context(|| {
            format!(
                "{error_label} exists but is not an object; refusing to overwrite. Pass --force to replace."
            )
        })
    }
}

/// `force`-aware open of an inner array property. Symmetric to
/// [`open_or_create_object`].
pub(super) fn open_or_create_array(
    parent: &CstObject,
    key: &str,
    force: bool,
    error_label: &str,
) -> Result<CstArray> {
    if force {
        Ok(parent.array_value_or_set(key))
    } else {
        parent.array_value_or_create(key).with_context(|| {
            format!(
                "{error_label} exists but is not an array; refusing to overwrite. Pass --force to replace."
            )
        })
    }
}

/// Drop event keys whose array is `[]`. Used by `tidy_after_remove` paths
/// (sync intentionally leaves empty event arrays alone — see
/// `test_merge_claude_leaves_empty_event_array_after_removing_all_owned_entries`).
pub(super) fn prune_empty_event_arrays(hooks_obj: &CstObject) {
    let event_props: Vec<_> = hooks_obj.properties();
    for event_prop in event_props {
        if event_prop
            .array_value()
            .is_some_and(|arr| arr.elements().is_empty())
        {
            event_prop.remove();
        }
    }
}

#[cfg(test)]
mod tests {
    use jsonc_parser::ParseOptions;
    use jsonc_parser::cst::CstRootNode;
    use serde_json::json;

    use super::*;

    /// Parse `src` and return both the root and the top-level object. Tests
    /// must hold both: dropping the root invalidates the CST handles
    /// (jsonc-parser uses `Rc`-shared internal state rooted at `CstRootNode`).
    fn parse_top(src: &str) -> (CstRootNode, CstObject) {
        let root = CstRootNode::parse(src, &ParseOptions::default()).expect("parse");
        let top = root.object_value_or_create().expect("top object");
        (root, top)
    }

    /// Pull the first element out of `top.<key>` (which must be an array). The
    /// production callers of `is_owned_entry` and `node_as_object` always
    /// receive a `CstNode` from `CstArray::elements()`, so route the test
    /// fixtures through the same path.
    fn first_element(top: &CstObject, key: &str) -> CstNode {
        top.get(key)
            .expect("prop")
            .array_value()
            .expect("array")
            .elements()
            .into_iter()
            .next()
            .expect("element")
    }

    #[test]
    fn test_is_owned_entry_true_for_object_with_sentinel() {
        let (_root, top) = parse_top(r#"{"items": [{"_agentspec_id": "id1"}]}"#);
        assert!(is_owned_entry(&first_element(&top, "items")));
    }

    #[test]
    fn test_is_owned_entry_false_for_object_without_sentinel() {
        let (_root, top) = parse_top(r#"{"items": [{"command": "ls"}]}"#);
        assert!(!is_owned_entry(&first_element(&top, "items")));
    }

    #[test]
    fn test_is_owned_entry_false_for_non_object() {
        let (_root, top) = parse_top(r#"{"items": ["string"]}"#);
        assert!(!is_owned_entry(&first_element(&top, "items")));
    }

    #[test]
    fn test_node_as_object_some_for_object() {
        let (_root, top) = parse_top(r#"{"items": [{"k": 1}]}"#);
        assert!(node_as_object(&first_element(&top, "items")).is_some());
    }

    #[test]
    fn test_node_as_object_none_for_arrays_and_scalars() {
        let (_root, top) = parse_top(r#"{"items": [[], "str", 1, null]}"#);
        let elements: Vec<_> = top
            .get("items")
            .expect("items")
            .array_value()
            .expect("arr")
            .elements();
        for el in &elements {
            assert!(node_as_object(el).is_none(), "expected None");
        }
    }

    #[test]
    fn test_value_to_cst_input_round_trips_each_variant() {
        // Build a value covering each variant and walk the resulting CstInputValue.
        let v = json!({
            "n": null,
            "b": true,
            "i": 42,
            "s": "hi",
            "a": [1, "x"],
            "o": {"k": 1}
        });
        let CstInputValue::Object(props) = value_to_cst_input(v) else {
            panic!("expected top-level Object");
        };
        let map: std::collections::HashMap<String, CstInputValue> = props.into_iter().collect();
        assert!(matches!(map["n"], CstInputValue::Null));
        assert!(matches!(map["b"], CstInputValue::Bool(true)));
        assert!(matches!(&map["i"], CstInputValue::Number(s) if s == "42"));
        assert!(matches!(&map["s"], CstInputValue::String(s) if s == "hi"));
        assert!(matches!(map["a"], CstInputValue::Array(_)));
        assert!(matches!(map["o"], CstInputValue::Object(_)));
    }

    #[test]
    fn test_open_or_create_object_force_replaces_non_object() {
        let (_root, top) = parse_top(r#"{"hooks": null}"#);
        let obj = open_or_create_object(&top, "hooks", true, "hooks").expect("force ok");
        // Adding a key proves we got back a usable object handle on the new value.
        obj.append("k", CstInputValue::Bool(true));
        assert!(
            top.get("hooks").expect("hooks").object_value().is_some(),
            "force should have replaced null with an object"
        );
    }

    #[test]
    fn test_open_or_create_object_no_force_errors_with_force_hint() {
        let (_root, top) = parse_top(r#"{"hooks": null}"#);
        let err =
            open_or_create_object(&top, "hooks", false, "hooks").expect_err("expected refusal");
        let msg = format!("{err:#}");
        assert!(msg.contains("not an object"), "got: {msg}");
        assert!(msg.contains("--force"), "got: {msg}");
    }

    #[test]
    fn test_open_or_create_array_force_replaces_non_array() {
        let (_root, top) = parse_top(r#"{"events": "weird"}"#);
        let arr = open_or_create_array(&top, "events", true, "events").expect("force ok");
        arr.append(CstInputValue::Bool(true));
        assert!(
            top.get("events").expect("events").array_value().is_some(),
            "force should have replaced string with an array"
        );
    }

    #[test]
    fn test_open_or_create_array_no_force_errors_with_force_hint() {
        let (_root, top) = parse_top(r#"{"events": "weird"}"#);
        let err = open_or_create_array(&top, "events", false, "hooks.PreToolUse")
            .expect_err("expected refusal");
        let msg = format!("{err:#}");
        assert!(msg.contains("not an array"), "got: {msg}");
        assert!(msg.contains("--force"), "got: {msg}");
        assert!(msg.contains("hooks.PreToolUse"), "got: {msg}");
    }

    #[test]
    fn test_prune_empty_event_arrays_drops_empty_keeps_nonempty() {
        let (_root, top) =
            parse_top(r#"{"PreToolUse": [], "SessionStart": [{"k":1}], "Stop": []}"#);
        prune_empty_event_arrays(&top);
        assert!(top.get("PreToolUse").is_none(), "empty PreToolUse pruned");
        assert!(top.get("Stop").is_none(), "empty Stop pruned");
        assert!(top.get("SessionStart").is_some(), "non-empty kept");
    }
}
