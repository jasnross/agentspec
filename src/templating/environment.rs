use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

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
/// `script()` is additionally registered when `spec` is `Spec::Skill(_)`.
/// Calling it from an agent, rule, or hook body produces an `UnknownFunction`
/// render error. (`Lenient` undefined behavior applies to variable lookups only;
/// unknown function calls raise `UnknownFunction` regardless.)
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

    if let Spec::Skill(s) = spec {
        let known_scripts: HashSet<PathBuf> = s.supporting_files.keys().cloned().collect();
        env.add_function("script", move |name: String| {
            resolve_script(&name, provider, &known_scripts)
        });
    }
    Ok(env)
}

fn resolve_script(
    name: &str,
    provider: Option<Provider>,
    known_scripts: &HashSet<PathBuf>,
) -> Result<String, minijinja::Error> {
    let relative = Path::new(name);

    if relative.has_root()
        || relative
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("script() path must be relative without '..', got: \"{name}\""),
        ));
    }

    let full_path = PathBuf::from("scripts").join(relative);

    if !known_scripts.contains(&full_path) {
        return Err(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!(
                "script(\"{name}\") references a file not found in this skill's scripts/ directory"
            ),
        ));
    }

    // Use explicit '/' so skill content always contains POSIX paths regardless of host OS.
    let posix_path = format!("scripts/{name}");
    Ok(match provider.and_then(|p| p.adapter().body_skill_root()) {
        Some(root) => format!("{root}/{posix_path}"),
        None => posix_path,
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

    use indexmap::IndexMap;

    use super::*;
    use crate::provider::Provider;
    use crate::spec::{
        AgentFrontmatter, AgentSpec, HookEvent, HookFrontmatter, HookSpec, RuleFrontmatter,
        RuleSpec, SkillFrontmatter, SkillSpec, SupportingFile,
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
        dummy_skill_spec_with_files(&["foo.sh"])
    }

    fn dummy_skill_spec_with_files(names: &[&str]) -> Spec {
        let mut supporting_files = IndexMap::new();
        for name in names {
            supporting_files.insert(
                PathBuf::from(format!("scripts/{name}")),
                SupportingFile {
                    content: vec![],
                    mode: 0o755,
                },
            );
        }
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
            supporting_files,
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
                events: vec![HookEvent::SessionStart],
                script: PathBuf::from("scripts/init.sh"),
                matcher: None,
                timeout: None,
                description: None,
                tags: None,
            },
            body: String::new(),
            supporting_files: IndexMap::new(),
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
    fn test_script_registered_for_skill_body() {
        let out = render_body(
            r#"{{ script("foo.sh") }}"#,
            Some(Provider::Claude),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "${CLAUDE_SKILL_DIR}/scripts/foo.sh");
    }

    #[test]
    fn test_script_passes_through_for_cursor_skill() {
        let out = render_body(
            r#"{{ script("foo.sh") }}"#,
            Some(Provider::Cursor),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "scripts/foo.sh");
    }

    #[test]
    fn test_script_not_registered_for_agent_body() {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, Some(Provider::Claude), &dummy_agent_spec())
            .expect("expected value");
        let template = env
            .template_from_str(r#"{{ script("foo.sh") }}"#)
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
            msg.contains("script"),
            "expected 'script' in error, got: {msg}"
        );
    }

    #[test]
    fn test_script_not_registered_for_rule_body() {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, Some(Provider::Claude), &dummy_rule_spec())
            .expect("expected value");
        let template = env
            .template_from_str(r#"{{ script("foo.sh") }}"#)
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
            msg.contains("script"),
            "expected 'script' in error, got: {msg}"
        );
    }

    #[test]
    fn test_script_not_registered_for_hook_body() {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, Some(Provider::Claude), &dummy_hook_spec())
            .expect("expected value");
        let template = env
            .template_from_str(r#"{{ script("foo.sh") }}"#)
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
            msg.contains("script"),
            "expected 'script' in error, got: {msg}"
        );
    }

    #[test]
    fn test_script_validate_mode_skill_renders() {
        let out = render_body(r#"{{ script("foo.sh") }}"#, None, &dummy_skill_spec());
        assert_eq!(out, "scripts/foo.sh");
    }

    #[test]
    fn test_script_validate_mode_agent_errors() {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, None, &dummy_agent_spec()).expect("expected value");
        let template = env
            .template_from_str(r#"{{ script("foo.sh") }}"#)
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
            msg.contains("script"),
            "expected 'script' in error, got: {msg}"
        );
    }

    #[test]
    fn test_script_missing_file_errors() {
        let spec = dummy_skill_spec_with_files(&["exists.sh"]);
        let fragments = HashMap::new();
        let env =
            build_environment(&fragments, Some(Provider::Claude), &spec).expect("expected value");
        let template = env
            .template_from_str(r#"{{ script("missing.sh") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for missing script");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("missing.sh"),
            "error should name the missing file, got: {msg}"
        );
        assert!(
            msg.contains("not found"),
            "error should say 'not found', got: {msg}"
        );
    }

    #[test]
    fn test_script_missing_file_errors_in_validate_mode() {
        let spec = dummy_skill_spec_with_files(&["exists.sh"]);
        let fragments = HashMap::new();
        let env = build_environment(&fragments, None, &spec).expect("expected value");
        let template = env
            .template_from_str(r#"{{ script("missing.sh") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for missing script in validate mode");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("missing.sh"),
            "error should name the missing file, got: {msg}"
        );
        assert!(
            msg.contains("not found"),
            "error should say 'not found', got: {msg}"
        );
    }

    #[test]
    fn test_script_allows_nested_path() {
        let spec = dummy_skill_spec_with_files(&["subdir/nested.sh"]);
        let out = render_body(
            r#"{{ script("subdir/nested.sh") }}"#,
            Some(Provider::Claude),
            &spec,
        );
        assert_eq!(out, "${CLAUDE_SKILL_DIR}/scripts/subdir/nested.sh");
        let out = render_body(
            r#"{{ script("subdir/nested.sh") }}"#,
            Some(Provider::Cursor),
            &spec,
        );
        assert_eq!(out, "scripts/subdir/nested.sh");
    }

    #[test]
    fn test_script_rejects_parent_traversal() {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, Some(Provider::Claude), &dummy_skill_spec())
            .expect("expected value");
        let template = env
            .template_from_str(r#"{{ script("../foo.sh") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for parent traversal");
        let msg = format!("{err:#}");
        assert!(msg.contains(".."), "error should mention '..', got: {msg}");
    }

    #[test]
    fn test_script_rejects_absolute_path() {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, Some(Provider::Claude), &dummy_skill_spec())
            .expect("expected value");
        let template = env
            .template_from_str(r#"{{ script("/etc/foo.sh") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for absolute path");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("relative"),
            "error should mention 'relative', got: {msg}"
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
