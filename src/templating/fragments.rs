use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use minijinja::Value;
use walkdir::WalkDir;

use super::Templating;
use super::context::TemplateContext;
use super::validation::validate_child_blocks;
use crate::provider::Provider;
use crate::spec::Spec;

/// Resolve fragment references in spec bodies by rendering them through `MiniJinja`.
///
/// Each spec body is treated as an inline template rendered in a per-spec
/// environment so that spec-type-specific helpers (e.g. `script()` for
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
        validate_child_blocks(spec.body(), templating.template_map(), spec.path())?;

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

/// Load template files from a directory. Returns a map of template name to content.
///
/// Template names are prefixed with `templates/` to match the
/// `{% extends "templates/<name>.md" %}` syntax used in spec bodies. A file at
/// `templates/critique.md` keys as `templates/critique.md`.
pub fn load_templates(templates_dir: &Path) -> Result<HashMap<String, String>> {
    let mut templates = HashMap::new();

    if !templates_dir.is_dir() {
        return Ok(templates);
    }

    let entries = WalkDir::new(templates_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|ext| ext == "md"));

    for entry in entries {
        let path = entry.path();
        let relative = path
            .strip_prefix(templates_dir)
            .context("failed to compute relative path for template")?;

        let name = format!("templates/{}", relative.to_string_lossy());

        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read template {}", path.display()))?;

        templates.insert(name, content);
    }

    Ok(templates)
}

