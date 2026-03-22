use indexmap::IndexMap;
use serde_json::Value;

/// Render a Markdown file with YAML frontmatter.
///
/// Matches the TypeScript `renderMarkdownWithFrontmatter`:
/// ```text
/// ---
/// {yaml, plain strings, no line wrapping}
/// ---
///
/// {body trimmed}
/// ```
pub fn render_markdown_with_frontmatter(fm: &IndexMap<String, Value>, body: &str) -> String {
    let yaml = serialize_frontmatter_yaml(fm);
    format!("---\n{yaml}\n---\n\n{}\n", body.trim())
}

/// Serialize frontmatter to YAML matching js-yaml's PLAIN string style.
///
/// We hand-roll this instead of using `serde_yml` because:
/// 1. `serde_yml` may quote strings that js-yaml leaves plain
/// 2. `serde_yml` has no `lineWidth: 0` equivalent
/// 3. We need insertion-order keys (`IndexMap`) and nested object support
fn serialize_frontmatter_yaml(fm: &IndexMap<String, Value>) -> String {
    let mut lines = Vec::new();
    for (key, value) in fm {
        serialize_yaml_value(&mut lines, key, value, 0);
    }
    lines.join("\n")
}

fn serialize_yaml_value(lines: &mut Vec<String>, key: &str, value: &Value, indent: usize) {
    let prefix = " ".repeat(indent);
    match value {
        Value::String(s) => {
            if yaml_needs_quoting(s) {
                lines.push(format!("{prefix}{key}: {}", yaml_quote(s)));
            } else {
                lines.push(format!("{prefix}{key}: {s}"));
            }
        }
        Value::Bool(b) => {
            lines.push(format!("{prefix}{key}: {b}"));
        }
        Value::Number(n) => {
            lines.push(format!("{prefix}{key}: {n}"));
        }
        Value::Null => {
            lines.push(format!("{prefix}{key}: null"));
        }
        Value::Object(map) => {
            lines.push(format!("{prefix}{key}:"));
            // Preserve insertion order if it's an IndexMap-backed object,
            // otherwise serde_json::Map iterates in its own order
            for (k, v) in map {
                serialize_yaml_value(lines, k, v, indent + 2);
            }
        }
        Value::Array(arr) => {
            lines.push(format!("{prefix}{key}:"));
            for item in arr {
                match item {
                    Value::String(s) => {
                        if yaml_needs_quoting(s) {
                            lines.push(format!("{prefix}  - {}", yaml_quote(s)));
                        } else {
                            lines.push(format!("{prefix}  - {s}"));
                        }
                    }
                    _ => {
                        lines.push(format!("{prefix}  - {item}"));
                    }
                }
            }
        }
    }
}

