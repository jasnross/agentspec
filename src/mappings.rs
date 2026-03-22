use std::path::Path;

use anyhow::{Context, Result};
use jsonschema::{Draft, Validator};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::config::AgentspecConfig;
use crate::schema::Schemas;
use crate::types::{FeaturesMapping, MappingBundle, ModelsMapping, ToolsMapping};

/// Load a YAML file, validate it against a JSON schema, and deserialize.
fn read_yaml_with_schema<T: DeserializeOwned>(path: &Path, schema: &Value) -> Result<T> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;

    // Parse YAML → JSON Value for schema validation
    let yaml_value: serde_yml::Value = serde_yml::from_str(&content)
        .with_context(|| format!("invalid YAML in {}", path.display()))?;
    let json_value: Value = serde_json::to_value(&yaml_value)
        .with_context(|| format!("failed to convert YAML to JSON for {}", path.display()))?;

    // Validate against schema
    let validator = Validator::options()
        .with_draft(Draft::Draft7)
        .build(schema)
        .map_err(|e| anyhow::anyhow!("failed to compile schema: {}", e))?;

    let mut errors = Vec::new();
    for error in validator.iter_errors(&json_value) {
        let at = if error.instance_path.as_str().is_empty() {
            "(root)".to_string()
        } else {
            error.instance_path.to_string()
        };
        errors.push(format!("{}: {} {}", path.display(), at, error));
    }
    if !errors.is_empty() {
        anyhow::bail!(
            "schema validation failed for {}:\n  {}",
            path.display(),
            errors.join("\n  ")
        );
    }

    // Deserialize from the JSON value (already validated)
    let result: T = serde_json::from_value(json_value)
        .with_context(|| format!("failed to deserialize {}", path.display()))?;

    Ok(result)
}

/// Shallow-merge an overlay's model profiles onto a base.
///
/// For each profile in the overlay, each provider key overwrites the base's
/// value for that (profile, provider) pair. Profiles and providers not in
/// the overlay are left unchanged. Profiles in the overlay but not in the
/// base are added.
fn merge_model_mappings(base: &mut ModelsMapping, overlay: ModelsMapping) {
    for (profile_name, overlay_providers) in overlay.profiles {
        let base_providers = base.profiles.entry(profile_name).or_default();
        for (provider, value) in overlay_providers {
            base_providers.insert(provider, value);
        }
    }
}

