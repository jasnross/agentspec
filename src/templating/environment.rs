use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use minijinja::Environment;

use crate::provider::Provider;
use crate::spec::{Spec, ToolFrontmatter};

/// Build a `MiniJinja` environment for `spec` with all fragments available as
/// templates and the `tool()` function registered for canonical-to-provider
/// tool name resolution.
///
/// Enables `{% include "review/prompt-contract.md" %}` syntax and
/// `{{ tool("<canonical>") }}` calls in all specs. When `provider` is `Some`,
/// `tool()` returns the provider-specific body-level name. When `None`
/// (e.g., during `agentspec validate`), `tool()` passes the canonical name
/// through unchanged after verifying it is a known tool.
///
/// `script_path()` is additionally registered when `spec` is `Spec::Skill(_)`.
/// Calling it from an agent, rule, or hook body produces an `UnknownFunction`
/// render error.
pub fn build_environment(
    fragments: &HashMap<String, String>,
    provider: Option<Provider>,
    spec: &Spec,
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

    if matches!(spec, Spec::Skill(_)) {
        env.add_function("script_path", move |path: String| {
            resolve_script_path(&path, provider)
        });
    }

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
    use std::path::PathBuf;

    use super::*;
    use crate::provider::Provider;
    use crate::spec::{
        AgentFrontmatter, AgentSpec, HookEvent, HookFrontmatter, HookSpec, RuleFrontmatter,
        RuleSpec, SkillFrontmatter, SkillSpec,
    };

    fn dummy_agent_spec() -> Spec {
        Spec::Agent(AgentSpec {
            path: PathBuf::from("/tmp/agent.md"),
            frontmatter: AgentFrontmatter {
                id: "dummy-agent".to_string(),
                description: "dummy".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: String::new(),
        })
    }

    fn dummy_skill_spec() -> Spec {
        Spec::Skill(SkillSpec {
            path: PathBuf::from("/tmp/skill.md"),
            frontmatter: SkillFrontmatter {
                id: "dummy-skill".to_string(),
                description: None,
                tags: None,
                user_invocable: false,
                agent_invocable: false,
                execution: None,
                capabilities: None,
            },
            body: String::new(),
            supporting_files: Vec::new(),
        })
    }

    fn dummy_rule_spec() -> Spec {
        Spec::Rule(RuleSpec {
            path: PathBuf::from("/tmp/rule.md"),
            frontmatter: RuleFrontmatter {
                id: "dummy-rule".to_string(),
                description: None,
                tags: None,
            },
            body: String::new(),
        })
    }

    fn dummy_hook_spec() -> Spec {
        Spec::Hook(HookSpec {
            path: PathBuf::from("/tmp/hooks.toml"),
            frontmatter: HookFrontmatter {
                id: "dummy-hook".to_string(),
                event: HookEvent::SessionStart,
                script: PathBuf::from("scripts/init.sh"),
                matcher: None,
                timeout: None,
                description: None,
                tags: None,
            },
            body: String::new(),
            supporting_files: Vec::new(),
        })
    }

    fn render_body(body: &str, provider: Option<Provider>, spec: &Spec) -> String {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, provider, spec).expect("expected value");
        let template = env.template_from_str(body).expect("expected value");
        template
            .render(minijinja::context! {})
            .expect("expected value")
    }

    #[test]
    fn test_simple_include() {
        let mut fragments = HashMap::new();
        fragments.insert("greeting.md".to_string(), "Hello, world!".to_string());

        let env = build_environment(&fragments, None, &dummy_skill_spec()).expect("expected value");
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

        let env = build_environment(&fragments, None, &dummy_skill_spec()).expect("expected value");
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

        let env = build_environment(&fragments, None, &dummy_skill_spec()).expect("expected value");
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
        let env = build_environment(&fragments, None, &dummy_skill_spec()).expect("expected value");
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

        let env = build_environment(&fragments, None, &dummy_skill_spec()).expect("expected value");
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

        let env = build_environment(&fragments, None, &dummy_skill_spec()).expect("expected value");
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
        let out = render_body(
            r#"{{ tool("question") }}"#,
            Some(Provider::Claude),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "AskUserQuestion");
        let out = render_body(
            r#"{{ tool("subagent") }}"#,
            Some(Provider::Claude),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "Agent");
        let out = render_body(
            r#"{{ tool("skill") }}"#,
            Some(Provider::Claude),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "Skill");
    }

    #[test]
    fn test_tool_resolves_for_cursor() {
        let out = render_body(
            r#"{{ tool("question") }}"#,
            Some(Provider::Cursor),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "Ask questions");
        let out = render_body(
            r#"{{ tool("subagent") }}"#,
            Some(Provider::Cursor),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "Task");
        let out = render_body(
            r#"{{ tool("skill") }}"#,
            Some(Provider::Cursor),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "Skill runner");
    }

    #[test]
    fn test_tool_resolves_for_opencode() {
        let out = render_body(
            r#"{{ tool("question") }}"#,
            Some(Provider::OpenCode),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "question");
        let out = render_body(
            r#"{{ tool("subagent") }}"#,
            Some(Provider::OpenCode),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "task");
        let out = render_body(
            r#"{{ tool("skill") }}"#,
            Some(Provider::OpenCode),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "skill");
    }

    #[test]
    fn test_tool_passes_through_canonical_when_provider_is_none() {
        let out = render_body(r#"{{ tool("question") }}"#, None, &dummy_skill_spec());
        assert_eq!(out, "question");
    }

    #[test]
    fn test_tool_resolves_inside_included_fragment() {
        let mut fragments = HashMap::new();
        fragments.insert(
            "tool-ref.md".to_owned(),
            r#"Use {{ tool("question") }}."#.to_owned(),
        );
        let env = build_environment(&fragments, Some(Provider::Claude), &dummy_skill_spec())
            .expect("expected value");
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
        let env = build_environment(&fragments, Some(Provider::Claude), &dummy_skill_spec())
            .expect("expected value");
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

    #[test]
    fn test_script_path_registered_for_skill_body() {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, Some(Provider::Claude), &dummy_skill_spec())
            .expect("expected value");
        let template = env
            .template_from_str(r#"{{ script_path("scripts/foo.sh") }}"#)
            .expect("expected value");
        let out = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(out, "${CLAUDE_SKILL_DIR}/scripts/foo.sh");
    }

    #[test]
    fn test_script_path_passes_through_for_cursor_skill() {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, Some(Provider::Cursor), &dummy_skill_spec())
            .expect("expected value");
        let template = env
            .template_from_str(r#"{{ script_path("scripts/foo.sh") }}"#)
            .expect("expected value");
        let out = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(out, "scripts/foo.sh");
    }

    #[test]
    fn test_script_path_not_registered_for_agent_body() {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, Some(Provider::Claude), &dummy_agent_spec())
            .expect("expected value");
        let template = env
            .template_from_str(r#"{{ script_path("scripts/foo.sh") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for agent spec");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unknown"),
            "expected 'unknown' in error, got: {msg}"
        );
        assert!(
            msg.contains("script_path"),
            "expected 'script_path' in error, got: {msg}"
        );
    }

    #[test]
    fn test_script_path_not_registered_for_rule_body() {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, Some(Provider::Claude), &dummy_rule_spec())
            .expect("expected value");
        let template = env
            .template_from_str(r#"{{ script_path("scripts/foo.sh") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for rule spec");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unknown"),
            "expected 'unknown' in error, got: {msg}"
        );
        assert!(
            msg.contains("script_path"),
            "expected 'script_path' in error, got: {msg}"
        );
    }

    #[test]
    fn test_script_path_not_registered_for_hook_body() {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, Some(Provider::Claude), &dummy_hook_spec())
            .expect("expected value");
        let template = env
            .template_from_str(r#"{{ script_path("scripts/foo.sh") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for hook spec");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unknown"),
            "expected 'unknown' in error, got: {msg}"
        );
        assert!(
            msg.contains("script_path"),
            "expected 'script_path' in error, got: {msg}"
        );
    }

    #[test]
    fn test_script_path_validate_mode_skill_renders() {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, None, &dummy_skill_spec()).expect("expected value");
        let template = env
            .template_from_str(r#"{{ script_path("scripts/foo.sh") }}"#)
            .expect("expected value");
        let out = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(out, "scripts/foo.sh");
    }

    #[test]
    fn test_script_path_validate_mode_agent_errors() {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, None, &dummy_agent_spec()).expect("expected value");
        let template = env
            .template_from_str(r#"{{ script_path("scripts/foo.sh") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for agent spec in validate mode");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unknown"),
            "expected 'unknown' in error, got: {msg}"
        );
        assert!(
            msg.contains("script_path"),
            "expected 'script_path' in error, got: {msg}"
        );
    }

    #[test]
    fn test_tool_remains_registered_for_all_spec_types() {
        let fragments = HashMap::new();
        let agent = dummy_agent_spec();
        let skill = dummy_skill_spec();
        let rule = dummy_rule_spec();
        let hook = dummy_hook_spec();
        for spec in [&agent, &skill, &rule, &hook] {
            let env = build_environment(&fragments, Some(Provider::Claude), spec)
                .expect("expected value");
            let template = env
                .template_from_str(r#"{{ tool("question") }}"#)
                .expect("expected value");
            let out = template
                .render(minijinja::context! {})
                .expect("expected value");
            assert_eq!(
                out, "AskUserQuestion",
                "tool() should resolve for all spec types"
            );
        }
    }
}