/// Check if a YAML string value needs quoting.
///
/// js-yaml with `defaultStringType: "PLAIN"` leaves most strings unquoted.
/// We quote only when necessary for valid YAML parsing.
fn yaml_needs_quoting(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }

    // Values that YAML parsers interpret as non-string types
    let lower = s.to_lowercase();
    if matches!(
        lower.as_str(),
        "true" | "false" | "yes" | "no" | "on" | "off" | "null" | "~"
    ) {
        return true;
    }

    // Starts with special YAML indicators
    if s.starts_with([
        '{', '}', '[', ']', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`',
    ]) {
        return true;
    }

    // Contains characters that would break plain scalar parsing
    if s.contains(": ")
        || s.contains(" #")
        || s.contains('\n')
        || s.starts_with("- ")
        || s.starts_with("? ")
    {
        return true;
    }

    // Looks like a number
    if s.parse::<f64>().is_ok() && !s.contains(|c: char| c.is_alphabetic()) {
        return true;
    }

    false
}

/// Quote a YAML string value using single quotes (matching js-yaml behavior).
fn yaml_quote(s: &str) -> String {
    // Single-quote, escaping embedded single quotes by doubling them
    let escaped = s.replace('\'', "''");
    format!("'{escaped}'")
}

/// Serialize a JSON value with sorted keys for deterministic output.
#[allow(dead_code)] // used in Phase 7 (manifest writing)
pub fn stable_json(value: &Value) -> String {
    let sorted = sort_json_keys(value);
    let mut output = serde_json::to_string_pretty(&sorted).expect("JSON serialization");
    output.push('\n');
    output
}

fn sort_json_keys(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted: serde_json::Map<String, Value> = serde_json::Map::new();
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for key in keys {
                sorted.insert(key.clone(), sort_json_keys(&map[key]));
            }
            Value::Object(sorted)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sort_json_keys).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_simple_frontmatter() {
        let mut fm = IndexMap::new();
        fm.insert("name".to_string(), json!("commit"));
        fm.insert(
            "description".to_string(),
            json!("Create git commits with user approval"),
        );

        let result = render_markdown_with_frontmatter(&fm, "# Commit\n\nBody here.\n");
        assert_eq!(
            result,
            "---\nname: commit\ndescription: Create git commits with user approval\n---\n\n# Commit\n\nBody here.\n"
        );
    }

    #[test]
    fn test_frontmatter_with_boolean() {
        let mut fm = IndexMap::new();
        fm.insert("name".to_string(), json!("test"));
        fm.insert("user-invocable".to_string(), json!(false));

        let result = render_markdown_with_frontmatter(&fm, "Body");
        assert!(result.contains("user-invocable: false"));
    }

    #[test]
    fn test_frontmatter_with_nested_object() {
        let mut fm = IndexMap::new();
        fm.insert("description".to_string(), json!("Test agent"));
        let mut tools = serde_json::Map::new();
        tools.insert("bash".to_string(), json!(true));
        tools.insert("edit".to_string(), json!(false));
        fm.insert("tools".to_string(), Value::Object(tools));

        let result = render_markdown_with_frontmatter(&fm, "Body");
        assert!(result.contains("tools:\n  bash: true\n  edit: false"));
    }

    #[test]
    fn test_frontmatter_preserves_insertion_order() {
        let mut fm = IndexMap::new();
        fm.insert("name".to_string(), json!("test"));
        fm.insert("description".to_string(), json!("A test"));
        fm.insert("model".to_string(), json!("opus"));

        let result = render_markdown_with_frontmatter(&fm, "Body");
        let yaml_section = result.split("---").nth(1).unwrap().trim();
        let keys: Vec<&str> = yaml_section
            .lines()
            .map(|l| l.split(':').next().unwrap())
            .collect();
        assert_eq!(keys, vec!["name", "description", "model"]);
    }

    #[test]
    fn test_body_is_trimmed() {
        let mut fm = IndexMap::new();
        fm.insert("name".to_string(), json!("test"));

        let result = render_markdown_with_frontmatter(&fm, "\n  Body with whitespace  \n\n");
        assert!(result.ends_with("Body with whitespace\n"));
    }

    #[test]
    fn test_yaml_quoting_booleans() {
        assert!(yaml_needs_quoting("true"));
        assert!(yaml_needs_quoting("false"));
        assert!(yaml_needs_quoting("yes"));
        assert!(yaml_needs_quoting("no"));
    }

    #[test]
    fn test_yaml_no_quoting_plain_strings() {
        assert!(!yaml_needs_quoting("commit"));
        assert!(!yaml_needs_quoting("Create git commits"));
        assert!(!yaml_needs_quoting("Bash, Read, Write"));
    }

    #[test]
    fn test_stable_json_sorts_keys() {
        let value = json!({"z": 1, "a": 2, "m": {"z": 3, "a": 4}});
        let result = stable_json(&value);
        let lines: Vec<&str> = result.lines().collect();
        // "a" should come before "m" which comes before "z"
        assert!(result.find("\"a\"").unwrap() < result.find("\"m\"").unwrap());
        assert!(result.find("\"m\"").unwrap() < result.find("\"z\": 1").unwrap());
        assert_eq!(lines.last().unwrap(), &"}");
    }
}