/// Load and merge fragments from the local directory plus zero or more extra
/// directories. Returns a flat `HashMap<String, String>` (fragment name →
/// content) suitable for registering into a `MiniJinja` environment.
///
/// The local `fragments/` dir may not exist (returns empty — this is the
/// existing behaviour for projects that don't use fragments). Extra dirs that
/// don't exist produce an error (explicit config implies intent, so a missing
/// dir is likely a typo). Fragment name collisions across any pair of sources
/// (local↔extra, extra↔extra) are reported with both source paths.
pub fn load_all_fragments(
    local_dir: &Path,
    extra_dirs: &[PathBuf],
) -> Result<HashMap<String, String>> {
    let mut merged = HashMap::new();
    let mut provenance: HashMap<String, PathBuf> = HashMap::new();

    let local = load_fragments(local_dir)?;
    for (name, content) in local {
        provenance.insert(name.clone(), local_dir.to_path_buf());
        merged.insert(name, content);
    }

    for extra_dir in extra_dirs {
        if !extra_dir.is_dir() {
            bail!(
                "extra fragment directory does not exist: {}",
                extra_dir.display()
            );
        }
        let extra = load_fragments(extra_dir)?;
        for (name, content) in extra {
            if let Some(existing_dir) = provenance.get(&name) {
                bail!(
                    "fragment name collision: \"{name}\"\n  \
                     --> {}\n  \
                     --> {}\n  \
                     = both resolve to the same include name; rename one to disambiguate",
                    existing_dir.join(&name).display(),
                    extra_dir.join(&name).display(),
                );
            }
            provenance.insert(name.clone(), extra_dir.clone());
            merged.insert(name, content);
        }
    }

    Ok(merged)
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
                supporting_files: IndexMap::new(),
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
    fn test_load_all_fragments_merges_extra_dirs() {
        let tmp = tempfile::tempdir().expect("expected value");
        let local = tmp.path().join("local");
        let extra = tmp.path().join("extra");
        fs::create_dir_all(&local).expect("expected value");
        fs::create_dir_all(&extra).expect("expected value");
        fs::write(local.join("local.md"), "local content").expect("expected value");
        fs::write(extra.join("extra.md"), "extra content").expect("expected value");

        let result = load_all_fragments(&local, &[extra]).expect("expected value");
        assert_eq!(result.len(), 2);
        assert_eq!(result["local.md"], "local content");
        assert_eq!(result["extra.md"], "extra content");
    }

    #[test]
    fn test_load_all_fragments_collision_local_vs_extra() {
        let tmp = tempfile::tempdir().expect("expected value");
        let local = tmp.path().join("local");
        let extra = tmp.path().join("extra");
        fs::create_dir_all(&local).expect("expected value");
        fs::create_dir_all(&extra).expect("expected value");
        fs::write(local.join("shared.md"), "from local").expect("expected value");
        fs::write(extra.join("shared.md"), "from extra").expect("expected value");

        let err = load_all_fragments(&local, std::slice::from_ref(&extra))
            .expect_err("expected collision error");
        let msg = err.to_string();
        assert!(msg.contains("collision"), "error: {msg}");
        assert!(
            msg.contains(&local.join("shared.md").display().to_string()),
            "error: {msg}"
        );
        assert!(
            msg.contains(&extra.join("shared.md").display().to_string()),
            "error: {msg}"
        );
    }

    #[test]
    fn test_load_all_fragments_collision_extra_vs_extra() {
        let tmp = tempfile::tempdir().expect("expected value");
        let local = tmp.path().join("local");
        let extra_a = tmp.path().join("extra_a");
        let extra_b = tmp.path().join("extra_b");
        fs::create_dir_all(&local).expect("expected value");
        fs::create_dir_all(&extra_a).expect("expected value");
        fs::create_dir_all(&extra_b).expect("expected value");
        fs::write(extra_a.join("dup.md"), "from a").expect("expected value");
        fs::write(extra_b.join("dup.md"), "from b").expect("expected value");

        let err = load_all_fragments(&local, &[extra_a.clone(), extra_b.clone()])
            .expect_err("expected collision error");
        let msg = err.to_string();
        assert!(msg.contains("collision"), "error: {msg}");
        assert!(
            msg.contains(&extra_a.join("dup.md").display().to_string()),
            "error: {msg}"
        );
        assert!(
            msg.contains(&extra_b.join("dup.md").display().to_string()),
            "error: {msg}"
        );
    }

    #[test]
    fn test_load_all_fragments_missing_extra_dir_errors() {
        let tmp = tempfile::tempdir().expect("expected value");
        let local = tmp.path().join("local");
        fs::create_dir_all(&local).expect("expected value");
        let missing = tmp.path().join("nonexistent");

        let err = load_all_fragments(&local, &[missing]).expect_err("expected missing dir error");
        let msg = err.to_string();
        assert!(msg.contains("does not exist"), "error: {msg}");
    }

    #[test]
    fn test_load_all_fragments_no_extras() {
        let tmp = tempfile::tempdir().expect("expected value");
        let local = tmp.path().join("local");
        fs::create_dir_all(&local).expect("expected value");
        fs::write(local.join("note.md"), "hello").expect("expected value");

        let result = load_all_fragments(&local, &[]).expect("expected value");
        assert_eq!(result.len(), 1);
        assert_eq!(result["note.md"], "hello");

        let nonexistent = tmp.path().join("no-such-dir");
        let empty = load_all_fragments(&nonexistent, &[]).expect("expected value");
        assert!(empty.is_empty());
    }

    #[test]
    fn test_load_all_fragments_nested_subdirs_in_extra() {
        let tmp = tempfile::tempdir().expect("expected value");
        let local = tmp.path().join("local");
        let extra = tmp.path().join("extra");
        fs::create_dir_all(&local).expect("expected value");
        fs::create_dir_all(extra.join("sub")).expect("expected value");
        fs::write(extra.join("top.md"), "top").expect("expected value");
        fs::write(extra.join("sub/nested.md"), "nested").expect("expected value");

        let result = load_all_fragments(&local, &[extra]).expect("expected value");
        assert_eq!(result.len(), 2);
        assert_eq!(result["top.md"], "top");
        assert_eq!(result["sub/nested.md"], "nested");
    }

    #[test]
    fn test_load_templates_basic() {
        let tmp = tempfile::tempdir().expect("expected value");
        let tpl_dir = tmp.path().join("templates");
        fs::create_dir_all(&tpl_dir).expect("expected value");
        fs::write(tpl_dir.join("critique.md"), "template content").expect("expected value");

        let templates = load_templates(&tpl_dir).expect("expected value");
        assert_eq!(templates.len(), 1);
        assert_eq!(templates["templates/critique.md"], "template content");
    }

    #[test]
    fn test_load_templates_nested_subdir() {
        let tmp = tempfile::tempdir().expect("expected value");
        let tpl_dir = tmp.path().join("templates");
        fs::create_dir_all(tpl_dir.join("review")).expect("expected value");
        fs::write(tpl_dir.join("review/base.md"), "nested template").expect("expected value");

        let templates = load_templates(&tpl_dir).expect("expected value");
        assert_eq!(templates.len(), 1);
        assert_eq!(templates["templates/review/base.md"], "nested template");
    }

    #[test]
    fn test_load_templates_nonexistent_dir() {
        let tmp = tempfile::tempdir().expect("expected value");
        let templates = load_templates(&tmp.path().join("nonexistent")).expect("expected value");
        assert!(templates.is_empty());
    }

    #[test]
    fn test_template_fragment_collision_detected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let frag_dir = tmp.path().join("fragments");
        let tpl_dir = tmp.path().join("templates");

        // A fragment at fragments/templates/x.md keys as "templates/x.md"
        fs::create_dir_all(frag_dir.join("templates")).expect("expected value");
        fs::write(frag_dir.join("templates/x.md"), "fragment content").expect("expected value");

        // A template at templates/x.md also keys as "templates/x.md"
        fs::create_dir_all(&tpl_dir).expect("expected value");
        fs::write(tpl_dir.join("x.md"), "template content").expect("expected value");

        let err = Templating::load(&frag_dir, &[], &tpl_dir).expect_err("expected collision error");
        let msg = err.to_string();
        assert!(msg.contains("collision"), "error: {msg}");
        assert!(msg.contains("templates/x.md"), "error: {msg}");
    }

    #[test]
    fn test_resolve_fragments_with_extends() {
        let mut fragments = HashMap::new();
        fragments.insert("note.md".to_string(), "a note".to_string());

        let mut templates = HashMap::new();
        templates.insert(
            "templates/base.md".to_string(),
            "Header\n{% block body %}default{% endblock %}\nFooter".to_string(),
        );
        let templating = Templating::from_fragments_and_templates(fragments, templates);

        let specs = vec![Spec::Agent(AgentSpec {
            path: "test.md".into(),
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
        let templates = HashMap::from([(
            "templates/base.md".to_string(),
            "{% block title %}default{% endblock %}".to_string(),
        )]);
        let templating = Templating::from_fragments_and_templates(HashMap::new(), templates);

        let specs = vec![Spec::Agent(AgentSpec {
            path: "bad-spec.md".into(),
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
