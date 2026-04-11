use serde::Serialize;

use crate::spec::NormalizedSpec;

/// A single spec entry exposed to templates.
#[derive(Clone, Debug, Serialize)]
pub struct SpecEntry {
    /// The spec's `id` field, exposed as `name` in templates for readability.
    pub name: String,
    pub description: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub tags: Vec<String>,
}

/// The `specs` variable available in templates — grouped by type and as a flat list.
#[derive(Clone, Debug, Serialize)]
pub struct SpecsContext {
    pub agents: Vec<SpecEntry>,
    pub skills: Vec<SpecEntry>,
    pub rules: Vec<SpecEntry>,
    pub all: Vec<SpecEntry>,
}

/// Top-level template context injected into every render call.
///
/// Extend by adding fields here; each field becomes a top-level template variable.
#[derive(Clone, Debug, Serialize)]
pub struct TemplateContext {
    pub specs: SpecsContext,
}

impl TemplateContext {
    /// Build the template context from validated specs.
    ///
    /// Entries are sorted alphabetically by name within each group and in `all`.
    pub fn from_specs(specs: &[NormalizedSpec]) -> Self {
        let mut agents = Vec::new();
        let mut skills = Vec::new();
        let mut rules = Vec::new();

        for spec in specs {
            let entry = SpecEntry {
                name: spec.id().to_owned(),
                description: spec.description().to_owned(),
                r#type: spec.spec_type().to_owned(),
                tags: spec.tags().to_vec(),
            };
            match spec {
                NormalizedSpec::Agent(_) => agents.push(entry),
                NormalizedSpec::Skill(_) => skills.push(entry),
                NormalizedSpec::Rule(_) => rules.push(entry),
            }
        }

        agents.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        skills.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        rules.sort_unstable_by(|a, b| a.name.cmp(&b.name));

        let mut all: Vec<SpecEntry> = agents
            .iter()
            .chain(skills.iter())
            .chain(rules.iter())
            .cloned()
            .collect();
        all.sort_unstable_by(|a, b| a.name.cmp(&b.name));

        Self {
            specs: SpecsContext {
                agents,
                skills,
                rules,
                all,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{
        NormalizedAgentFrontmatter, NormalizedAgentSpec, NormalizedRuleFrontmatter,
        NormalizedRuleSpec, NormalizedSkillFrontmatter, NormalizedSkillSpec,
    };

    fn make_agent(id: &str, description: &str) -> NormalizedSpec {
        make_agent_with_tags(id, description, None)
    }

    fn make_agent_with_tags(
        id: &str,
        description: &str,
        tags: Option<Vec<String>>,
    ) -> NormalizedSpec {
        NormalizedSpec::Agent(NormalizedAgentSpec {
            path: format!("{id}.md").into(),
            frontmatter: NormalizedAgentFrontmatter {
                id: id.to_owned(),
                description: description.to_owned(),
                tags,
                execution: None,
                capabilities: None,
            },
            body: String::new(),
        })
    }

    fn make_skill(id: &str, description: Option<&str>) -> NormalizedSpec {
        NormalizedSpec::Skill(NormalizedSkillSpec {
            path: format!("{id}.md").into(),
            frontmatter: NormalizedSkillFrontmatter {
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

    fn make_rule(id: &str, description: Option<&str>) -> NormalizedSpec {
        NormalizedSpec::Rule(NormalizedRuleSpec {
            path: format!("{id}.md").into(),
            frontmatter: NormalizedRuleFrontmatter {
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
}
