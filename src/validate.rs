use std::fmt;
use std::path::PathBuf;

use anyhow::Result;

use crate::presets::ProviderPresetsMap;
use crate::spec::{
    AgentSpec, NormalizedAgentFrontmatter, NormalizedAgentSpec, NormalizedRuleFrontmatter,
    NormalizedRuleSpec, NormalizedSkillFrontmatter, NormalizedSkillSpec, NormalizedSpec, RuleSpec,
    SkillSpec, Spec,
};

/// A semantic validation error.
#[derive(Debug)]
pub struct SemanticError {
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for SemanticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for SemanticError {}

// TODO: move normalization into its own module
pub fn normalize_specs(specs: Vec<Spec>) -> Result<Vec<NormalizedSpec>> {
    let mut results = Vec::with_capacity(specs.len());

    for spec in specs {
        results.push(match spec {
            Spec::Agent(agent_spec) => NormalizedSpec::Agent(normalize_agent_spec(agent_spec)),
            Spec::Skill(skill_spec) => NormalizedSpec::Skill(normalize_skill_spec(skill_spec)),
            Spec::Rule(rule_spec) => NormalizedSpec::Rule(normalize_rule_spec(rule_spec)),
        });
    }

    Ok(results)
}

fn normalize_agent_spec(spec: AgentSpec) -> NormalizedAgentSpec {
    let frontmatter = NormalizedAgentFrontmatter {
        id: spec.frontmatter.id,
        description: spec.frontmatter.description,
        execution: spec.frontmatter.execution,
        capabilities: spec.frontmatter.capabilities,
    };

    NormalizedAgentSpec {
        path: spec.path,
        frontmatter,
        body: spec.body,
    }
}

fn normalize_skill_spec(spec: SkillSpec) -> NormalizedSkillSpec {
    let frontmatter = NormalizedSkillFrontmatter {
        id: spec.frontmatter.id,
        description: spec.frontmatter.description,
        user_invocable: spec.frontmatter.user_invocable,
        agent_invocable: spec.frontmatter.agent_invocable,
        execution: spec.frontmatter.execution,
        capabilities: spec.frontmatter.capabilities,
    };

    NormalizedSkillSpec {
        path: spec.path,
        frontmatter,
        body: spec.body,
        supporting_files: spec.supporting_files,
    }
}

fn normalize_rule_spec(spec: RuleSpec) -> NormalizedRuleSpec {
    let frontmatter = NormalizedRuleFrontmatter {
        id: spec.frontmatter.id,
        description: spec.frontmatter.description,
    };

    NormalizedRuleSpec {
        path: spec.path,
        frontmatter,
        body: spec.body,
    }
}

/// Run semantic validation checks on normalized specs.
///
/// Returns all errors found. An empty vec means all checks pass.
/// This function does no I/O and cannot fail structurally.
pub fn validate_semantics(
    specs: &[NormalizedSpec],
    presets: &ProviderPresetsMap,
) -> Vec<SemanticError> {
    let mut errors = Vec::new();
    let mut id_set = std::collections::HashSet::new();

    for spec in specs {
        // Duplicate ID check
        if !id_set.insert(spec.id()) {
            errors.push(SemanticError {
                path: spec.path().clone(),
                message: format!("duplicate id '{}'", spec.id()),
            });
        }

        // Empty body check
        if spec.body().is_empty() {
            errors.push(SemanticError {
                path: spec.path().clone(),
                message: "instruction body cannot be empty".to_string(),
            });
        }

        // Skill invocability check
        if let NormalizedSpec::Skill(skill_spec) = spec
            && !skill_spec.frontmatter.user_invocable
            && !skill_spec.frontmatter.agent_invocable
        {
            errors.push(SemanticError {
                path: skill_spec.path.clone(),
                message: "at least one of user_invocable or agent_invocable must be true"
                    .to_string(),
            });
        }

        let execution = match spec {
            NormalizedSpec::Agent(normalized_agent_spec) => {
                &normalized_agent_spec.frontmatter.execution
            }
            NormalizedSpec::Skill(normalized_skill_spec) => {
                &normalized_skill_spec.frontmatter.execution
            }
            NormalizedSpec::Rule(_) => &None,
        };

        // Preset validation (skip if no presets loaded)
        if let Some(preset_name) = execution.as_ref().and_then(|x| x.preset.as_ref()) {
            match presets.get(preset_name) {
                Some(_) => (),
                None => {
                    errors.push(SemanticError {
                        path: spec.path().clone(),
                        message: format!("unknown preset '{preset_name}'"),
                    });
                }
            }
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::presets::{ProviderPresets, ProviderPresetsMap};
    use crate::spec::{
        AgentFrontmatter, ExecutionFrontmatter, NormalizedRuleFrontmatter,
        NormalizedSkillFrontmatter, RuleFrontmatter, SkillFrontmatter,
    };

    // -- Helpers --

    fn make_agent(id: &str, body: &str) -> NormalizedSpec {
        NormalizedSpec::Agent(NormalizedAgentSpec {
            path: PathBuf::from(format!("{id}.md")),
            frontmatter: NormalizedAgentFrontmatter {
                id: id.to_string(),
                description: "test description".to_string(),
                execution: None,
                capabilities: None,
            },
            body: body.to_string(),
        })
    }

    fn make_skill(id: &str, body: &str) -> NormalizedSpec {
        NormalizedSpec::Skill(NormalizedSkillSpec {
            path: PathBuf::from(format!("{id}.md")),
            frontmatter: NormalizedSkillFrontmatter {
                id: id.to_string(),
                description: None,
                user_invocable: true,
                agent_invocable: false,
                execution: None,
                capabilities: None,
            },
            body: body.to_string(),
            supporting_files: vec![],
        })
    }

    fn make_rule(id: &str, body: &str) -> NormalizedSpec {
        NormalizedSpec::Rule(NormalizedRuleSpec {
            path: PathBuf::from(format!("{id}.md")),
            frontmatter: NormalizedRuleFrontmatter {
                id: id.to_string(),
                description: None,
            },
            body: body.to_string(),
        })
    }

    // -- Normalization tests --

    #[test]
    fn test_normalize_agent() {
        let spec = Spec::Agent(AgentSpec {
            path: PathBuf::from("test.md"),
            frontmatter: AgentFrontmatter {
                id: "my-agent".to_string(),
                description: "An agent".to_string(),
                execution: None,
                capabilities: None,
            },
            body: "body text".to_string(),
        });

        let normalized = normalize_specs(vec![spec]).expect("expected value");
        assert_eq!(normalized.len(), 1);
        let NormalizedSpec::Agent(ref n) = normalized[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(n.frontmatter.id, "my-agent");
        assert_eq!(n.frontmatter.description, "An agent");
        assert_eq!(n.body, "body text");
    }

    #[test]
    fn test_normalize_skill() {
        let spec = Spec::Skill(SkillSpec {
            path: PathBuf::from("test.md"),
            frontmatter: SkillFrontmatter {
                id: "my-skill".to_string(),
                description: Some("A skill".to_string()),
                user_invocable: true,
                agent_invocable: false,
                execution: None,
                capabilities: None,
            },
            body: "body".to_string(),
            supporting_files: vec![],
        });

        let normalized = normalize_specs(vec![spec]).expect("expected value");
        let NormalizedSpec::Skill(ref n) = normalized[0] else {
            panic!("expected Skill variant")
        };
        assert_eq!(n.frontmatter.id, "my-skill");
        assert_eq!(n.frontmatter.description.as_deref(), Some("A skill"));
        assert!(n.frontmatter.user_invocable);
        assert!(!n.frontmatter.agent_invocable);
    }

    #[test]
    fn test_normalize_rule() {
        let spec = Spec::Rule(RuleSpec {
            path: PathBuf::from("test.md"),
            frontmatter: RuleFrontmatter {
                id: "my-rule".to_string(),
                description: None,
            },
            body: "body".to_string(),
        });

        let normalized = normalize_specs(vec![spec]).expect("expected value");
        let NormalizedSpec::Rule(ref n) = normalized[0] else {
            panic!("expected Rule variant")
        };
        assert_eq!(n.frontmatter.id, "my-rule");
        assert_eq!(n.frontmatter.description, None);
    }

    #[test]
    fn test_normalize_execution_preset() {
        let spec = Spec::Agent(AgentSpec {
            path: PathBuf::from("test.md"),
            frontmatter: AgentFrontmatter {
                id: "test".to_string(),
                description: "desc".to_string(),
                execution: Some(ExecutionFrontmatter {
                    preset: Some("fast".to_string()),
                }),
                capabilities: None,
            },
            body: "body".to_string(),
        });

        let normalized = normalize_specs(vec![spec]).expect("expected value");
        let NormalizedSpec::Agent(ref n) = normalized[0] else {
            panic!("expected Agent variant")
        };
        let exec = n
            .frontmatter
            .execution
            .as_ref()
            .expect("expected execution");
        assert_eq!(exec.preset.as_deref(), Some("fast"));
    }

    // -- Semantic validation tests --

    #[test]
    fn test_semantics_clean() {
        let specs = vec![make_agent("alpha", "body"), make_skill("beta", "body")];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new());
        assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
    }

    #[test]
    fn test_semantics_duplicate_id() {
        let specs = vec![make_agent("dup", "body a"), make_agent("dup", "body b")];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("duplicate id 'dup'"));
    }

    #[test]
    fn test_semantics_empty_body() {
        let specs = vec![make_agent("empty", "")];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new());
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("body cannot be empty"))
        );
    }

    #[test]
    fn test_semantics_skill_not_invocable() {
        let mut spec = make_skill("no-invoke", "body");
        let NormalizedSpec::Skill(ref mut s) = spec else {
            panic!("expected Skill variant")
        };
        s.frontmatter.user_invocable = false;
        s.frontmatter.agent_invocable = false;
        let errors = validate_semantics(&[spec], &ProviderPresetsMap::new());
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("user_invocable or agent_invocable"))
        );
    }

    #[test]
    fn test_semantics_unknown_preset() {
        let mut spec = make_agent("unknown", "body");
        let NormalizedSpec::Agent(ref mut s) = spec else {
            panic!("expected Agent variant")
        };
        s.frontmatter.execution = Some(ExecutionFrontmatter {
            preset: Some("nonexistent".to_string()),
        });
        let mut presets = ProviderPresetsMap::new();
        presets.insert("known".to_string(), ProviderPresets::default());
        let errors = validate_semantics(&[spec], &presets);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("unknown preset 'nonexistent'"))
        );
    }

    #[test]
    fn test_semantics_known_preset_no_error() {
        let mut spec = make_agent("known", "body");
        let NormalizedSpec::Agent(ref mut s) = spec else {
            panic!("expected Agent variant")
        };
        s.frontmatter.execution = Some(ExecutionFrontmatter {
            preset: Some("fast".to_string()),
        });
        let mut presets = ProviderPresetsMap::new();
        presets.insert("fast".to_string(), ProviderPresets::default());
        let errors = validate_semantics(&[spec], &presets);
        assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
    }

    #[test]
    fn test_semantics_rule_passes_all_checks() {
        let specs = vec![make_rule("my-rule", "body")];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new());
        assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
    }
}
