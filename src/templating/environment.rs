use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use minijinja::Environment;

use crate::provider::Provider;
use crate::spec::ToolFrontmatter;

/// Build a `MiniJinja` environment with all fragments available as templates
/// and a `tool()` function registered for canonical-to-provider tool name
/// resolution.
///
/// Enables `{% include "review/prompt-contract.md" %}` syntax and
/// `{{ tool("<canonical>") }}` calls in specs. When `provider` is `Some`,
/// `tool()` returns the provider-specific body-level name. When `None`
/// (e.g., during `agentspec validate`), `tool()` passes the canonical name
/// through unchanged after verifying it is a known tool.
pub fn build_environment(
    fragments: &HashMap<String, String>,
    provider: Option<Provider>,
) -> Result<Environment<'static>> {
    let mut env = Environment::new();
    // Lenient: undefined variables evaluate as falsy rather than erroring,
    // which is useful for optional boolean flags in templates.
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Lenient);

    for (name, content) in fragments {
        env.add_template_owned(name.clone(), content.clone())
            .with_context(|| format!("failed to parse fragment '{name}'"))?;
    }

    env.add_function("tool", move |name: String| resolve_tool(&name, provider));

    env.add_function("script_path", move |path: String| {
        resolve_script_path(&path, provider)
    });

    Ok(env)
}

fn resolve_script_path(
    path: &String,
    provider: Option<Provider>,
) -> Result<String, minijinja::Error> {
    let Some(p) = provider else {
        return Ok(path.to_owned());
    };

    let end = Path::new(path);

    if end.has_root() {
        return Err(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("script path must be relative. got: {}", end.display()),
        ));
    }

    Ok(match p.adapter().body_skill_root() {
        Some(root) => format!("{}/{}", root, end.display()),
        None => end.display().to_string(),
    })
}

/// Resolve a canonical tool name to the provider-specific body-level name.
///
/// Returns a `MiniJinja` render error if `name` is not a known canonical
/// tool. When `provider` is `None`, the canonical name is returned unchanged
/// (after the round-trip through `ToolFrontmatter` confirms it is valid).
fn resolve_tool(name: &str, provider: Option<Provider>) -> Result<String, minijinja::Error> {
    let tool: ToolFrontmatter = name.parse().map_err(|_| {
        minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("unknown canonical tool name '{name}'"),
        )
    })?;
    let Some(p) = provider else {
        return Ok(name.to_owned());
    };
    Ok(p.adapter().body_tool_name(&tool).to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::provider::Provider;

    fn render_body(body: &str, provider: Option<Provider>) -> String {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, provider).expect("expected value");
        let template = env.template_from_str(body).expect("expected value");
        template
            .render(minijinja::context! {})
            .expect("expected value")
    }

    #[test]
    fn test_simple_include() {
        let mut fragments = HashMap::new();
        fragments.insert("greeting.md".to_string(), "Hello, world!".to_string());

        let env = build_environment(&fragments, None).expect("expected value");
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

        let env = build_environment(&fragments, None).expect("expected value");
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

        let env = build_environment(&fragments, None).expect("expected value");
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
        let env = build_environment(&fragments, None).expect("expected value");
        let template = env
            .template_from_str("{% include \"nonexistent.md\" %}")
            .expect("expected value");
        let result = template.render(minijinja::context! {});
        assert!(result.is_err());
    }

    #[test]
    fn test_filter_indent_with_include() {
        let mut fragments = HashMap::new();
        fragments.insert("rules.md".to_string(), "Rule 1\nRule 2\nRule 3".to_string());

        let env = build_environment(&fragments, None).expect("expected value");
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

        let env = build_environment(&fragments, None).expect("expected value");
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

    #[test]
    fn test_tool_resolves_for_claude() {
        let out = render_body(r#"{{ tool("question") }}"#, Some(Provider::Claude));
        assert_eq!(out, "AskUserQuestion");
        let out = render_body(r#"{{ tool("subagent") }}"#, Some(Provider::Claude));
        assert_eq!(out, "Agent");
        let out = render_body(r#"{{ tool("skill") }}"#, Some(Provider::Claude));
        assert_eq!(out, "Skill");
    }

    #[test]
    fn test_tool_resolves_for_cursor() {
        let out = render_body(r#"{{ tool("question") }}"#, Some(Provider::Cursor));
        assert_eq!(out, "Ask questions");
        let out = render_body(r#"{{ tool("subagent") }}"#, Some(Provider::Cursor));
        assert_eq!(out, "Task");
        let out = render_body(r#"{{ tool("skill") }}"#, Some(Provider::Cursor));
        assert_eq!(out, "Skill runner");
    }

    #[test]
    fn test_tool_resolves_for_opencode() {
        let out = render_body(r#"{{ tool("question") }}"#, Some(Provider::OpenCode));
        assert_eq!(out, "question");
        let out = render_body(r#"{{ tool("subagent") }}"#, Some(Provider::OpenCode));
        assert_eq!(out, "task");
        let out = render_body(r#"{{ tool("skill") }}"#, Some(Provider::OpenCode));
        assert_eq!(out, "skill");
    }

    #[test]
    fn test_tool_passes_through_canonical_when_provider_is_none() {
        let out = render_body(r#"{{ tool("question") }}"#, None);
        assert_eq!(out, "question");
    }

    #[test]
    fn test_tool_resolves_inside_included_fragment() {
        let mut fragments = HashMap::new();
        fragments.insert(
            "tool-ref.md".to_owned(),
            r#"Use {{ tool("question") }}."#.to_owned(),
        );
        let env = build_environment(&fragments, Some(Provider::Claude)).expect("expected value");
        let template = env
            .template_from_str(r#"{% include "tool-ref.md" %}"#)
            .expect("expected value");
        let out = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(out, "Use AskUserQuestion.");
    }

    #[test]
    fn test_tool_unknown_name_errors() {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, Some(Provider::Claude)).expect("expected value");
        let template = env
            .template_from_str(r#"{{ tool("nope") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for unknown tool");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("nope"),
            "error message should contain offending name 'nope', got: {msg}"
        );
    }
}
