use std::collections::BTreeMap;

use serde::Serialize;

use crate::compile::AdapterConfig;
use crate::provider::Provider;
use crate::spec::Spec;

/// Top-level template context injected into every render call.
///
/// Extend by adding fields here; each field becomes a top-level template variable.
#[derive(Clone, Debug, Serialize)]
pub struct TemplateContext {
    pub specs: SpecsContext,
}

impl TemplateContext {
    /// Build the template context from validated specs using canonical
    /// (unprefixed) IDs. Used by the `validate` command and as the default
    /// when no provider context is available.
    pub fn from_specs(specs: &[Spec]) -> Self {
        Self {
            specs: build_specs_context(specs, |s| s.id().to_owned()),
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
            specs: build_specs_context(specs, |s| adapter.body_spec_name(s, adapter_config)),
        }
    }
}

/// The fields every spec entry exposed to templates carries, whatever its
/// kind.
#[derive(Clone, Debug, Serialize)]
pub struct SpecEntry {
    /// The spec's name as the model sees it (may be prefixed).
    pub name: String,
    pub description: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub tags: Vec<String>,
}

/// A skill entry exposed to templates: the shared fields plus whether an
/// agent may load the skill.
#[derive(Clone, Debug, Serialize)]
pub struct SkillEntry {
    #[serde(flatten)]
    pub entry: SpecEntry,
    /// Whether an agent can load the skill on its own.
    pub agent_invocable: bool,
}

/// An entry in `specs.all`, which mixes spec kinds.
///
/// Untagged, so each variant serializes as its inner entry and templates
/// see the same flat object they would through the per-kind lists.
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum AnyEntry {
    Agent(SpecEntry),
    Skill(SkillEntry),
    Rule(SpecEntry),
}

impl AnyEntry {
    /// The fields every kind shares.
    pub fn entry(&self) -> &SpecEntry {
        match self {
            AnyEntry::Agent(entry) | AnyEntry::Rule(entry) => entry,
            AnyEntry::Skill(skill) => &skill.entry,
        }
    }
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
    pub skills: Vec<SkillEntry>,
    pub rules: Vec<SpecEntry>,
    pub all: Vec<AnyEntry>,
    // Keyed access (for `{{ specs.skill.gh_safe.name }}`)
    pub agent: BTreeMap<String, SpecEntry>,
    pub skill: BTreeMap<String, SkillEntry>,
    pub rule: BTreeMap<String, SpecEntry>,
}

