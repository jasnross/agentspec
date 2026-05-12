use std::collections::BTreeMap;

use serde::Serialize;

use crate::compile::AdapterConfig;
use crate::provider::Provider;
use crate::spec::Spec;

/// A single spec entry exposed to templates.
#[derive(Clone, Debug, Serialize)]
pub struct SpecEntry {
    /// The spec's name as the model sees it (may be prefixed).
    pub name: String,
    pub description: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub tags: Vec<String>,
}

/// The `specs` variable available in templates.
///
/// Provides both list access (for iteration) and keyed access (for direct
/// lookup by underscore-normalized ID):
///
/// - List: `{% for agent in specs.agents %}{{ agent.name }}{% endfor %}`
/// - Keyed: `{{ specs.skill.gh_safe.name }}`
#[derive(Clone, Debug, Serialize)]
pub struct SpecsContext {
    // List access (for iteration)
    pub agents: Vec<SpecEntry>,
    pub skills: Vec<SpecEntry>,
    pub rules: Vec<SpecEntry>,
    pub all: Vec<SpecEntry>,
    // Keyed access (for `{{ specs.skill.gh_safe.name }}`)
    pub agent: BTreeMap<String, SpecEntry>,
    pub skill: BTreeMap<String, SpecEntry>,
    pub rule: BTreeMap<String, SpecEntry>,
}

/// Top-level template context injected into every render call.
///
/// Extend by adding fields here; each field becomes a top-level template variable.
#[derive(Clone, Debug, Serialize)]
pub struct TemplateContext {
    pub specs: SpecsContext,
}

/// Replace hyphens with underscores for `MiniJinja` dot-access compatibility.
fn normalize_key(id: &str) -> String {
    id.replace('-', "_")
}

/// Shared logic for building a [`SpecsContext`] from specs.
///
/// `name_fn` determines how each entry's `name` is computed: canonical ID for
/// unprefixed contexts, or the model-facing name for provider-specific contexts.
fn build_context(specs: &[Spec], name_fn: impl Fn(&Spec) -> String) -> SpecsContext {
    let mut agents_list = Vec::new();
    let mut skills_list = Vec::new();
    let mut rules_list = Vec::new();
    let mut agent_map = BTreeMap::new();
    let mut skill_map = BTreeMap::new();
    let mut rule_map = BTreeMap::new();

    for spec in specs {
        let entry = SpecEntry {
            name: name_fn(spec),
            description: spec.description().to_owned(),
            r#type: spec.spec_type().to_owned(),
            tags: spec.tags().to_vec(),
        };
        let key = normalize_key(spec.id());

        match spec {
            Spec::Agent(_) => {
                agent_map.insert(key, entry.clone());
                agents_list.push(entry);
            }
            Spec::Skill(_) => {
                skill_map.insert(key, entry.clone());
                skills_list.push(entry);
            }
            Spec::Rule(_) => {
                rule_map.insert(key, entry.clone());
                rules_list.push(entry);
            }
            // Hooks aren't user-referenceable in templates (no `specs.hook.foo`
            // surface today); they participate in the pipeline but are absent
            // from `TemplateContext`. Adding them later is purely additive.
            Spec::Hook(_) => {}
        }
    }

    agents_list.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    skills_list.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    rules_list.sort_unstable_by(|a, b| a.name.cmp(&b.name));

    let mut all: Vec<SpecEntry> = agents_list
        .iter()
        .chain(skills_list.iter())
        .chain(rules_list.iter())
        .cloned()
        .collect();
    all.sort_unstable_by(|a, b| a.name.cmp(&b.name));

    SpecsContext {
        agents: agents_list,
        skills: skills_list,
        rules: rules_list,
        all,
        agent: agent_map,
        skill: skill_map,
        rule: rule_map,
    }
}

impl TemplateContext {
    /// Build the template context from validated specs using canonical
    /// (unprefixed) IDs. Used by the `validate` command and as the default
    /// when no provider context is available.
    pub fn from_specs(specs: &[Spec]) -> Self {
        Self {
            specs: build_context(specs, |s| s.id().to_owned()),
        }
    }

