use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use minijinja::Value;
use walkdir::WalkDir;

use super::Templating;
use super::context::TemplateContext;
use crate::provider::Provider;
use crate::spec::Spec;

/// Resolve fragment references in spec bodies by rendering them through `MiniJinja`.
///
/// Each spec body is treated as an inline template rendered in a per-spec
/// environment so that spec-type-specific helpers (e.g. `script_path()` for
/// skills) are available only where appropriate. Specs that contain no template
/// syntax pass through unchanged. Operates on validated specs so that template
/// resolution is decoupled from the spec loading/validation lifecycle.
pub fn resolve_fragments(
    specs: Vec<Spec>,
    templating: &Templating,
    provider: Option<Provider>,
    context: &TemplateContext,
) -> Result<Vec<Spec>> {
    let ctx = Value::from_serialize(context);
    let mut resolved = Vec::with_capacity(specs.len());

    for mut spec in specs {
        let env = templating.build_environment(provider, &spec)?;
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::spec::{
        AgentFrontmatter, AgentSpec, RuleFrontmatter, RuleSpec, SkillFrontmatter, SkillSpec,
    };
    use crate::templating::Templating;

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
    fn test_resolve_fragments_no_syntax() {
        let templating = Templating::from_fragments(HashMap::new());

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

        let resolved =
            resolve_fragments(specs, &templating, None, &empty_context()).expect("expected value");
        let Spec::Agent(ref s) = resolved[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(s.body, "Plain body with no template syntax.");
    }

    #[test]
    fn test_resolve_fragments_with_include() {
        let mut fragments = HashMap::new();
        fragments.insert("footer.md".to_string(), "-- End --".to_string());
        let templating = Templating::from_fragments(fragments);

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

        let resolved =
            resolve_fragments(specs, &templating, None, &empty_context()).expect("expected value");
        let Spec::Agent(ref s) = resolved[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(s.body, "Body.\n-- End --");
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
        let templating = Templating::from_fragments(HashMap::new());

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

        let resolved = resolve_fragments(specs, &templating, None, &ctx).expect("expected value");
        let Spec::Agent(ref s) = resolved[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(s.body, "2");
    }

    #[test]
    fn test_specs_agents_sorted_names() {
        let ctx = test_context();
        let templating = Templating::from_fragments(HashMap::new());

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

        let resolved = resolve_fragments(specs, &templating, None, &ctx).expect("expected value");
        let Spec::Agent(ref s) = resolved[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(s.body, "alpha-agent\nzeta-agent\n");
    }

    #[test]
    fn test_specs_all_type_field() {
        let ctx = test_context();
        let templating = Templating::from_fragments(HashMap::new());

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

        let resolved = resolve_fragments(specs, &templating, None, &ctx).expect("expected value");
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
        let templating = Templating::from_fragments(fragments);

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

        let resolved = resolve_fragments(specs, &templating, None, &ctx).expect("expected value");
        let Spec::Agent(ref s) = resolved[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(s.body, "Skills: 1");
    }

    #[test]
    fn test_no_variable_usage_unchanged() {
        let ctx = test_context();
        let templating = Templating::from_fragments(HashMap::new());

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

        let resolved = resolve_fragments(specs, &templating, None, &ctx).expect("expected value");
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

        let templating = Templating::from_fragments(HashMap::new());

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

        let resolved = resolve_fragments(specs, &templating, None, &ctx).expect("expected value");
        let Spec::Skill(ref s) = resolved[0] else {
            panic!("expected Skill variant")
        };
        assert_eq!(s.body, "Agent: tw-test-agent");
    }

    #[test]
    fn test_resolve_fragments_errors_for_non_skill_script_path() {
        use crate::provider::Provider;

        let templating = Templating::from_fragments(HashMap::new());
        let specs = vec![Spec::Agent(AgentSpec {
            path: "agent.md".into(),
            frontmatter: AgentFrontmatter {
                id: "test-agent".to_owned(),
                description: "An agent".to_owned(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: r#"{{ script_path("scripts/foo.sh") }}"#.to_owned(),
        })];

        let err = resolve_fragments(specs, &templating, Some(Provider::Claude), &empty_context())
            .expect_err("expected render error for script_path in agent body");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("failed to render template in"),
            "expected with_context prefix in error, got: {msg}"
        );
        assert!(
            msg.contains("script_path"),
            "expected 'script_path' in error, got: {msg}"
        );
    }
}
