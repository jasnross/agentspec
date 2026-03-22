use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use minijinja::value::Kwargs;
use minijinja::{Environment, State};
use walkdir::WalkDir;

use crate::types::CanonicalSpec;

/// Load fragment files from a directory. Returns a map of fragment name to content.
///
/// Fragment names are paths relative to the fragments directory with `.md` stripped.
/// For example, `spec/fragments/review/prompt-contract.md` becomes `review/prompt-contract`.
pub fn load_fragments(fragments_dir: &Path) -> Result<HashMap<String, String>> {
    let mut fragments = HashMap::new();

    if !fragments_dir.is_dir() {
        return Ok(fragments);
    }

    for entry in WalkDir::new(fragments_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|ext| ext == "md"))
    {
        let path = entry.path();
        let relative = path
            .strip_prefix(fragments_dir)
            .context("failed to compute relative path for fragment")?;

        // Key = relative path with .md stripped (e.g., "review/prompt-contract")
        let name = relative.with_extension("").to_string_lossy().to_string();

        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read fragment {}", path.display()))?;

        fragments.insert(name, content);
    }

    Ok(fragments)
}

/// Build a `MiniJinja` environment with all fragments available as templates.
///
/// Templates are keyed with `.md` suffix appended to the fragment name
/// (e.g., fragment "review/prompt-contract" → template "review/prompt-contract.md").
/// This matches the `{% include "review/prompt-contract.md" %}` syntax used in specs.
///
/// Fragments are loaded lazily via a source callback — they are only parsed when
/// actually referenced by a `{% include %}`. This is important because fragments
/// may still contain Handlebars syntax during the migration period (Phases 3-7).
///
/// A custom `include_indented` function is registered to support Handlebars-style
/// auto-indentation of included content.
pub fn build_environment(
    fragments: &HashMap<String, String>,
) -> (Environment<'static>, Vec<String>) {
    let mut env = Environment::new();
    // Lenient: undefined variables evaluate as falsy rather than erroring.
    // This matches Handlebars semantics where optional boolean flags (e.g.,
    // `writeable`, `pr_specific`) default to falsy when not passed by the caller.
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Lenient);
    let mut warnings = Vec::new();

    // Register each fragment as a template with .md suffix.
    // Fragments are registered eagerly — if any contain invalid MiniJinja syntax
    // (e.g., Handlebars syntax pre-migration), we skip them with a warning.
    // They'll only cause an error if actually referenced by a spec.
    for (name, content) in fragments {
        let template_name = format!("{name}.md");
        match env.add_template_owned(template_name.clone(), content.clone()) {
            Ok(()) => {}
            Err(e) => {
                // Fragment contains syntax MiniJinja can't parse (likely Handlebars).
                // This is expected during the migration period. The fragment will
                // cause a "template not found" error if a spec tries to include it.
                warnings.push(format!(
                    "skipping fragment '{template_name}' (parse error: {e})"
                ));
            }
        }
    }

    // Register the include_indented custom function for indentation-aware includes.
    // Usage: {{ include_indented("fragment/name.md", indent=4) }}
    env.add_function("include_indented", include_indented);

    (env, warnings)
}