/// Load all three mapping files and optionally merge a profile overlay.
pub fn load_mappings(
    config: &AgentspecConfig,
    schemas: &Schemas,
    profile: Option<&str>,
) -> Result<MappingBundle> {
    let models_path = config.resolve(&config.mappings.models);
    let tools_path = config.resolve(&config.mappings.tools);
    let features_path = config.resolve(&config.mappings.features);

    let mut models: ModelsMapping = read_yaml_with_schema(&models_path, &schemas.models_mapping)
        .with_context(|| "failed to load models mapping")?;

    let tools: ToolsMapping = read_yaml_with_schema(&tools_path, &schemas.tools_mapping)
        .with_context(|| "failed to load tools mapping")?;

    let features: FeaturesMapping =
        read_yaml_with_schema(&features_path, &schemas.features_mapping)
            .with_context(|| "failed to load features mapping")?;

    // If a profile is specified, load and merge the overlay
    if let Some(profile_name) = profile {
        let overlay_filename = format!("models.{}.yaml", profile_name);
        let overlay_path = models_path
            .parent()
            .context("models mapping path has no parent directory")?
            .join(&overlay_filename);

        if overlay_path.is_file() {
            let overlay: ModelsMapping =
                read_yaml_with_schema(&overlay_path, &schemas.models_mapping).with_context(
                    || format!("failed to load profile overlay {}", overlay_path.display()),
                )?;
            merge_model_mappings(&mut models, overlay);
            eprintln!("loaded profile overlay: {}", overlay_filename);
        } else {
            anyhow::bail!(
                "mapping profile '{}' not found (expected {})",
                profile_name,
                overlay_path.display()
            );
        }
    }

    Ok(MappingBundle {
        models,
        tools,
        features,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_merge_model_mappings_override() {
        let mut base = ModelsMapping {
            profiles: HashMap::from([(
                "fast".to_string(),
                HashMap::from([
                    ("claude".to_string(), serde_json::json!("sonnet")),
                    ("opencode".to_string(), serde_json::json!("oc-fast")),
                ]),
            )]),
        };

        let overlay = ModelsMapping {
            profiles: HashMap::from([(
                "fast".to_string(),
                HashMap::from([(
                    "opencode".to_string(),
                    serde_json::json!({"model": "oc-turbo", "variant": "max"}),
                )]),
            )]),
        };

        merge_model_mappings(&mut base, overlay);

        // claude should be preserved (not in overlay)
        assert_eq!(base.profiles["fast"]["claude"], serde_json::json!("sonnet"));
        // opencode should be overridden
        assert_eq!(
            base.profiles["fast"]["opencode"],
            serde_json::json!({"model": "oc-turbo", "variant": "max"})
        );
    }

    #[test]
    fn test_merge_model_mappings_new_profile() {
        let mut base = ModelsMapping {
            profiles: HashMap::from([(
                "fast".to_string(),
                HashMap::from([("claude".to_string(), serde_json::json!("sonnet"))]),
            )]),
        };

        let overlay = ModelsMapping {
            profiles: HashMap::from([(
                "minimal".to_string(),
                HashMap::from([("claude".to_string(), serde_json::json!("haiku"))]),
            )]),
        };

        merge_model_mappings(&mut base, overlay);

        assert!(base.profiles.contains_key("fast"));
        assert!(base.profiles.contains_key("minimal"));
        assert_eq!(
            base.profiles["minimal"]["claude"],
            serde_json::json!("haiku")
        );
    }

    #[test]
    fn test_read_yaml_with_schema_valid() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            "tools:\n  read:\n    claude: Read\n    opencode: read\n    codex: read\n    cursor: null\n  write:\n    claude: Write\n    opencode: write\n    codex: write\n    cursor: null\n  edit:\n    claude: Edit\n    opencode: edit\n    codex: edit\n    cursor: null\n  grep:\n    claude: Grep\n    opencode: grep\n    codex: grep\n    cursor: null\n  glob:\n    claude: Glob\n    opencode: glob\n    codex: glob\n    cursor: null\n  bash:\n    claude: Bash\n    opencode: bash\n    codex: bash\n    cursor: null\n  webfetch:\n    claude: WebFetch\n    opencode: webfetch\n    codex: webfetch\n    cursor: null\n  websearch:\n    claude: WebSearch\n    opencode: websearch\n    codex: websearch\n    cursor: null\n  task:\n    claude: Task\n    opencode: task\n    codex: task\n    cursor: null\n  todowrite:\n    claude: TodoWrite\n    opencode: todowrite\n    codex: todowrite\n    cursor: null\n  ls:\n    claude: LS\n    opencode: null\n    codex: null\n    cursor: null\n",
        ).unwrap();

        let schema: Value = serde_json::from_str(crate::schema::TOOLS_MAPPING_SCHEMA_STR).unwrap();
        let tools: ToolsMapping = read_yaml_with_schema(tmp.path(), &schema).unwrap();
        assert_eq!(tools.tools.len(), 11);
        assert_eq!(tools.tools["read"]["claude"], Some("Read".to_string()));
        assert_eq!(tools.tools["ls"]["opencode"], None);
    }

    #[test]
    fn test_read_yaml_with_schema_invalid() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "bogus: true\n").unwrap();

        let schema: Value = serde_json::from_str(crate::schema::TOOLS_MAPPING_SCHEMA_STR).unwrap();
        let result = read_yaml_with_schema::<ToolsMapping>(tmp.path(), &schema);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("schema validation failed"),
        );
    }
}
