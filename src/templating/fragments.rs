use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use minijinja::{Environment, Value};
use walkdir::WalkDir;

use super::context::TemplateContext;
use crate::provider::Provider;
use crate::spec::{Spec, ToolFrontmatter};

// TODO: This file has taken on more responsibilities than just fragments. We should reorganize once it becomes clearer where the other concerns belong.

/// Resolve fragment references in spec bodies by rendering them through `MiniJinja`.
///
/// Each spec body is treated as an inline template. Specs that contain no template
/// syntax pass through unchanged. Operates on validated specs so that template
/// resolution is decoupled from the spec loading/validation lifecycle.
pub fn resolve_fragments(
    specs: Vec<Spec>,
    env: &Environment<'_>,
    context: &TemplateContext,
) -> Result<Vec<Spec>> {
    let ctx = Value::from_serialize(context);
    let mut resolved = Vec::with_capacity(specs.len());

    for mut spec in specs {
        let template = env
            .template_from_str(spec.body())
            .with_context(|| format!("failed to parse template in {}", spec.path().display()))?;

        let body = template
            .render(&ctx)
            .with_context(|| format!("failed to render template in {}", spec.path().display()))?;

        match &mut spec {
            Spec::Agent(s) => s.body = body,
            Spec::Skill(s) => s.body = body,
            Spec::Rule(s) => s.body = body,
            // Hooks have empty bodies and skip templating entirely. The render
            // pass above produces the empty string for an empty input template,
            // so no special-case is needed beyond keeping the body unchanged.
            Spec::Hook(_) => {}
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
    use std::fs;

    use super::*;
    use crate::spec::{
        AgentFrontmatter, AgentSpec, RuleFrontmatter, RuleSpec, SkillFrontmatter, SkillSpec,
    };

    fn empty_context() -> TemplateContext {
        TemplateContext::from_specs(&[])
    }

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
    fn test_resolve_fragments_no_syntax() {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, None).expect("expected value");

        let specs = vec![Spec::Agent(AgentSpec {
            path: "test.md".into(),
            frontmatter: AgentFrontmatter {
                id: "test".to_string(),
                description: "test".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: "Plain body with no template syntax.".to_string(),
        })];

        let resolved = resolve_fragments(specs, &env, &empty_context()).expect("expected value");
        let Spec::Agent(ref s) = resolved[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(s.body, "Plain body with no template syntax.");
    }

    #[test]
    fn test_resolve_fragments_with_include() {
        let mut fragments = HashMap::new();
        fragments.insert("footer.md".to_string(), "-- End --".to_string());

        let env = build_environment(&fragments, None).expect("expected value");

        let specs = vec![Spec::Agent(AgentSpec {
            path: "test.md".into(),
            frontmatter: AgentFrontmatter {
                id: "test".to_string(),
                description: "test".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: "Body.\n{% include \"footer.md\" %}".to_string(),
        })];

        let resolved = resolve_fragments(specs, &env, &empty_context()).expect("expected value");
        let Spec::Agent(ref s) = resolved[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(s.body, "Body.\n-- End --");
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

    // --- Template variable tests ---

    fn test_context() -> TemplateContext {
        use crate::spec::Spec;

        let specs = vec![
            Spec::Agent(AgentSpec {
                path: "zeta.md".into(),
                frontmatter: AgentFrontmatter {
                    id: "zeta-agent".to_owned(),
                    description: "Zeta description".to_owned(),
                    tags: None,
                    execution: None,
                    capabilities: None,
                },
                body: String::new(),
            }),
            Spec::Agent(AgentSpec {
                path: "alpha.md".into(),
                frontmatter: AgentFrontmatter {
                    id: "alpha-agent".to_owned(),
                    description: "Alpha description".to_owned(),
                    tags: None,
                    execution: None,
                    capabilities: None,
                },
                body: String::new(),
            }),
            Spec::Skill(SkillSpec {
                path: "my-skill.md".into(),
                frontmatter: SkillFrontmatter {
                    id: "my-skill".to_owned(),
                    description: Some("Skill description".to_owned()),
                    tags: None,
                    user_invocable: false,
                    agent_invocable: false,
                    execution: None,
                    capabilities: None,
                },
                body: String::new(),
                supporting_files: Vec::new(),
            }),
            Spec::Rule(RuleSpec {
                path: "my-rule.md".into(),
                frontmatter: RuleFrontmatter {
                    id: "my-rule".to_owned(),
                    description: Some("Rule description".to_owned()),
                    tags: None,
                },
                body: String::new(),
            }),
        ];

        TemplateContext::from_specs(&specs)
    }

    #[test]
    fn test_specs_agents_length() {
        let ctx = test_context();
        let fragments = HashMap::new();
        let env = build_environment(&fragments, None).expect("expected value");

        let specs = vec![Spec::Agent(AgentSpec {
            path: "test.md".into(),
            frontmatter: AgentFrontmatter {
                id: "test".to_owned(),
                description: "test".to_owned(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: "{{ specs.agents | length }}".to_owned(),
        })];

        let resolved = resolve_fragments(specs, &env, &ctx).expect("expected value");
        let Spec::Agent(ref s) = resolved[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(s.body, "2");
    }

    #[test]
    fn test_specs_agents_sorted_names() {
        let ctx = test_context();
        let fragments = HashMap::new();
        let env = build_environment(&fragments, None).expect("expected value");

        let specs = vec![Spec::Agent(AgentSpec {
            path: "test.md".into(),
            frontmatter: AgentFrontmatter {
                id: "test".to_owned(),
                description: "test".to_owned(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: "{% for agent in specs.agents %}{{ agent.name }}\n{% endfor %}".to_owned(),
        })];

        let resolved = resolve_fragments(specs, &env, &ctx).expect("expected value");
        let Spec::Agent(ref s) = resolved[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(s.body, "alpha-agent\nzeta-agent\n");
    }

    #[test]
    fn test_specs_all_type_field() {
        let ctx = test_context();
        let fragments = HashMap::new();
        let env = build_environment(&fragments, None).expect("expected value");

        let specs = vec![Spec::Agent(AgentSpec {
            path: "test.md".into(),
            frontmatter: AgentFrontmatter {
                id: "test".to_owned(),
                description: "test".to_owned(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: "{{ specs.all[0].type }}".to_owned(),
        })];

        let resolved = resolve_fragments(specs, &env, &ctx).expect("expected value");
        let Spec::Agent(ref s) = resolved[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(s.body, "agent");
    }

    #[test]
    fn test_fragments_can_access_specs_variable() {
        let ctx = test_context();
        let mut fragments = HashMap::new();
        fragments.insert(
            "listing.md".to_owned(),
            "Skills: {{ specs.skills | length }}".to_owned(),
        );
        let env = build_environment(&fragments, None).expect("expected value");

        let specs = vec![Spec::Agent(AgentSpec {
            path: "test.md".into(),
            frontmatter: AgentFrontmatter {
                id: "test".to_owned(),
                description: "test".to_owned(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: "{% include \"listing.md\" %}".to_owned(),
        })];

        let resolved = resolve_fragments(specs, &env, &ctx).expect("expected value");
        let Spec::Agent(ref s) = resolved[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(s.body, "Skills: 1");
    }

    #[test]
    fn test_no_variable_usage_unchanged() {
        let ctx = test_context();
        let fragments = HashMap::new();
        let env = build_environment(&fragments, None).expect("expected value");

        let specs = vec![Spec::Agent(AgentSpec {
            path: "test.md".into(),
            frontmatter: AgentFrontmatter {
                id: "test".to_owned(),
                description: "test".to_owned(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: "Plain body with no template syntax.".to_owned(),
        })];

        let resolved = resolve_fragments(specs, &env, &ctx).expect("expected value");
        let Spec::Agent(ref s) = resolved[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(s.body, "Plain body with no template syntax.");
    }

    #[test]
    fn test_keyed_access_resolves_prefixed_name() {
        use crate::compile::AdapterConfig;
        use crate::provider::Provider;

        let all_specs = vec![
            Spec::Agent(AgentSpec {
                path: "agent.md".into(),
                frontmatter: AgentFrontmatter {
                    id: "test-agent".to_owned(),
                    description: "An agent".to_owned(),
                    tags: None,
                    execution: None,
                    capabilities: None,
                },
                body: String::new(),
            }),
            Spec::Skill(SkillSpec {
                path: "skill.md".into(),
                frontmatter: SkillFrontmatter {
                    id: "my-skill".to_owned(),
                    description: Some("A skill".to_owned()),
                    tags: None,
                    user_invocable: false,
                    agent_invocable: false,
                    execution: None,
                    capabilities: None,
                },
                body: String::new(),
                supporting_files: Vec::new(),
            }),
        ];

        let cfg = AdapterConfig {
            prefix: Some("tw".to_owned()),
            ..AdapterConfig::default()
        };
        let ctx =
            TemplateContext::from_specs_for_provider(&all_specs, Provider::Claude, Some(&cfg));

        let fragments = HashMap::new();
        let env = build_environment(&fragments, None).expect("expected value");

        // A spec body that references another spec by keyed access
        let specs = vec![Spec::Skill(SkillSpec {
            path: "referrer.md".into(),
            frontmatter: SkillFrontmatter {
                id: "referrer".to_owned(),
                description: Some("Referrer".to_owned()),
                tags: None,
                user_invocable: false,
                agent_invocable: false,
                execution: None,
                capabilities: None,
            },
            body: "Agent: {{ specs.agent.test_agent.name }}".to_owned(),
            supporting_files: Vec::new(),
        })];

        let resolved = resolve_fragments(specs, &env, &ctx).expect("expected value");
        let Spec::Skill(ref s) = resolved[0] else {
            panic!("expected Skill variant")
        };
        assert_eq!(s.body, "Agent: tw-test-agent");
    }

    // --- `tool()` MiniJinja function tests ---

    fn render_body(body: &str, provider: Option<Provider>) -> String {
        let fragments = HashMap::new();
        let env = build_environment(&fragments, provider).expect("expected value");
        let template = env.template_from_str(body).expect("expected value");
        template
            .render(minijinja::context! {})
            .expect("expected value")
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
