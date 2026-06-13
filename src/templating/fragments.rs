use anyhow::{Context, Result};
use minijinja::Value;

use super::Templating;
use super::context::TemplateContext;
use super::environment::resolve_include;
use super::validation::validate_child_blocks;
use crate::provider::Provider;
use crate::spec::Spec;

/// Resolve fragment references in spec bodies by rendering them through `MiniJinja`.
///
/// Each spec body is treated as an inline named template rendered in a per-spec
/// environment so that spec-type-specific helpers (e.g. `script()` for
/// skills) are available only where appropriate. The template name is the
/// spec's path relative to `sources_dir`, enabling `./`-prefixed self-relative
/// includes via `MiniJinja`'s path join callback.
pub fn resolve_fragments(
    specs: Vec<Spec>,
    templating: &Templating,
    provider: Option<Provider>,
    context: &TemplateContext,
) -> Result<Vec<Spec>> {
    let ctx = Value::from_serialize(context);
    let mut resolved = Vec::with_capacity(specs.len());

    for mut spec in specs {
        let resolver = |name: &str| -> Result<Option<String>> {
            resolve_include(name, templating.sources_dir(), templating.extra_dirs())
                .map_err(|e| anyhow::anyhow!("{e}"))
        };
        validate_child_blocks(spec.body(), &resolver, spec.path())?;

        let env = templating.build_environment(provider, &spec);

        let spec_name = spec
            .path()
            .strip_prefix(templating.sources_dir())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        let template = env
            .template_from_named_str(&spec_name, spec.body())
            .with_context(|| format!("failed to parse template in {}", spec.path().display()))?;

        let body = template
            .render(&ctx)
            .with_context(|| format!("failed to render template in {}", spec.path().display()))?;

        match &mut spec {
            Spec::Agent(s) => s.body = body,
            Spec::Skill(s) => s.body = body,
            Spec::Rule(s) => s.body = body,
            Spec::Hook(_) => {}
        }

        resolved.push(spec);
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use indexmap::IndexMap;

    use super::*;
    use crate::spec::{
        AgentFrontmatter, AgentSpec, RuleFrontmatter, RuleSpec, SkillFrontmatter, SkillSpec,
    };
    use crate::templating::Templating;

    fn empty_context() -> TemplateContext {
        TemplateContext::from_specs(&[])
    }

    fn make_templating(tmp: &std::path::Path) -> Templating {
        Templating::from_sources(tmp.to_path_buf(), vec![])
    }

    #[test]
    fn test_resolve_fragments_no_syntax() {
        let tmp = tempfile::tempdir().expect("expected value");
        let templating = make_templating(tmp.path());

        let specs = vec![Spec::Agent(AgentSpec {
            path: tmp.path().join("test.md"),
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
        let tmp = tempfile::tempdir().expect("expected value");
        fs::write(tmp.path().join("footer.md"), "-- End --").expect("expected value");
        let templating = make_templating(tmp.path());

        let specs = vec![Spec::Agent(AgentSpec {
            path: tmp.path().join("test.md"),
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
                supporting_files: IndexMap::new(),
            }),
            Spec::Rule(RuleSpec {
                path: "my-rule.md".into(),
                frontmatter: RuleFrontmatter {
                    id: "my-rule".to_owned(),
                    description: Some("Rule description".to_owned()),
                    tags: None,
                    paths: None,
                },
                body: String::new(),
            }),
        ];

        TemplateContext::from_specs(&specs)
    }

    #[test]
    fn test_specs_agents_length() {
        let ctx = test_context();
        let tmp = tempfile::tempdir().expect("expected value");
        let templating = make_templating(tmp.path());

        let specs = vec![Spec::Agent(AgentSpec {
            path: tmp.path().join("test.md"),
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
        let tmp = tempfile::tempdir().expect("expected value");
        let templating = make_templating(tmp.path());

        let specs = vec![Spec::Agent(AgentSpec {
            path: tmp.path().join("test.md"),
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
        let tmp = tempfile::tempdir().expect("expected value");
        let templating = make_templating(tmp.path());

        let specs = vec![Spec::Agent(AgentSpec {
            path: tmp.path().join("test.md"),
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
        let tmp = tempfile::tempdir().expect("expected value");
        fs::write(
            tmp.path().join("listing.md"),
            "Skills: {{ specs.skills | length }}",
        )
        .expect("expected value");
        let templating = make_templating(tmp.path());

        let specs = vec![Spec::Agent(AgentSpec {
            path: tmp.path().join("test.md"),
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
        let tmp = tempfile::tempdir().expect("expected value");
        let templating = make_templating(tmp.path());

        let specs = vec![Spec::Agent(AgentSpec {
            path: tmp.path().join("test.md"),
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
                supporting_files: IndexMap::new(),
            }),
        ];

        let cfg = AdapterConfig {
            prefix: Some("tw".to_owned()),
            ..AdapterConfig::default()
        };
        let ctx =
            TemplateContext::from_specs_for_provider(&all_specs, Provider::Claude, Some(&cfg));

        let tmp = tempfile::tempdir().expect("expected value");
        let templating = make_templating(tmp.path());

        let specs = vec![Spec::Skill(SkillSpec {
            path: tmp.path().join("referrer.md"),
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
            supporting_files: IndexMap::new(),
        })];

        let resolved = resolve_fragments(specs, &templating, None, &ctx).expect("expected value");
        let Spec::Skill(ref s) = resolved[0] else {
            panic!("expected Skill variant")
        };
        assert_eq!(s.body, "Agent: tw-test-agent");
    }

    #[test]
    fn test_resolve_fragments_errors_for_non_skill_script() {
        use crate::provider::Provider;

        let tmp = tempfile::tempdir().expect("expected value");
        let templating = make_templating(tmp.path());
        let specs = vec![Spec::Agent(AgentSpec {
            path: tmp.path().join("agent.md"),
            frontmatter: AgentFrontmatter {
                id: "test-agent".to_owned(),
                description: "An agent".to_owned(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: r#"{{ script("foo.sh") }}"#.to_owned(),
        })];

        let err = resolve_fragments(specs, &templating, Some(Provider::Claude), &empty_context())
            .expect_err("expected render error for script() in agent body");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("failed to render template in"),
            "expected with_context prefix in error, got: {msg}"
        );
        assert!(
            msg.contains("script"),
            "expected 'script' in error, got: {msg}"
        );
    }

    #[test]
    fn test_resolve_fragments_with_extends() {
        let tmp = tempfile::tempdir().expect("expected value");
        fs::write(tmp.path().join("note.md"), "a note").expect("expected value");
        fs::create_dir_all(tmp.path().join("templates")).expect("expected value");
        fs::write(
            tmp.path().join("templates/base.md"),
            "Header\n{% block body %}default{% endblock %}\nFooter",
        )
        .expect("expected value");
        let templating = make_templating(tmp.path());

        let specs = vec![Spec::Agent(AgentSpec {
            path: tmp.path().join("test.md"),
            frontmatter: AgentFrontmatter {
                id: "test".to_string(),
                description: "test".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: concat!(
                "{% extends \"templates/base.md\" %}",
                "{% block body %}custom body{% endblock %}"
            )
            .to_string(),
        })];

        let resolved =
            resolve_fragments(specs, &templating, None, &empty_context()).expect("expected value");
        let Spec::Agent(ref s) = resolved[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(s.body, "Header\ncustom body\nFooter");
    }

    #[test]
    fn test_resolve_fragments_rejects_unrecognized_block() {
        let tmp = tempfile::tempdir().expect("expected value");
        fs::create_dir_all(tmp.path().join("templates")).expect("expected value");
        fs::write(
            tmp.path().join("templates/base.md"),
            "{% block title %}default{% endblock %}",
        )
        .expect("expected value");
        let templating = make_templating(tmp.path());

        let specs = vec![Spec::Agent(AgentSpec {
            path: tmp.path().join("bad-spec.md"),
            frontmatter: AgentFrontmatter {
                id: "bad".to_string(),
                description: "bad".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: concat!(
                "{% extends \"templates/base.md\" %}",
                "{% block typo %}oops{% endblock %}"
            )
            .to_string(),
        })];

        let err = resolve_fragments(specs, &templating, None, &empty_context())
            .expect_err("expected error for unrecognized block");
        let msg = err.to_string();
        assert!(msg.contains("typo"), "error should name the block: {msg}");
        assert!(
            msg.contains("bad-spec.md"),
            "error should name the spec: {msg}"
        );
    }
}
