use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use minijinja::Environment;
use walkdir::WalkDir;

use crate::spec::NormalizedSpec;

/// Resolve fragment references in spec bodies by rendering them through `MiniJinja`.
///
/// Each spec body is treated as an inline template. Specs that contain no template
/// syntax pass through unchanged. Operates on normalized specs so that template
/// resolution is decoupled from the spec loading/validation lifecycle.
pub fn resolve_fragments(
    specs: Vec<NormalizedSpec>,
    env: &Environment<'_>,
) -> Result<Vec<NormalizedSpec>> {
    let mut resolved = Vec::with_capacity(specs.len());

    for mut spec in specs {
        let template = env
            .template_from_str(spec.body())
            .with_context(|| format!("failed to parse template in {}", spec.path().display()))?;

        let body = template
            .render(minijinja::context! {})
            .with_context(|| format!("failed to resolve fragments in {}", spec.path().display()))?;

        match &mut spec {
            NormalizedSpec::Agent(s) => s.body = body,
            NormalizedSpec::Skill(s) => s.body = body,
            NormalizedSpec::Rule(s) => s.body = body,
        }

        resolved.push(spec);
    }

    Ok(resolved)
}

/// Load fragment files from a directory. Returns a map of fragment name to content.
///
/// Fragment names are relative paths including the `.md` extension, matching the
/// `{% include "review/prompt-contract.md" %}` syntax used in spec bodies.
pub fn load_fragments(fragments_dir: &Path) -> Result<HashMap<String, String>> {
    let mut fragments = HashMap::new();

    if !fragments_dir.is_dir() {
        return Ok(fragments);
    }

    let entries = WalkDir::new(fragments_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|ext| ext == "md"));

    for entry in entries {
        let path = entry.path();
        let relative = path
            .strip_prefix(fragments_dir)
            .context("failed to compute relative path for fragment")?;

        let name = relative.to_string_lossy().to_string();

        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read fragment {}", path.display()))?;

        fragments.insert(name, content);
    }

    Ok(fragments)
}

