use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::presets::ProviderPresetsMap;
use crate::spec::{
    AgentSpec, HookSpec, NormalizedAgentFrontmatter, NormalizedAgentSpec,
    NormalizedHookFrontmatter, NormalizedHookSpec, NormalizedRuleFrontmatter, NormalizedRuleSpec,
    NormalizedSkillFrontmatter, NormalizedSkillSpec, NormalizedSpec, RuleSpec, SkillSpec, Spec,
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
pub fn normalize_specs(specs: Vec<Spec>) -> Vec<NormalizedSpec> {
    specs
        .into_iter()
        .map(|spec| match spec {
            Spec::Agent(s) => NormalizedSpec::Agent(normalize_agent_spec(s)),
            Spec::Skill(s) => NormalizedSpec::Skill(normalize_skill_spec(s)),
            Spec::Rule(s) => NormalizedSpec::Rule(normalize_rule_spec(s)),
            Spec::Hook(s) => NormalizedSpec::Hook(normalize_hook_spec(s)),
        })
        .collect()
}

fn normalize_agent_spec(spec: AgentSpec) -> NormalizedAgentSpec {
    let frontmatter = NormalizedAgentFrontmatter {
        id: spec.frontmatter.id,
        description: spec.frontmatter.description,
        tags: spec.frontmatter.tags,
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
        tags: spec.frontmatter.tags,
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
        tags: spec.frontmatter.tags,
    };

    NormalizedRuleSpec {
        path: spec.path,
        frontmatter,
        body: spec.body,
    }
}

fn normalize_hook_spec(spec: HookSpec) -> NormalizedHookSpec {
    let frontmatter = NormalizedHookFrontmatter {
        id: spec.frontmatter.id,
        event: spec.frontmatter.event,
        script: spec.frontmatter.script,
        matcher: spec.frontmatter.matcher,
        timeout: spec.frontmatter.timeout,
        description: spec.frontmatter.description,
        tags: spec.frontmatter.tags,
    };

    NormalizedHookSpec {
        path: spec.path,
        frontmatter,
        body: spec.body,
        supporting_files: spec.supporting_files,
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
                path: spec.path().to_path_buf(),
                message: format!("duplicate id '{}'", spec.id()),
            });
        }

        // Empty body check
        // Hook specs intentionally have empty bodies (they are TOML-driven, not
        // markdown-bodied), so they are exempt from this check.
        if spec.body().is_empty() && !matches!(spec, NormalizedSpec::Hook(_)) {
            errors.push(SemanticError {
                path: spec.path().to_path_buf(),
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

        // Hook event/matcher compatibility: a `matcher` is only valid on the
        // tool-execute events. `HookEvent::allows_matcher` codifies the rule;
        // both providers agree on the answer, so it lives on the enum.
        if let NormalizedSpec::Hook(hook_spec) = spec
            && hook_spec.frontmatter.matcher.is_some()
            && !hook_spec.frontmatter.event.allows_matcher()
        {
            errors.push(SemanticError {
                path: hook_spec.path.clone(),
                message: format!(
                    "hook '{}' uses event '{:?}' which does not accept a `matcher`; \
                     only pre_tool_use, post_tool_use, and post_tool_use_failure may set one",
                    hook_spec.frontmatter.id, hook_spec.frontmatter.event,
                ),
            });
        }

        let execution = match spec {
            NormalizedSpec::Agent(normalized_agent_spec) => {
                &normalized_agent_spec.frontmatter.execution
            }
            NormalizedSpec::Skill(normalized_skill_spec) => {
                &normalized_skill_spec.frontmatter.execution
            }
            NormalizedSpec::Rule(_) | NormalizedSpec::Hook(_) => &None,
        };

        // Preset validation (skip if no presets loaded)
        if let Some(preset_name) = execution.as_ref().and_then(|x| x.preset.as_ref()) {
            match presets.get(preset_name) {
                Some(_) => (),
                None => {
                    errors.push(SemanticError {
                        path: spec.path().to_path_buf(),
                        message: format!("unknown preset '{preset_name}'"),
                    });
                }
            }
        }
    }

    // Per-type underscore-normalization collision check.
    // Keyed template access normalizes hyphens to underscores, so IDs that
    // differ only in hyphen/underscore placement would collide within the same map.
    // Key: (spec_type, normalized_id) → list of (original_id, path).
    let mut normalized_groups: HashMap<(&str, String), Vec<(&str, &Path)>> = HashMap::new();
    for spec in specs {
        let normalized = spec.id().replace('-', "_");
        normalized_groups
            .entry((spec.spec_type(), normalized))
            .or_default()
            .push((spec.id(), spec.path()));
    }
    for ((spec_type, normalized), entries) in &normalized_groups {
        if entries.len() > 1 {
            let names: Vec<&str> = entries.iter().map(|(id, _)| *id).collect();
            // Skip if all entries share the same original ID — that case is
            // already caught by the duplicate-ID check above.
            if names.iter().all(|n| *n == names[0]) {
                continue;
            }
            for (_, path) in entries {
                errors.push(SemanticError {
                    path: path.to_path_buf(),
                    message: format!(
                        "{spec_type} IDs {} all normalize to '{normalized}' \
                         (hyphens \u{2192} underscores) and would collide in template keyed access",
                        names
                            .iter()
                            .map(|n| format!("'{n}'"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
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
        AgentFrontmatter, ExecutionFrontmatter, HookEvent, NormalizedRuleFrontmatter,
        NormalizedSkillFrontmatter, RuleFrontmatter, SkillFrontmatter,
    };

    // -- Helpers --

    fn make_agent(id: &str, body: &str) -> NormalizedSpec {
        NormalizedSpec::Agent(NormalizedAgentSpec {
            path: PathBuf::from(format!("{id}.md")),
            frontmatter: NormalizedAgentFrontmatter {
                id: id.to_string(),
                description: "test description".to_string(),
                tags: None,
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
                tags: None,
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
                tags: None,
            },
            body: body.to_string(),
        })
    }

    fn make_hook(id: &str, event: HookEvent, matcher: Option<&str>) -> NormalizedSpec {
        NormalizedSpec::Hook(NormalizedHookSpec {
            path: PathBuf::from("hooks.toml"),
            frontmatter: NormalizedHookFrontmatter {
                id: id.to_string(),
                event,
                script: PathBuf::from(format!("scripts/{id}.sh")),
                matcher: matcher.map(str::to_string),
                timeout: None,
                description: None,
                tags: None,
            },
            body: String::new(),
            supporting_files: Vec::new(),
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
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: "body text".to_string(),
        });

        let normalized = normalize_specs(vec![spec]);
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
                tags: None,
                user_invocable: true,
                agent_invocable: false,
                execution: None,
                capabilities: None,
            },
            body: "body".to_string(),
            supporting_files: vec![],
        });

        let normalized = normalize_specs(vec![spec]);
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
                tags: None,
            },
            body: "body".to_string(),
        });

        let normalized = normalize_specs(vec![spec]);
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
                tags: None,
                execution: Some(ExecutionFrontmatter {
                    preset: Some("fast".to_string()),
                }),
                capabilities: None,
            },
            body: "body".to_string(),
        });

        let normalized = normalize_specs(vec![spec]);
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

    #[test]
    fn test_normalize_agent_tags() {
        let spec = Spec::Agent(AgentSpec {
            path: PathBuf::from("test.md"),
            frontmatter: AgentFrontmatter {
                id: "tagged".to_string(),
                description: "desc".to_string(),
                tags: Some(vec!["research".to_string(), "codebase".to_string()]),
                execution: None,
                capabilities: None,
            },
            body: "body".to_string(),
        });

        let normalized = normalize_specs(vec![spec]);
        let NormalizedSpec::Agent(ref n) = normalized[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(
            n.frontmatter.tags.as_deref(),
            Some(["research".to_string(), "codebase".to_string()].as_slice())
        );
    }

    #[test]
    fn test_normalize_agent_no_tags() {
        let spec = Spec::Agent(AgentSpec {
            path: PathBuf::from("test.md"),
            frontmatter: AgentFrontmatter {
                id: "untagged".to_string(),
                description: "desc".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: "body".to_string(),
        });

        let normalized = normalize_specs(vec![spec]);
        let NormalizedSpec::Agent(ref n) = normalized[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(n.frontmatter.tags, None);
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

    // -- Underscore-normalization collision tests --

    #[test]
    fn test_semantics_underscore_collision_same_type() {
        let specs = vec![
            make_skill("gh-safe", "body one"),
            make_skill("gh_safe", "body two"),
        ];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new());
        let collision_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.message.contains("normalize"))
            .collect();
        assert_eq!(
            collision_errors.len(),
            2,
            "expected 2 collision errors (one per spec), got: {collision_errors:?}"
        );
        assert!(collision_errors[0].message.contains("'gh-safe'"));
        assert!(collision_errors[0].message.contains("'gh_safe'"));
        assert!(collision_errors[0].message.contains("skill"));
    }

    #[test]
    fn test_semantics_underscore_collision_cross_type_no_error() {
        let specs = vec![
            make_agent("gh-safe", "body one"),
            make_skill("gh_safe", "body two"),
        ];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new());
        let collision_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.message.contains("normalize"))
            .collect();
        assert!(
            collision_errors.is_empty(),
            "cross-type collision should not error, got: {collision_errors:?}"
        );
    }

    #[test]
    fn test_semantics_underscore_no_collision_single_spec() {
        let specs = vec![make_skill("foo-bar", "body")];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new());
        let collision_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.message.contains("normalize"))
            .collect();
        assert!(
            collision_errors.is_empty(),
            "single spec should not collide, got: {collision_errors:?}"
        );
    }

    // -- Hook validation tests --

    #[test]
    fn test_hook_empty_body_does_not_error() {
        // Hooks have empty bodies by design; the empty-body check must skip them.
        let specs = vec![make_hook("init", HookEvent::SessionStart, None)];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new());
        assert!(
            errors.is_empty(),
            "expected no errors for hook with empty body, got: {errors:?}"
        );
    }

    #[test]
    fn test_hook_matcher_on_tool_event_passes() {
        let specs = vec![make_hook("audit", HookEvent::PreToolUse, Some("Bash|Edit"))];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new());
        assert!(
            errors.is_empty(),
            "expected no errors for matcher on pre_tool_use, got: {errors:?}"
        );
    }

    #[test]
    fn test_hook_matcher_on_non_tool_event_errors() {
        let specs = vec![make_hook("init", HookEvent::SessionStart, Some("anything"))];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new());
        let matcher_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.message.contains("does not accept a `matcher`"))
            .collect();
        assert_eq!(
            matcher_errors.len(),
            1,
            "expected exactly one matcher error, got all: {errors:?}"
        );
        assert!(matcher_errors[0].message.contains("'init'"));
    }

    #[test]
    fn test_hook_id_collides_with_skill_id() {
        // The existing duplicate-ID check is global across spec types — a hook
        // with the same ID as an agent/skill/rule errors.
        let specs = vec![
            make_skill("gh-safe", "body"),
            make_hook("gh-safe", HookEvent::SessionStart, None),
        ];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new());
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("duplicate id 'gh-safe'")),
            "expected cross-spec-type duplicate-ID error, got: {errors:?}"
        );
    }

    #[test]
    fn test_hook_duplicate_ids_within_hooks_error() {
        let specs = vec![
            make_hook("init", HookEvent::SessionStart, None),
            make_hook("init", HookEvent::SessionEnd, None),
        ];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new());
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("duplicate id 'init'")),
            "expected duplicate-id error, got: {errors:?}"
        );
    }
}
