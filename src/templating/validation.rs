use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Result, bail};
use regex::Regex;

#[allow(clippy::expect_used)] // literal regex pattern; failure is a programmer error
static BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{%-?\s*block\s+(\w+)(?:\s+required)?\s*-?%\}").expect("expected valid regex")
});

#[allow(clippy::expect_used)] // literal regex pattern; failure is a programmer error
static EXTENDS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\{%-?\s*extends\s+"(templates/[^"]+)"\s*-?%\}"#).expect("expected valid regex")
});

fn extract_block_names(source: &str) -> HashSet<String> {
    BLOCK_RE
        .captures_iter(source)
        .map(|cap| cap[1].to_string())
        .collect()
}

fn extract_extends_target(source: &str) -> Option<String> {
    EXTENDS_RE.captures(source).map(|cap| cap[1].to_string())
}

/// Validate that every block override in a child spec exists in the parent
/// template chain. Returns `Ok(())` for specs that don't use `{% extends %}`.
pub(super) fn validate_child_blocks(
    child_source: &str,
    resolve_template: &dyn Fn(&str) -> Result<Option<String>>,
    spec_path: &Path,
) -> Result<()> {
    let Some(parent_name) = extract_extends_target(child_source) else {
        return Ok(());
    };

    let child_blocks = extract_block_names(child_source);
    if child_blocks.is_empty() {
        return Ok(());
    }

    let mut parent_blocks = HashSet::new();
    let mut visited = HashSet::new();
    let mut current = Some(parent_name.clone());

    while let Some(ref name) = current {
        if !visited.insert(name.clone()) {
            bail!(
                "circular extends chain detected: \"{}\" appears twice in the \
                 inheritance chain for {}",
                name,
                spec_path.display(),
            );
        }

        let Some(parent_source) = resolve_template(name)? else {
            // Template not found — let MiniJinja surface its own
            // "template not found" error during render.
            return Ok(());
        };

        parent_blocks.extend(extract_block_names(&parent_source));
        current = extract_extends_target(&parent_source);
    }

    let unrecognized: Vec<&str> = child_blocks
        .iter()
        .filter(|b| !parent_blocks.contains(b.as_str()))
        .map(String::as_str)
        .collect();

    if !unrecognized.is_empty() {
        let mut sorted = unrecognized;
        sorted.sort_unstable();
        let names = sorted
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "{} overrides block(s) {names} not defined in template \
             \"{parent_name}\" or its parent chain",
            spec_path.display(),
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn resolver(map: &HashMap<String, String>) -> impl Fn(&str) -> Result<Option<String>> + '_ {
        move |name: &str| Ok(map.get(name).cloned())
    }

    #[test]
    fn test_extract_block_names_basic() {
        let source = "{% block title %}some content{% endblock %}";
        let names = extract_block_names(source);
        assert_eq!(names, HashSet::from(["title".to_string()]));
    }

    #[test]
    fn test_extract_block_names_required() {
        let source = "{% block title required %}{% endblock %}";
        let names = extract_block_names(source);
        assert_eq!(names, HashSet::from(["title".to_string()]));
    }

    #[test]
    fn test_extract_block_names_whitespace_variants() {
        let trimmed = "{%- block name -%}content{%- endblock -%}";
        let normal = "{% block name %}content{% endblock %}";
        assert_eq!(extract_block_names(trimmed), extract_block_names(normal),);
    }

    #[test]
    fn test_extract_block_names_multiple() {
        let source = concat!(
            "{% block header %}h{% endblock %}",
            "{% block body %}b{% endblock %}",
            "{% block footer %}f{% endblock %}"
        );
        let names = extract_block_names(source);
        assert_eq!(names.len(), 3);
        assert!(names.contains("header"));
        assert!(names.contains("body"));
        assert!(names.contains("footer"));
    }

    #[test]
    fn test_extract_extends_target_present() {
        let source = "{% extends \"templates/critique.md\" %}{% block x %}y{% endblock %}";
        assert_eq!(
            extract_extends_target(source),
            Some("templates/critique.md".to_string()),
        );
    }

    #[test]
    fn test_extract_extends_target_absent() {
        let source = "Plain body with no extends.";
        assert_eq!(extract_extends_target(source), None);
    }

    #[test]
    fn test_extract_extends_target_non_template() {
        let source = "{% extends \"not-a-template.md\" %}";
        assert_eq!(extract_extends_target(source), None);
    }

    #[test]
    fn test_validate_child_blocks_all_known() {
        let templates = HashMap::from([(
            "templates/base.md".to_string(),
            "{% block title %}t{% endblock %}{% block body %}b{% endblock %}".to_string(),
        )]);
        let child = concat!(
            "{% extends \"templates/base.md\" %}",
            "{% block title %}override{% endblock %}"
        );
        validate_child_blocks(child, &resolver(&templates), Path::new("spec.md"))
            .expect("expected Ok for known blocks");
    }

    #[test]
    fn test_validate_child_blocks_unrecognized() {
        let templates = HashMap::from([(
            "templates/base.md".to_string(),
            "{% block title %}t{% endblock %}".to_string(),
        )]);
        let child = concat!(
            "{% extends \"templates/base.md\" %}",
            "{% block typo %}override{% endblock %}"
        );
        let err = validate_child_blocks(child, &resolver(&templates), Path::new("skill.md"))
            .expect_err("expected error for unrecognized block");
        let msg = err.to_string();
        assert!(msg.contains("typo"), "error: {msg}");
        assert!(msg.contains("skill.md"), "error: {msg}");
    }

    #[test]
    fn test_validate_child_blocks_multi_level() {
        let templates = HashMap::from([
            (
                "templates/grandparent.md".to_string(),
                "{% block deep %}gp{% endblock %}".to_string(),
            ),
            (
                "templates/parent.md".to_string(),
                concat!(
                    "{% extends \"templates/grandparent.md\" %}",
                    "{% block deep %}parent{% endblock %}"
                )
                .to_string(),
            ),
        ]);
        let child = concat!(
            "{% extends \"templates/parent.md\" %}",
            "{% block deep %}child{% endblock %}"
        );
        validate_child_blocks(child, &resolver(&templates), Path::new("spec.md"))
            .expect("expected Ok — block defined in grandparent");
    }

    #[test]
    fn test_validate_child_blocks_multi_level_unrecognized() {
        let templates = HashMap::from([
            (
                "templates/grandparent.md".to_string(),
                "{% block real %}gp{% endblock %}".to_string(),
            ),
            (
                "templates/parent.md".to_string(),
                "{% extends \"templates/grandparent.md\" %}".to_string(),
            ),
        ]);
        let child = concat!(
            "{% extends \"templates/parent.md\" %}",
            "{% block fake %}child{% endblock %}"
        );
        let err = validate_child_blocks(child, &resolver(&templates), Path::new("spec.md"))
            .expect_err("expected error for unrecognized block");
        let msg = err.to_string();
        assert!(msg.contains("fake"), "error: {msg}");
    }

    #[test]
    fn test_validate_child_blocks_non_template_spec() {
        let resolver = |_name: &str| -> Result<Option<String>> { Ok(None) };
        let child = "Plain body with no extends.";
        validate_child_blocks(child, &resolver, Path::new("spec.md"))
            .expect("expected Ok for non-template spec");
    }

    #[test]
    fn test_validate_child_blocks_circular_extends() {
        let templates = HashMap::from([
            (
                "templates/a.md".to_string(),
                "{% extends \"templates/b.md\" %}{% block x %}a{% endblock %}".to_string(),
            ),
            (
                "templates/b.md".to_string(),
                "{% extends \"templates/a.md\" %}{% block x %}b{% endblock %}".to_string(),
            ),
        ]);
        let child = concat!(
            "{% extends \"templates/a.md\" %}",
            "{% block x %}child{% endblock %}"
        );
        let err = validate_child_blocks(child, &resolver(&templates), Path::new("spec.md"))
            .expect_err("expected error for circular chain");
        let msg = err.to_string();
        assert!(msg.contains("circular"), "error: {msg}");
    }
}