/// Build a `MiniJinja` environment with all fragments available as templates.
///
/// Enables `{% include "review/prompt-contract.md" %}` syntax in specs.
pub fn build_environment(fragments: &HashMap<String, String>) -> Result<Environment<'static>> {
    let mut env = Environment::new();
    // Lenient: undefined variables evaluate as falsy rather than erroring,
    // which is useful for optional boolean flags in templates.
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Lenient);

    for (name, content) in fragments {
        env.add_template_owned(name.clone(), content.clone())
            .with_context(|| format!("failed to parse fragment '{name}'"))?;
    }

    Ok(env)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::spec::{NormalizedAgentFrontmatter, NormalizedAgentSpec};

    #[test]
    fn test_load_fragments() {
        let tmp = tempfile::tempdir().expect("expected value");
        let frag_dir = tmp.path().join("fragments");
        fs::create_dir_all(frag_dir.join("review")).expect("expected value");
        fs::write(
            frag_dir.join("review/prompt-contract.md"),
            "You must follow these rules.",
        )
        .expect("expected value");
        fs::write(frag_dir.join("simple.md"), "Simple fragment.").expect("expected value");

        let fragments = load_fragments(&frag_dir).expect("expected value");
        assert_eq!(fragments.len(), 2);
        assert_eq!(
            fragments["review/prompt-contract.md"],
            "You must follow these rules."
        );
        assert_eq!(fragments["simple.md"], "Simple fragment.");
    }

    #[test]
    fn test_load_fragments_nonexistent_dir() {
        let tmp = tempfile::tempdir().expect("expected value");
        let fragments = load_fragments(&tmp.path().join("nonexistent")).expect("expected value");
        assert!(fragments.is_empty());
    }

    #[test]
    fn test_simple_include() {
        let mut fragments = HashMap::new();
        fragments.insert("greeting.md".to_string(), "Hello, world!".to_string());

        let env = build_environment(&fragments).expect("expected value");
        let template = env
            .template_from_str("Before.\n{% include \"greeting.md\" %}\nAfter.")
            .expect("expected value");
        let result = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(result, "Before.\nHello, world!\nAfter.");
    }

    #[test]
    fn test_include_with_variables() {
        let mut fragments = HashMap::new();
        fragments.insert("greeting.md".to_string(), "Hello, {{ name }}!".to_string());

        let env = build_environment(&fragments).expect("expected value");
        let template = env
            .template_from_str(
                "{% with name = \"Alice\" %}{% include \"greeting.md\" %}{% endwith %}",
            )
            .expect("expected value");
        let result = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(result, "Hello, Alice!");
    }

    #[test]
    fn test_nested_includes() {
        let mut fragments = HashMap::new();
        fragments.insert("inner.md".to_string(), "inner content".to_string());
        fragments.insert(
            "outer.md".to_string(),
            "before {% include \"inner.md\" %} after".to_string(),
        );

        let env = build_environment(&fragments).expect("expected value");
        let template = env
            .template_from_str("start {% include \"outer.md\" %} end")
            .expect("expected value");
        let result = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(result, "start before inner content after end");
    }

    #[test]
    fn test_missing_fragment_errors() {
        let fragments = HashMap::new();
        let env = build_environment(&fragments).expect("expected value");
        let template = env
            .template_from_str("{% include \"nonexistent.md\" %}")
            .expect("expected value");
        let result = template.render(minijinja::context! {});
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_fragments_no_syntax() {
        let fragments = HashMap::new();
        let env = build_environment(&fragments).expect("expected value");

        let specs = vec![NormalizedSpec::Agent(NormalizedAgentSpec {
            path: "test.md".into(),
            frontmatter: NormalizedAgentFrontmatter {
                id: "test".to_string(),
                description: "test".to_string(),
                execution: None,
                capabilities: None,
            },
            body: "Plain body with no template syntax.".to_string(),
        })];

        let resolved = resolve_fragments(specs, &env).expect("expected value");
        let NormalizedSpec::Agent(ref s) = resolved[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(s.body, "Plain body with no template syntax.");
    }

    #[test]
    fn test_resolve_fragments_with_include() {
        let mut fragments = HashMap::new();
        fragments.insert("footer.md".to_string(), "-- End --".to_string());

        let env = build_environment(&fragments).expect("expected value");

        let specs = vec![NormalizedSpec::Agent(NormalizedAgentSpec {
            path: "test.md".into(),
            frontmatter: NormalizedAgentFrontmatter {
                id: "test".to_string(),
                description: "test".to_string(),
                execution: None,
                capabilities: None,
            },
            body: "Body.\n{% include \"footer.md\" %}".to_string(),
        })];

        let resolved = resolve_fragments(specs, &env).expect("expected value");
        let NormalizedSpec::Agent(ref s) = resolved[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(s.body, "Body.\n-- End --");
    }

    #[test]
    fn test_filter_indent_with_include() {
        let mut fragments = HashMap::new();
        fragments.insert("rules.md".to_string(), "Rule 1\nRule 2\nRule 3".to_string());

        let env = build_environment(&fragments).expect("expected value");
        let template = env
            .template_from_str(
                "Items:\n   {% filter indent(3, first=false) %}{% include \"rules.md\" %}{% endfilter %}",
            )
            .expect("expected value");
        let result = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(result, "Items:\n   Rule 1\n   Rule 2\n   Rule 3");
    }

    #[test]
    fn test_filter_indent_with_variables() {
        let mut fragments = HashMap::new();
        fragments.insert(
            "greeting.md".to_string(),
            "Hello, {{ name }}!\nWelcome aboard.".to_string(),
        );

        let env = build_environment(&fragments).expect("expected value");
        let template = env
            .template_from_str(
                "Message:\n    {% filter indent(4, first=false) %}{% with name = \"Alice\" %}{% include \"greeting.md\" %}{% endwith %}{% endfilter %}",
            )
            .expect("expected value");
        let result = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(result, "Message:\n    Hello, Alice!\n    Welcome aboard.");
    }
}
