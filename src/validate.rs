use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::presets::ProviderPresetsMap;
use crate::spec::Spec;

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

/// Run semantic validation checks on loaded specs.
///
/// Returns all errors found. An empty vec means all checks pass.
/// This function does no I/O and cannot fail structurally.
pub fn validate_semantics(specs: &[Spec], presets: &ProviderPresetsMap) -> Vec<SemanticError> {
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
        if spec.body().is_empty() && !matches!(spec, Spec::Hook(_)) {
            errors.push(SemanticError {
                path: spec.path().to_path_buf(),
                message: "instruction body cannot be empty".to_string(),
            });
        }

        // Skill invocability check
        if let Spec::Skill(skill_spec) = spec
            && !skill_spec.frontmatter.user_invocable
            && !skill_spec.frontmatter.agent_invocable
        {
            errors.push(SemanticError {
                path: skill_spec.path.clone(),
                message: "at least one of user_invocable or agent_invocable must be true"
                    .to_string(),
            });
        }

        if let Spec::Hook(hook_spec) = spec
            && hook_spec.frontmatter.matcher.is_some()
        {
            let bad_events: Vec<_> = hook_spec
                .frontmatter
                .events
                .iter()
                .filter(|e| !e.allows_matcher())
                .collect();
            if !bad_events.is_empty() {
                errors.push(SemanticError {
                    path: hook_spec.path.clone(),
                    message: format!(
                        "hook '{}' sets `matcher` but targets event(s) that do not accept one: {}; \
                         only pre_tool_use, post_tool_use, and post_tool_use_failure may use a matcher",
                        hook_spec.frontmatter.id,
                        bad_events
                            .iter()
                            .map(|e| e.snake_case())
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                });
            }
        }

        let execution = match spec {
            Spec::Agent(agent_spec) => &agent_spec.frontmatter.execution,
            Spec::Skill(skill_spec) => &skill_spec.frontmatter.execution,
            Spec::Rule(_) | Spec::Hook(_) => &None,
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

    use indexmap::IndexMap;

    use super::*;
    use crate::presets::{ProviderPresets, ProviderPresetsMap};
    use crate::spec::{
        AgentFrontmatter, AgentSpec, ExecutionFrontmatter, HookEvent, HookFrontmatter, HookSpec,
        RuleFrontmatter, RuleSpec, SkillFrontmatter, SkillSpec,
    };

    // -- Helpers --

    fn make_agent(id: &str, body: &str) -> Spec {
        Spec::Agent(AgentSpec {
            path: PathBuf::from(format!("{id}.md")),
            frontmatter: AgentFrontmatter {
                id: id.to_string(),
                description: "test description".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: body.to_string(),
        })
    }

    fn make_skill(id: &str, body: &str) -> Spec {
        Spec::Skill(SkillSpec {
            path: PathBuf::from(format!("{id}.md")),
            frontmatter: SkillFrontmatter {
                id: id.to_string(),
                description: None,
                tags: None,
                user_invocable: true,
                agent_invocable: false,
                execution: None,
                capabilities: None,
            },
            body: body.to_string(),
            supporting_files: IndexMap::new(),
        })
    }

    fn make_rule(id: &str, body: &str) -> Spec {
        Spec::Rule(RuleSpec {
            path: PathBuf::from(format!("{id}.md")),
            frontmatter: RuleFrontmatter {
                id: id.to_string(),
                description: None,
                tags: None,
            },
            body: body.to_string(),
        })
    }

    fn make_hook(id: &str, events: Vec<HookEvent>, matcher: Option<&str>) -> Spec {
        Spec::Hook(HookSpec {
            path: PathBuf::from("hooks.toml"),
            frontmatter: HookFrontmatter {
                id: id.to_string(),
                events,
                script: PathBuf::from(format!("scripts/{id}.sh")),
                matcher: matcher.map(str::to_string),
                timeout: None,
                description: None,
                tags: None,
            },
            body: String::new(),
            supporting_files: IndexMap::new(),
        })
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
        let Spec::Skill(ref mut s) = spec else {
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
        let Spec::Agent(ref mut s) = spec else {
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
        let Spec::Agent(ref mut s) = spec else {
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
        let specs = vec![make_hook("init", vec![HookEvent::SessionStart], None)];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new());
        assert!(
            errors.is_empty(),
            "expected no errors for hook with empty body, got: {errors:?}"
        );
    }

    #[test]
    fn test_hook_matcher_on_tool_event_passes() {
        let specs = vec![make_hook(
            "audit",
            vec![HookEvent::PreToolUse],
            Some("Bash|Edit"),
        )];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new());
        assert!(
            errors.is_empty(),
            "expected no errors for matcher on pre_tool_use, got: {errors:?}"
        );
    }

    #[test]
    fn test_hook_matcher_on_non_tool_event_errors() {
        let specs = vec![make_hook(
            "init",
            vec![HookEvent::SessionStart],
            Some("anything"),
        )];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new());
        let matcher_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.message.contains("do not accept one"))
            .collect();
        assert_eq!(
            matcher_errors.len(),
            1,
            "expected exactly one matcher error, got all: {errors:?}"
        );
        assert!(matcher_errors[0].message.contains("'init'"));
        assert!(matcher_errors[0].message.contains("session_start"));
    }

    #[test]
    fn test_hook_matcher_on_mixed_events_errors() {
        let specs = vec![make_hook(
            "mixed",
            vec![HookEvent::PreToolUse, HookEvent::SessionStart],
            Some("Edit"),
        )];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new());
        let matcher_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.message.contains("do not accept one"))
            .collect();
        assert_eq!(
            matcher_errors.len(),
            1,
            "expected one error for mixed events with matcher, got: {errors:?}"
        );
        assert!(matcher_errors[0].message.contains("session_start"));
        assert!(
            matcher_errors[0]
                .message
                .starts_with("hook 'mixed' sets `matcher` but targets event(s) that do not accept one: session_start"),
            "error should list only the offending event, got: {}",
            matcher_errors[0].message
        );
    }

    #[test]
    fn test_hook_matcher_on_all_tool_events_passes() {
        let specs = vec![make_hook(
            "audit",
            vec![HookEvent::PreToolUse, HookEvent::PostToolUse],
            Some("Bash"),
        )];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new());
        assert!(
            errors.is_empty(),
            "expected no errors for matcher on all-tool-events, got: {errors:?}"
        );
    }

    #[test]
    fn test_hook_id_collides_with_skill_id() {
        let specs = vec![
            make_skill("gh-safe", "body"),
            make_hook("gh-safe", vec![HookEvent::SessionStart], None),
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
            make_hook("init", vec![HookEvent::SessionStart], None),
            make_hook("init", vec![HookEvent::SessionEnd], None),
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
