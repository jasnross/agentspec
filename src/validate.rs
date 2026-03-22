use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use anyhow::{Context, Result};
use jsonschema::{Draft, Validator};
use serde_json::Value;

use crate::types::{
    CanonicalSpec, Execution, MappingBundle, NormalizedSpec, Provider, Routing, SkillMeta, SpecKind,
};

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// A JSON schema validation error for a single spec.
#[derive(Debug)]
pub struct SchemaError {
    pub path: PathBuf,
    pub instance_path: String,
    pub message: String,
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}: {}",
            self.path.display(),
            self.instance_path,
            self.message
        )
    }
}

impl std::error::Error for SchemaError {}

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

// ---------------------------------------------------------------------------
// Schema validation
// ---------------------------------------------------------------------------

/// Validate each spec's frontmatter against the canonical JSON schema.
///
/// Returns all validation errors across all specs. An empty vec means all specs
/// pass schema validation. The `Result` wrapper covers structural failures
/// (e.g., corrupt schema) — validation findings are in the `Ok` payload.
pub fn validate_schema(specs: &[CanonicalSpec], schema: &Value) -> Result<Vec<SchemaError>> {
    let validator = Validator::options()
        .with_draft(Draft::Draft7)
        .build(schema)
        .map_err(|e| anyhow::anyhow!("failed to compile canonical schema: {}", e))?;

    let mut errors = Vec::new();

    for spec in specs {
        for error in validator.iter_errors(&spec.fm) {
            errors.push(SchemaError {
                path: spec.path.clone(),
                instance_path: if error.instance_path.as_str().is_empty() {
                    "(root)".to_string()
                } else {
                    error.instance_path.to_string()
                },
                message: error.to_string(),
            });
        }
    }

    Ok(errors)
}

// ---------------------------------------------------------------------------
// Normalization
// ---------------------------------------------------------------------------

/// Normalize canonical specs into `NormalizedSpec` with all defaults applied.
///
/// This mirrors the TypeScript `normalizeSpecs()`:
/// - `name` defaults to `id`
/// - `user_invocable` / `agent_invocable` default to `false`
/// - `tools` are deduplicated and sorted
/// - `targets` defaults to all four providers
/// - Output is sorted by `id`
pub fn normalize_specs(specs: Vec<CanonicalSpec>) -> Result<Vec<NormalizedSpec>> {
    let mut normalized = Vec::with_capacity(specs.len());

    for spec in specs {
        let fm = &spec.fm;

        let id = fm["id"]
            .as_str()
            .context(format!(
                "{}: missing required frontmatter field 'id'",
                spec.path.display()
            ))?
            .to_string();

        let kind = spec.kind;

        let description = fm["description"]
            .as_str()
            .context(format!(
                "{}: missing required frontmatter field 'description'",
                spec.path.display()
            ))?
            .to_string();

        let version = fm["version"].as_i64().context(format!(
            "{}: missing required frontmatter field 'version'",
            spec.path.display()
        ))?;

        let name = fm
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&id)
            .to_string();

        let user_invocable = fm
            .get("user_invocable")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let agent_invocable = fm
            .get("agent_invocable")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Tools: deduplicate via a set, then sort
        let mut tools: Vec<String> = fm
            .get("capabilities")
            .and_then(|c| c.get("tools"))
            .and_then(|t| t.as_array())
            .map(|arr| {
                let mut seen = std::collections::HashSet::new();
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .filter(|s| seen.insert(s.clone()))
                    .collect()
            })
            .unwrap_or_default();
        tools.sort();

        // Targets: from compat.targets or default to all providers
        let targets: Vec<Provider> = fm
            .get("compat")
            .and_then(|c| c.get("targets"))
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| match s {
                        "claude" => Ok(Provider::Claude),
                        "cursor" => Ok(Provider::Cursor),
                        "codex" => Ok(Provider::Codex),
                        "opencode" => Ok(Provider::OpenCode),
                        other => Err(anyhow::anyhow!(
                            "{}: unknown provider '{}' in compat.targets",
                            spec.path.display(),
                            other
                        )),
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_else(|| Provider::ALL.to_vec());

        // Execution
        let exec_obj = fm.get("execution");
        let execution = Execution {
            model_profile: exec_obj
                .and_then(|e| e.get("model_profile"))
                .and_then(|v| v.as_str())
                .map(String::from),
            temperature: exec_obj
                .and_then(|e| e.get("temperature"))
                .and_then(|v| v.as_f64()),
            mode: exec_obj
                .and_then(|e| e.get("mode"))
                .and_then(|v| v.as_str())
                .map(String::from),
            readonly: exec_obj
                .and_then(|e| e.get("readonly"))
                .and_then(|v| v.as_bool()),
            background: exec_obj
                .and_then(|e| e.get("background"))
                .and_then(|v| v.as_bool()),
        };

        // Skill metadata
        let skill = fm.get("skill").map(|s| SkillMeta {
            accepts_args: s.get("accepts_args").and_then(|v| v.as_bool()),
            args_schema: s
                .get("args_schema")
                .and_then(|v| v.as_str())
                .map(String::from),
            delegate_to: s
                .get("delegate_to")
                .and_then(|v| v.as_str())
                .map(String::from),
        });

        // Routing
        let routing = fm.get("routing").map(|r| Routing {
            trigger: r.get("trigger").and_then(|v| v.as_str()).map(String::from),
            aliases: r
                .get("aliases")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        });

        // Provider overrides
        let provider_overrides: HashMap<String, serde_json::Value> = fm
            .get("provider_overrides")
            .and_then(|v| v.as_object())
            .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();

        normalized.push(NormalizedSpec {
            source_path: spec.path,
            id,
            kind,
            name,
            description,
            version,
            user_invocable,
            agent_invocable,
            body: spec.body.trim().to_string(),
            execution,
            tools,
            skill,
            supporting_files: spec.supporting_files,
            targets,
            provider_overrides,
            routing,
        });
    }

    normalized.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(normalized)
}