    /// Build a provider-specific template context with prefix-aware names
    /// and keyed access maps.
    ///
    /// The `name` field in each [`SpecEntry`] is the model-facing name for
    /// the target provider (e.g., `tw-gh-safe` for Claude, `gh-safe` for
    /// `OpenCode` skills).
    pub fn from_specs_for_provider(
        specs: &[Spec],
        provider: Provider,
        adapter_config: Option<&AdapterConfig>,
    ) -> Self {
        let adapter = provider.adapter();
        Self {
            specs: build_context(specs, |s| adapter.model_facing_name(s, adapter_config)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{
        AgentFrontmatter, AgentSpec, RuleFrontmatter, RuleSpec, SkillFrontmatter, SkillSpec,
    };

    fn make_agent(id: &str, description: &str) -> Spec {
        make_agent_with_tags(id, description, None)
    }

    fn make_agent_with_tags(id: &str, description: &str, tags: Option<Vec<String>>) -> Spec {
        Spec::Agent(AgentSpec {
            path: format!("{id}.md").into(),
            frontmatter: AgentFrontmatter {
                id: id.to_owned(),
                description: description.to_owned(),
                tags,
                execution: None,
                capabilities: None,
            },
            body: String::new(),
        })
    }

    fn make_skill(id: &str, description: Option<&str>) -> Spec {
        Spec::Skill(SkillSpec {
            path: format!("{id}.md").into(),
            frontmatter: SkillFrontmatter {
                id: id.to_owned(),
                description: description.map(ToOwned::to_owned),
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

    fn make_rule(id: &str, description: Option<&str>) -> Spec {
        Spec::Rule(RuleSpec {
            path: format!("{id}.md").into(),
            frontmatter: RuleFrontmatter {
                id: id.to_owned(),
                description: description.map(ToOwned::to_owned),
                tags: None,
            },
            body: String::new(),
        })
    }

    #[test]
    fn test_from_specs_groups_and_sorts() {
        let specs = vec![
            make_agent("zeta-agent", "Zeta desc"),
            make_agent("alpha-agent", "Alpha desc"),
            make_skill("beta-skill", Some("Beta desc")),
            make_rule("gamma-rule", Some("Gamma desc")),
        ];

        let ctx = TemplateContext::from_specs(&specs);

        assert_eq!(ctx.specs.agents.len(), 2);
        assert_eq!(ctx.specs.agents[0].name, "alpha-agent");
        assert_eq!(ctx.specs.agents[1].name, "zeta-agent");

        assert_eq!(ctx.specs.skills.len(), 1);
        assert_eq!(ctx.specs.skills[0].name, "beta-skill");

        assert_eq!(ctx.specs.rules.len(), 1);
        assert_eq!(ctx.specs.rules[0].name, "gamma-rule");

        // `all` is sorted across all types
        assert_eq!(ctx.specs.all.len(), 4);
        assert_eq!(ctx.specs.all[0].name, "alpha-agent");
        assert_eq!(ctx.specs.all[1].name, "beta-skill");
        assert_eq!(ctx.specs.all[2].name, "gamma-rule");
        assert_eq!(ctx.specs.all[3].name, "zeta-agent");
    }

    #[test]
    fn test_none_description_produces_empty_string() {
        let specs = vec![make_skill("no-desc", None), make_rule("also-no-desc", None)];

        let ctx = TemplateContext::from_specs(&specs);

        assert_eq!(ctx.specs.skills[0].description, "");
        assert_eq!(ctx.specs.rules[0].description, "");
    }

    #[test]
    fn test_all_contains_all_types() {
        let specs = vec![
            make_agent("a", "desc"),
            make_skill("b", Some("desc")),
            make_rule("c", Some("desc")),
        ];

        let ctx = TemplateContext::from_specs(&specs);

        assert_eq!(ctx.specs.all.len(), 3);
        assert_eq!(ctx.specs.all[0].r#type, "agent");
        assert_eq!(ctx.specs.all[1].r#type, "skill");
        assert_eq!(ctx.specs.all[2].r#type, "rule");
    }

    #[test]
    fn test_tags_exposed_in_spec_entry() {
        let specs = vec![make_agent_with_tags(
            "tagged",
            "desc",
            Some(vec!["research".to_string(), "codebase".to_string()]),
        )];

        let ctx = TemplateContext::from_specs(&specs);

        assert_eq!(ctx.specs.agents[0].tags, vec!["research", "codebase"]);
    }

    #[test]
    fn test_no_tags_produces_empty_vec() {
        let specs = vec![make_agent("untagged", "desc")];

        let ctx = TemplateContext::from_specs(&specs);

        assert!(ctx.specs.agents[0].tags.is_empty());
    }

    // --- Keyed access tests ---

    #[test]
    fn test_from_specs_populates_keyed_maps() {
        let specs = vec![
            make_agent("my-agent", "Agent desc"),
            make_skill("gh-safe", Some("Skill desc")),
            make_rule("git-conventions", Some("Rule desc")),
        ];

        let ctx = TemplateContext::from_specs(&specs);

        // Keyed maps use underscore-normalized keys
        assert_eq!(
            ctx.specs.agent.get("my_agent").map(|e| &*e.name),
            Some("my-agent")
        );
        assert_eq!(
            ctx.specs.skill.get("gh_safe").map(|e| &*e.name),
            Some("gh-safe")
        );
        assert_eq!(
            ctx.specs.rule.get("git_conventions").map(|e| &*e.name),
            Some("git-conventions")
        );

        // Hyphenated keys should not exist
        assert!(!ctx.specs.agent.contains_key("my-agent"));
    }

    #[test]
    fn test_from_specs_for_provider_claude_with_prefix() {
        let specs = vec![
            make_agent("my-agent", "Agent desc"),
            make_skill("gh-safe", Some("Skill desc")),
        ];

        let cfg = AdapterConfig {
            prefix: Some("tw".to_owned()),
            ..AdapterConfig::default()
        };
        let ctx = TemplateContext::from_specs_for_provider(&specs, Provider::Claude, Some(&cfg));

        // Claude: all types get prefixed names
        assert_eq!(
            ctx.specs.agent.get("my_agent").map(|e| &*e.name),
            Some("tw-my-agent")
        );
        assert_eq!(
            ctx.specs.skill.get("gh_safe").map(|e| &*e.name),
            Some("tw-gh-safe")
        );

        // Lists also have prefixed names
        assert_eq!(ctx.specs.agents[0].name, "tw-my-agent");
        assert_eq!(ctx.specs.skills[0].name, "tw-gh-safe");
    }

    #[test]
    fn test_from_specs_for_provider_opencode_skills_unprefixed() {
        let specs = vec![
            make_agent("my-agent", "Agent desc"),
            make_skill("gh-safe", Some("Skill desc")),
        ];

        let cfg = AdapterConfig {
            prefix: Some("tw".to_owned()),
            ..AdapterConfig::default()
        };
        let ctx = TemplateContext::from_specs_for_provider(&specs, Provider::OpenCode, Some(&cfg));

        // OpenCode agents: prefixed (identity from filename)
        assert_eq!(
            ctx.specs.agent.get("my_agent").map(|e| &*e.name),
            Some("tw-my-agent")
        );
        // OpenCode skills: unprefixed (identity from frontmatter name)
        assert_eq!(
            ctx.specs.skill.get("gh_safe").map(|e| &*e.name),
            Some("gh-safe")
        );
    }

    #[test]
    fn test_from_specs_for_provider_no_prefix() {
        let specs = vec![
            make_agent("my-agent", "Agent desc"),
            make_skill("gh-safe", Some("Skill desc")),
        ];

        let ctx = TemplateContext::from_specs_for_provider(&specs, Provider::Claude, None);

        // No prefix: names are canonical IDs
        assert_eq!(
            ctx.specs.agent.get("my_agent").map(|e| &*e.name),
            Some("my-agent")
        );
        assert_eq!(
            ctx.specs.skill.get("gh_safe").map(|e| &*e.name),
            Some("gh-safe")
        );
    }

    #[test]
    fn test_from_specs_for_provider_claude_with_content_prefix() {
        let specs = vec![
            make_agent("my-agent", "Agent desc"),
            make_skill("gh-safe", Some("Skill desc")),
        ];

        let cfg = AdapterConfig {
            content_prefix: Some("tw:".to_owned()),
            ..AdapterConfig::default()
        };
        let ctx = TemplateContext::from_specs_for_provider(&specs, Provider::Claude, Some(&cfg));

        // Claude with content_prefix: all types get colon-prefixed names
        assert_eq!(
            ctx.specs.agent.get("my_agent").map(|e| &*e.name),
            Some("tw:my-agent")
        );
        assert_eq!(
            ctx.specs.skill.get("gh_safe").map(|e| &*e.name),
            Some("tw:gh-safe")
        );
    }

    #[test]
    fn test_from_specs_for_provider_opencode_with_content_prefix() {
        let specs = vec![
            make_agent("my-agent", "Agent desc"),
            make_skill("gh-safe", Some("Skill desc")),
        ];

        let cfg = AdapterConfig {
            content_prefix: Some("tw:".to_owned()),
            ..AdapterConfig::default()
        };
        let ctx = TemplateContext::from_specs_for_provider(&specs, Provider::OpenCode, Some(&cfg));

        // OpenCode agents: use content_prefix
        assert_eq!(
            ctx.specs.agent.get("my_agent").map(|e| &*e.name),
            Some("tw:my-agent")
        );
        // OpenCode skills: always unprefixed (ignores prefix for skills)
        assert_eq!(
            ctx.specs.skill.get("gh_safe").map(|e| &*e.name),
            Some("gh-safe")
        );
    }
}