/// Shared logic for building a [`SpecsContext`] from specs.
///
/// `name_fn` determines how each entry's `name` is computed: canonical ID for
/// unprefixed contexts, or the model-facing name for provider-specific contexts.
fn build_specs_context(specs: &[Spec], name_fn: impl Fn(&Spec) -> String) -> SpecsContext {
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
            Spec::Skill(skill) => {
                let entry = SkillEntry {
                    entry,
                    agent_invocable: skill.frontmatter.agent_invocable,
                };
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
    skills_list.sort_unstable_by(|a, b| a.entry.name.cmp(&b.entry.name));
    rules_list.sort_unstable_by(|a, b| a.name.cmp(&b.name));

    let mut all: Vec<AnyEntry> = agents_list
        .iter()
        .cloned()
        .map(AnyEntry::Agent)
        .chain(skills_list.iter().cloned().map(AnyEntry::Skill))
        .chain(rules_list.iter().cloned().map(AnyEntry::Rule))
        .collect();
    all.sort_unstable_by(|a, b| a.entry().name.cmp(&b.entry().name));

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

/// Replace hyphens with underscores for `MiniJinja` dot-access compatibility.
fn normalize_key(id: &str) -> String {
    id.replace('-', "_")
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

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

    fn make_skill(id: &str, description: Option<&str>, agent_invocable: bool) -> Spec {
        Spec::Skill(SkillSpec {
            path: format!("{id}.md").into(),
            frontmatter: SkillFrontmatter {
                id: id.to_owned(),
                description: description.map(ToOwned::to_owned),
                tags: None,
                user_invocable: false,
                agent_invocable,
                execution: None,
                capabilities: None,
            },
            body: String::new(),
            supporting_files: IndexMap::new(),
        })
    }

    fn make_rule(id: &str, description: Option<&str>) -> Spec {
        Spec::Rule(RuleSpec {
            path: format!("{id}.md").into(),
            frontmatter: RuleFrontmatter {
                id: id.to_owned(),
                description: description.map(ToOwned::to_owned),
                tags: None,
                paths: None,
            },
            body: String::new(),
        })
    }

    #[test]
    fn test_from_specs_groups_and_sorts() {
        let specs = vec![
            make_agent("zeta-agent", "Zeta desc"),
            make_agent("alpha-agent", "Alpha desc"),
            make_skill("beta-skill", Some("Beta desc"), false),
            make_rule("gamma-rule", Some("Gamma desc")),
        ];

        let ctx = TemplateContext::from_specs(&specs);

        assert_eq!(ctx.specs.agents.len(), 2);
        assert_eq!(ctx.specs.agents[0].name, "alpha-agent");
        assert_eq!(ctx.specs.agents[1].name, "zeta-agent");

        assert_eq!(ctx.specs.skills.len(), 1);
        assert_eq!(ctx.specs.skills[0].entry.name, "beta-skill");

        assert_eq!(ctx.specs.rules.len(), 1);
        assert_eq!(ctx.specs.rules[0].name, "gamma-rule");

        // `all` is sorted across all types
        assert_eq!(ctx.specs.all.len(), 4);
        assert_eq!(ctx.specs.all[0].entry().name, "alpha-agent");
        assert_eq!(ctx.specs.all[1].entry().name, "beta-skill");
        assert_eq!(ctx.specs.all[2].entry().name, "gamma-rule");
        assert_eq!(ctx.specs.all[3].entry().name, "zeta-agent");
    }

    #[test]
    fn test_none_description_produces_empty_string() {
        let specs = vec![
            make_skill("no-desc", None, false),
            make_rule("also-no-desc", None),
        ];

        let ctx = TemplateContext::from_specs(&specs);

        assert_eq!(ctx.specs.skills[0].entry.description, "");
        assert_eq!(ctx.specs.rules[0].description, "");
    }

    #[test]
    fn test_all_contains_all_types() {
        let specs = vec![
            make_agent("a", "desc"),
            make_skill("b", Some("desc"), false),
            make_rule("c", Some("desc")),
        ];

        let ctx = TemplateContext::from_specs(&specs);

        assert_eq!(ctx.specs.all.len(), 3);
        assert_eq!(ctx.specs.all[0].entry().r#type, "agent");
        assert_eq!(ctx.specs.all[1].entry().r#type, "skill");
        assert_eq!(ctx.specs.all[2].entry().r#type, "rule");
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
            make_skill("gh-safe", Some("Skill desc"), false),
            make_rule("git-conventions", Some("Rule desc")),
        ];

        let ctx = TemplateContext::from_specs(&specs);

        // Keyed maps use underscore-normalized keys
        assert_eq!(
            ctx.specs.agent.get("my_agent").map(|e| &*e.name),
            Some("my-agent")
        );
        assert_eq!(
            ctx.specs.skill.get("gh_safe").map(|e| &*e.entry.name),
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
            make_skill("gh-safe", Some("Skill desc"), false),
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
            ctx.specs.skill.get("gh_safe").map(|e| &*e.entry.name),
            Some("tw-gh-safe")
        );

        // Lists also have prefixed names
        assert_eq!(ctx.specs.agents[0].name, "tw-my-agent");
        assert_eq!(ctx.specs.skills[0].entry.name, "tw-gh-safe");
    }

    #[test]
    fn test_from_specs_for_provider_opencode_skills_unprefixed() {
        let specs = vec![
            make_agent("my-agent", "Agent desc"),
            make_skill("gh-safe", Some("Skill desc"), false),
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
            ctx.specs.skill.get("gh_safe").map(|e| &*e.entry.name),
            Some("gh-safe")
        );
    }

    #[test]
    fn test_from_specs_for_provider_no_prefix() {
        let specs = vec![
            make_agent("my-agent", "Agent desc"),
            make_skill("gh-safe", Some("Skill desc"), false),
        ];

        let ctx = TemplateContext::from_specs_for_provider(&specs, Provider::Claude, None);

        // No prefix: names are canonical IDs
        assert_eq!(
            ctx.specs.agent.get("my_agent").map(|e| &*e.name),
            Some("my-agent")
        );
        assert_eq!(
            ctx.specs.skill.get("gh_safe").map(|e| &*e.entry.name),
            Some("gh-safe")
        );
    }

    #[test]
    fn test_from_specs_for_provider_claude_with_content_prefix() {
        let specs = vec![
            make_agent("my-agent", "Agent desc"),
            make_skill("gh-safe", Some("Skill desc"), false),
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
            ctx.specs.skill.get("gh_safe").map(|e| &*e.entry.name),
            Some("tw:gh-safe")
        );
    }

    #[test]
    fn test_from_specs_for_provider_opencode_with_content_prefix() {
        let specs = vec![
            make_agent("my-agent", "Agent desc"),
            make_skill("gh-safe", Some("Skill desc"), false),
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
            ctx.specs.skill.get("gh_safe").map(|e| &*e.entry.name),
            Some("gh-safe")
        );
    }

    #[test]
    fn test_skill_entries_carry_agent_invocable() {
        let specs = vec![
            make_skill("user-only", Some("d"), false),
            make_skill("agent-loadable", Some("d"), true),
        ];

        let ctx = TemplateContext::from_specs(&specs);

        assert!(!ctx.specs.skill["user_only"].agent_invocable);
        assert!(ctx.specs.skill["agent_loadable"].agent_invocable);

        let listed = |name: &str| {
            ctx.specs
                .skills
                .iter()
                .find(|s| s.entry.name == name)
                .map(|s| s.agent_invocable)
        };
        assert_eq!(listed("user-only"), Some(false));
        assert_eq!(listed("agent-loadable"), Some(true));
    }

    #[test]
    fn test_all_skill_entries_serialize_agent_invocable() {
        let specs = vec![
            make_agent("a", "desc"),
            make_skill("b", Some("desc"), false),
            make_rule("c", Some("desc")),
        ];

        let ctx = TemplateContext::from_specs(&specs);
        let objects: Vec<serde_json::Value> = ctx
            .specs
            .all
            .iter()
            .map(|e| serde_json::to_value(e).expect("entry serializes"))
            .collect();

        let skill = &objects[1];
        assert_eq!(skill["type"], "skill");
        assert_eq!(skill["agent_invocable"], serde_json::Value::Bool(false));
        for key in ["name", "description", "type", "tags"] {
            assert!(
                skill.get(key).is_some(),
                "skill entry lacks top-level `{key}`"
            );
        }

        for other in [&objects[0], &objects[2]] {
            assert!(
                other.get("agent_invocable").is_none(),
                "non-skill entry carries agent_invocable: {other}"
            );
        }
        for object in &objects {
            assert!(
                object.get("user_invocable").is_none(),
                "entry carries user_invocable: {object}"
            );
        }
    }
}