// ---------------------------------------------------------------------------
// Semantic validation
// ---------------------------------------------------------------------------

/// Run semantic validation checks on normalized specs.
///
/// Returns all errors found. An empty vec means all checks pass.
/// This function does no I/O and cannot fail structurally.
pub fn validate_semantics(
    specs: &[NormalizedSpec],
    mappings: &MappingBundle,
) -> Vec<SemanticError> {
    let mut errors = Vec::new();
    let mut id_set = std::collections::HashSet::new();

    for spec in specs {
        // Duplicate ID check
        if !id_set.insert(&spec.id) {
            errors.push(SemanticError {
                path: spec.source_path.clone(),
                message: format!("duplicate id '{}'", spec.id),
            });
        }

        // Empty body check
        if spec.body.is_empty() {
            errors.push(SemanticError {
                path: spec.source_path.clone(),
                message: "instruction body cannot be empty".to_string(),
            });
        }

        // Skill invocability check
        if spec.kind == SpecKind::Skill && !spec.user_invocable && !spec.agent_invocable {
            errors.push(SemanticError {
                path: spec.source_path.clone(),
                message: "at least one of user_invocable or agent_invocable must be true"
                    .to_string(),
            });
        }

        // delegate_to reference check
        if let Some(ref skill) = spec.skill
            && let Some(ref delegate_to) = skill.delegate_to
            && !specs.iter().any(|s| s.id == *delegate_to)
        {
            errors.push(SemanticError {
                path: spec.source_path.clone(),
                message: format!(
                    "skill.delegate_to '{}' does not match any canonical id",
                    delegate_to
                ),
            });
        }

        // Model profile validation (skip if no mappings loaded yet — Phase 5)
        if let Some(ref profile) = spec.execution.model_profile
            && !mappings.models.profiles.is_empty()
        {
            match mappings.models.profiles.get(profile) {
                None => {
                    errors.push(SemanticError {
                        path: spec.source_path.clone(),
                        message: format!("unknown model profile '{}'", profile),
                    });
                }
                Some(profile_mapping) => {
                    for provider in &spec.targets {
                        let provider_key = provider.to_string();
                        let has_model = profile_mapping
                            .get(&provider_key)
                            .map(|v| {
                                // String shorthand or object with "model" key
                                v.is_string()
                                    || v.get("model")
                                        .and_then(|m| m.as_str())
                                        .is_some_and(|s| !s.is_empty())
                            })
                            .unwrap_or(false);

                        if !has_model {
                            errors.push(SemanticError {
                                path: spec.source_path.clone(),
                                message: format!(
                                    "model profile '{}' missing mapping for provider '{}'",
                                    profile, provider
                                ),
                            });
                        }
                    }
                }
            }
        }
    }

    errors
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;

    fn make_spec(fm_json: &str, body: &str) -> CanonicalSpec {
        CanonicalSpec {
            path: PathBuf::from("test.md"),
            fm: serde_json::from_str(fm_json).unwrap(),
            body: body.to_string(),
            kind: SpecKind::Skill,
            supporting_files: vec![],
        }
    }

    fn make_valid_fm() -> String {
        r#"{"id":"test-skill","description":"A test","version":1}"#.to_string()
    }

    // -- Schema validation tests --

    #[test]
    fn test_validate_schema_valid_spec() {
        let schemas = schema::load_schemas();
        let spec = make_spec(&make_valid_fm(), "body text");
        let errors = validate_schema(&[spec], &schemas.canonical).unwrap();
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn test_validate_schema_missing_id() {
        let schemas = schema::load_schemas();
        let spec = make_spec(r#"{"description":"A test","version":1}"#, "body");
        let errors = validate_schema(&[spec], &schemas.canonical).unwrap();
        assert!(!errors.is_empty());
        assert!(
            errors.iter().any(|e| e.message.contains("id")),
            "expected error about missing id, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_schema_invalid_id_pattern() {
        let schemas = schema::load_schemas();
        let spec = make_spec(
            r#"{"id":"Invalid_ID","description":"A test","version":1}"#,
            "body",
        );
        let errors = validate_schema(&[spec], &schemas.canonical).unwrap();
        assert!(!errors.is_empty());
    }

    #[test]
    fn test_validate_schema_missing_version() {
        let schemas = schema::load_schemas();
        let spec = make_spec(r#"{"id":"test","description":"A test"}"#, "body");
        let errors = validate_schema(&[spec], &schemas.canonical).unwrap();
        assert!(!errors.is_empty());
    }

    #[test]
    fn test_validate_schema_extra_field() {
        let schemas = schema::load_schemas();
        let spec = make_spec(
            r#"{"id":"test","description":"A test","version":1,"bogus":"field"}"#,
            "body",
        );
        let errors = validate_schema(&[spec], &schemas.canonical).unwrap();
        assert!(
            !errors.is_empty(),
            "additionalProperties should be rejected"
        );
    }

    #[test]
    fn test_validate_schema_error_includes_path() {
        let schemas = schema::load_schemas();
        let spec = CanonicalSpec {
            path: PathBuf::from("/specs/my-skill.md"),
            fm: serde_json::json!({"description": "no id or version"}),
            body: "body".to_string(),
            kind: SpecKind::Skill,
            supporting_files: vec![],
        };
        let errors = validate_schema(&[spec], &schemas.canonical).unwrap();
        assert!(
            errors
                .iter()
                .any(|e| e.path.to_string_lossy().contains("my-skill"))
        );
    }

    // -- Normalization tests --

    #[test]
    fn test_normalize_defaults() {
        let spec = CanonicalSpec {
            path: PathBuf::from("test.md"),
            fm: serde_json::json!({
                "id": "my-skill",
                "description": "A skill",
                "version": 1
            }),
            body: "  body text  ".to_string(),
            kind: SpecKind::Skill,
            supporting_files: vec![],
        };

        let normalized = normalize_specs(vec![spec]).unwrap();
        assert_eq!(normalized.len(), 1);
        let n = &normalized[0];
        assert_eq!(n.name, "my-skill"); // name defaults to id
        assert!(!n.user_invocable);
        assert!(!n.agent_invocable);
        assert_eq!(n.body, "body text"); // trimmed
        assert_eq!(n.targets.len(), 4); // all providers
        assert!(n.tools.is_empty());
    }

    #[test]
    fn test_normalize_explicit_name() {
        let spec = make_spec(
            r#"{"id":"test","name":"My Test","description":"desc","version":1}"#,
            "body",
        );
        let normalized = normalize_specs(vec![spec]).unwrap();
        assert_eq!(normalized[0].name, "My Test");
    }

    #[test]
    fn test_normalize_tools_dedup_and_sort() {
        let spec = make_spec(
            r#"{"id":"test","description":"desc","version":1,"capabilities":{"tools":["grep","bash","grep","edit"]}}"#,
            "body",
        );
        let normalized = normalize_specs(vec![spec]).unwrap();
        assert_eq!(normalized[0].tools, vec!["bash", "edit", "grep"]);
    }

    #[test]
    fn test_normalize_custom_targets() {
        let spec = make_spec(
            r#"{"id":"test","description":"desc","version":1,"compat":{"targets":["claude","cursor"]}}"#,
            "body",
        );
        let normalized = normalize_specs(vec![spec]).unwrap();
        assert_eq!(normalized[0].targets.len(), 2);
        assert_eq!(normalized[0].targets[0], Provider::Claude);
        assert_eq!(normalized[0].targets[1], Provider::Cursor);
    }

    #[test]
    fn test_normalize_sorted_by_id() {
        let spec_b = make_spec(
            r#"{"id":"beta","description":"desc","version":1}"#,
            "body b",
        );
        let spec_a = make_spec(
            r#"{"id":"alpha","description":"desc","version":1}"#,
            "body a",
        );
        let normalized = normalize_specs(vec![spec_b, spec_a]).unwrap();
        assert_eq!(normalized[0].id, "alpha");
        assert_eq!(normalized[1].id, "beta");
    }

    #[test]
    fn test_normalize_execution_fields() {
        let spec = make_spec(
            r#"{"id":"test","description":"desc","version":1,"execution":{"model_profile":"fast","temperature":0.5,"mode":"subagent","readonly":true,"background":false}}"#,
            "body",
        );
        let normalized = normalize_specs(vec![spec]).unwrap();
        let exec = &normalized[0].execution;
        assert_eq!(exec.model_profile.as_deref(), Some("fast"));
        assert_eq!(exec.temperature, Some(0.5));
        assert_eq!(exec.mode.as_deref(), Some("subagent"));
        assert_eq!(exec.readonly, Some(true));
        assert_eq!(exec.background, Some(false));
    }

    #[test]
    fn test_normalize_skill_metadata() {
        let spec = make_spec(
            r#"{"id":"test","description":"desc","version":1,"skill":{"accepts_args":true,"delegate_to":"other-skill"}}"#,
            "body",
        );
        let normalized = normalize_specs(vec![spec]).unwrap();
        let skill = normalized[0].skill.as_ref().unwrap();
        assert_eq!(skill.accepts_args, Some(true));
        assert_eq!(skill.delegate_to.as_deref(), Some("other-skill"));
    }

    // -- Semantic validation tests --

    fn make_normalized(id: &str, kind: SpecKind, body: &str) -> NormalizedSpec {
        NormalizedSpec {
            source_path: PathBuf::from(format!("{}.md", id)),
            id: id.to_string(),
            kind,
            name: id.to_string(),
            description: "test".to_string(),
            version: 1,
            user_invocable: kind == SpecKind::Skill,
            agent_invocable: false,
            body: body.to_string(),
            execution: Execution::default(),
            tools: vec![],
            skill: None,
            supporting_files: vec![],
            targets: Provider::ALL.to_vec(),
            provider_overrides: HashMap::new(),
            routing: None,
        }
    }

    #[test]
    fn test_semantics_clean() {
        let specs = vec![
            make_normalized("alpha", SpecKind::Agent, "body"),
            make_normalized("beta", SpecKind::Skill, "body"),
        ];
        let mappings = MappingBundle::default();
        let errors = validate_semantics(&specs, &mappings);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn test_semantics_duplicate_id() {
        let specs = vec![
            make_normalized("dup", SpecKind::Agent, "body a"),
            make_normalized("dup", SpecKind::Agent, "body b"),
        ];
        let mappings = MappingBundle::default();
        let errors = validate_semantics(&specs, &mappings);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("duplicate id 'dup'"));
    }

    #[test]
    fn test_semantics_empty_body() {
        let specs = vec![make_normalized("empty", SpecKind::Agent, "")];
        let mappings = MappingBundle::default();
        let errors = validate_semantics(&specs, &mappings);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("body cannot be empty"))
        );
    }

    #[test]
    fn test_semantics_skill_not_invocable() {
        let mut spec = make_normalized("no-invoke", SpecKind::Skill, "body");
        spec.user_invocable = false;
        spec.agent_invocable = false;
        let mappings = MappingBundle::default();
        let errors = validate_semantics(&[spec], &mappings);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("user_invocable or agent_invocable"))
        );
    }

    #[test]
    fn test_semantics_delegate_to_missing() {
        let mut spec = make_normalized("delegator", SpecKind::Skill, "body");
        spec.skill = Some(SkillMeta {
            delegate_to: Some("nonexistent".to_string()),
            ..Default::default()
        });
        let mappings = MappingBundle::default();
        let errors = validate_semantics(&[spec], &mappings);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("delegate_to 'nonexistent'"))
        );
    }

    #[test]
    fn test_semantics_delegate_to_valid() {
        let target = make_normalized("target-skill", SpecKind::Skill, "body");
        let mut delegator = make_normalized("delegator", SpecKind::Skill, "body");
        delegator.skill = Some(SkillMeta {
            delegate_to: Some("target-skill".to_string()),
            ..Default::default()
        });
        let mappings = MappingBundle::default();
        let errors = validate_semantics(&[delegator, target], &mappings);
        assert!(
            errors.is_empty(),
            "valid delegate_to should not error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_semantics_unknown_model_profile() {
        let mut spec = make_normalized("profiled", SpecKind::Agent, "body");
        spec.execution.model_profile = Some("nonexistent".to_string());

        // Need at least one profile so model_profile validation isn't skipped
        let mut profiles = HashMap::new();
        profiles.insert("known".to_string(), HashMap::new());
        let mappings = MappingBundle {
            models: crate::types::ModelsMapping { profiles },
            ..Default::default()
        };
        let errors = validate_semantics(&[spec], &mappings);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("unknown model profile 'nonexistent'"))
        );
    }

    #[test]
    fn test_semantics_model_profile_missing_provider() {
        let mut spec = make_normalized("profiled", SpecKind::Agent, "body");
        spec.execution.model_profile = Some("fast".to_string());

        // Only claude has a mapping, other 3 providers are missing
        let mut profiles = HashMap::new();
        let mut fast_profile = HashMap::new();
        fast_profile.insert("claude".to_string(), serde_json::json!("claude-sonnet"));
        profiles.insert("fast".to_string(), fast_profile);

        let mappings = MappingBundle {
            models: crate::types::ModelsMapping { profiles },
            ..Default::default()
        };
        let errors = validate_semantics(&[spec], &mappings);
        // Should have errors for cursor, codex, opencode
        assert_eq!(
            errors.len(),
            3,
            "expected 3 missing provider errors, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_semantics_model_profile_all_providers_mapped() {
        let mut spec = make_normalized("profiled", SpecKind::Agent, "body");
        spec.execution.model_profile = Some("fast".to_string());

        let mut fast_profile = HashMap::new();
        fast_profile.insert("claude".to_string(), serde_json::json!("claude-sonnet"));
        fast_profile.insert("cursor".to_string(), serde_json::json!("cursor-fast"));
        fast_profile.insert("codex".to_string(), serde_json::json!("codex-mini"));
        fast_profile.insert(
            "opencode".to_string(),
            serde_json::json!({"model": "oc-fast"}),
        );

        let mut profiles = HashMap::new();
        profiles.insert("fast".to_string(), fast_profile);

        let mappings = MappingBundle {
            models: crate::types::ModelsMapping { profiles },
            ..Default::default()
        };
        let errors = validate_semantics(&[spec], &mappings);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }
}