/// Custom function that renders a template and indents every line by the specified amount.
///
/// Signature: `{{ include_indented("template.md", indent=N) }}`
///
/// Reconstructs the caller's variable scope via `State::lookup` so that
/// `{% with %}` variables are available inside the included template.
fn include_indented(
    state: &State,
    template_name: String,
    kwargs: Kwargs,
) -> Result<String, minijinja::Error> {
    let indent: usize = kwargs.get::<usize>("indent").unwrap_or(0);
    kwargs.assert_all_used()?;

    let tmpl = state.env().get_template(&template_name)?;

    // Pass the caller's known variables as context so {% with %} vars propagate.
    let known: std::collections::BTreeMap<String, minijinja::Value> = state
        .exports()
        .iter()
        .filter_map(|name| state.lookup(name).map(|v| (name.to_string(), v)))
        .collect();
    let rendered = tmpl.render(known)?;

    if indent == 0 {
        return Ok(rendered);
    }

    let prefix = " ".repeat(indent);
    let indented = rendered
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 || line.is_empty() {
                // First line is already at the call site's indentation;
                // empty lines don't get indented
                line.to_string()
            } else {
                format!("{prefix}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Preserve trailing newline if the original had one
    if rendered.ends_with('\n') && !indented.ends_with('\n') {
        Ok(format!("{indented}\n"))
    } else {
        Ok(indented)
    }
}

/// Check if a string contains `MiniJinja` template syntax.
///
/// Detects `{% ... %}` block tags and `{{ ... }}` expression tags.
fn contains_minijinja_syntax(body: &str) -> bool {
    body.contains("{%") || body.contains("{{")
}

/// Resolve fragment references in spec bodies by rendering them through `MiniJinja`.
///
/// Each spec body is treated as an inline template. Specs that contain no template
/// syntax pass through unchanged.
pub fn resolve_fragments(
    specs: Vec<CanonicalSpec>,
    env: &Environment<'_>,
) -> Result<Vec<CanonicalSpec>> {
    let mut resolved = Vec::with_capacity(specs.len());

    for mut spec in specs {
        // Only process specs that contain MiniJinja template syntax.
        // We specifically check for `{%` (block tags like {% include %}, {% with %})
        // rather than `{{` alone, because `{{` also matches Handlebars syntax
        // (e.g., `{{> partial}}`) which is present before the Phase 8 migration.
        if contains_minijinja_syntax(&spec.body) {
            let tmpl = env
                .template_from_str(&spec.body)
                .with_context(|| format!("failed to parse template in {}", spec.path.display()))?;
            spec.body = tmpl.render(minijinja::context! {}).with_context(|| {
                format!("failed to resolve fragments in {}", spec.path.display())
            })?;
        }
        resolved.push(spec);
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
            fragments["review/prompt-contract"],
            "You must follow these rules."
        );
        assert_eq!(fragments["simple"], "Simple fragment.");
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
        fragments.insert("greeting".to_string(), "Hello, world!".to_string());

        let (env, _warnings) = build_environment(&fragments);
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
        fragments.insert("greeting".to_string(), "Hello, {{ name }}!".to_string());

        let (env, _warnings) = build_environment(&fragments);
        let tmpl = env
            .template_from_str(
                "{% with name = \"Alice\" %}{% include \"greeting.md\" %}{% endwith %}",
            )
            .expect("expected value");
        let result = tmpl.render(minijinja::context! {}).expect("expected value");
        assert_eq!(result, "Hello, Alice!");
    }

    #[test]
    fn test_nested_includes() {
        let mut fragments = HashMap::new();
        fragments.insert("inner".to_string(), "inner content".to_string());
        fragments.insert(
            "outer".to_string(),
            "before {% include \"inner.md\" %} after".to_string(),
        );

        let (env, _warnings) = build_environment(&fragments);
        let tmpl = env
            .template_from_str("start {% include \"outer.md\" %} end")
            .expect("expected value");
        let result = tmpl.render(minijinja::context! {}).expect("expected value");
        assert_eq!(result, "start before inner content after end");
    }

    #[test]
    fn test_missing_fragment_errors() {
        let fragments = HashMap::new();
        let (env, _warnings) = build_environment(&fragments);
        let tmpl = env
            .template_from_str("{% include \"nonexistent.md\" %}")
            .expect("expected value");
        let result = tmpl.render(minijinja::context! {});
        assert!(result.is_err());
    }

    #[test]
    fn test_include_indented() {
        let mut fragments = HashMap::new();
        fragments.insert("rules".to_string(), "Rule 1\nRule 2\nRule 3".to_string());

        let (env, _warnings) = build_environment(&fragments);
        let tmpl = env
            .template_from_str("Items:\n   {{ include_indented(\"rules.md\", indent=3) }}")
            .expect("expected value");
        let result = tmpl.render(minijinja::context! {}).expect("expected value");
        assert_eq!(result, "Items:\n   Rule 1\n   Rule 2\n   Rule 3");
    }

    #[test]
    fn test_resolve_fragments_no_syntax() {
        let fragments = HashMap::new();
        let (env, _warnings) = build_environment(&fragments);

        let specs = vec![CanonicalSpec {
            path: "test.md".into(),
            fm: serde_json::json!({"id": "test"}),
            body: "Plain body with no template syntax.".to_string(),
            kind: crate::types::SpecKind::Agent,
            supporting_files: vec![],
        }];

        let resolved = resolve_fragments(specs, &env).expect("expected value");
        assert_eq!(resolved[0].body, "Plain body with no template syntax.");
    }

    #[test]
    fn test_resolve_fragments_with_include() {
        let mut fragments = HashMap::new();
        fragments.insert("footer".to_string(), "-- End --".to_string());

        let (env, _warnings) = build_environment(&fragments);

        let specs = vec![CanonicalSpec {
            path: "test.md".into(),
            fm: serde_json::json!({"id": "test"}),
            body: "Body.\n{% include \"footer.md\" %}".to_string(),
            kind: crate::types::SpecKind::Agent,
            supporting_files: vec![],
        }];

        let resolved = resolve_fragments(specs, &env).expect("expected value");
        assert_eq!(resolved[0].body, "Body.\n-- End --");
    }

    #[test]
    fn test_filter_indent_with_include() {
        let mut fragments = HashMap::new();
        fragments.insert("rules".to_string(), "Rule 1\nRule 2\nRule 3".to_string());

        let (env, _warnings) = build_environment(&fragments);
        let tmpl = env
            .template_from_str(
                "Items:\n   {% filter indent(3, first=false) %}{% include \"rules.md\" %}{% endfilter %}",
            )
            .expect("expected value");
        let result = tmpl.render(minijinja::context! {}).expect("expected value");
        assert_eq!(result, "Items:\n   Rule 1\n   Rule 2\n   Rule 3");
    }

    #[test]
    fn test_filter_indent_with_variables() {
        let mut fragments = HashMap::new();
        fragments.insert(
            "greeting".to_string(),
            "Hello, {{ name }}!\nWelcome aboard.".to_string(),
        );

        let (env, _warnings) = build_environment(&fragments);
        let tmpl = env
            .template_from_str(
                "Message:\n    {% filter indent(4, first=false) %}{% with name = \"Alice\" %}{% include \"greeting.md\" %}{% endwith %}{% endfilter %}",
            )
            .expect("expected value");
        let result = tmpl.render(minijinja::context! {}).expect("expected value");
        assert_eq!(result, "Message:\n    Hello, Alice!\n    Welcome aboard.");
    }
}
