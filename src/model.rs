use crate::types::{MappingBundle, ModelConfig, Provider};

#[allow(dead_code)] // called by provider adapters in Phase 6
/// Resolve the model configuration for a given profile and provider.
///
/// Mirrors the TypeScript `resolveProviderModelConfig`:
/// - String shorthand (e.g., `"opus"`) → `ModelConfig { model: "opus", .. }`
/// - Object form (e.g., `{ model: "...", variant: "max" }`) → extract known fields
/// - Missing provider key → `ModelConfig { model: None, .. }`
pub fn resolve_provider_model_config(
    profile_name: &str,
    provider: Provider,
    mappings: &MappingBundle,
) -> ModelConfig {
    let Some(profile) = mappings.models.profiles.get(profile_name) else {
        return ModelConfig::default();
    };

    let provider_key = provider.to_string();
    let Some(raw) = profile.get(&provider_key) else {
        return ModelConfig::default();
    };

    // String shorthand: the entire value is the model name
    if let Some(model_str) = raw.as_str() {
        return ModelConfig {
            model: Some(model_str.to_string()),
            ..Default::default()
        };
    }

    // Object form: extract known fields
    ModelConfig {
        model: raw.get("model").and_then(|v| v.as_str()).map(String::from),
        variant: raw
            .get("variant")
            .and_then(|v| v.as_str())
            .map(String::from),
        reasoning_effort: raw
            .get("reasoning_effort")
            .and_then(|v| v.as_str())
            .map(String::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ModelsMapping;
    use std::collections::HashMap;

    fn make_mappings(
        profiles: HashMap<String, HashMap<String, serde_json::Value>>,
    ) -> MappingBundle {
        MappingBundle {
            models: ModelsMapping { profiles },
            ..Default::default()
        }
    }

    #[test]
    fn test_string_shorthand() {
        let mut profile = HashMap::new();
        profile.insert("claude".to_string(), serde_json::json!("opus"));

        let mappings = make_mappings(HashMap::from([("deep".to_string(), profile)]));
        let config = resolve_provider_model_config("deep", Provider::Claude, &mappings);

        assert_eq!(config.model.as_deref(), Some("opus"));
        assert!(config.variant.is_none());
        assert!(config.reasoning_effort.is_none());
    }

    #[test]
    fn test_object_form_with_variant() {
        let mut profile = HashMap::new();
        profile.insert(
            "opencode".to_string(),
            serde_json::json!({"model": "anthropic/claude-opus-4-6", "variant": "max"}),
        );

        let mappings = make_mappings(HashMap::from([("deep".to_string(), profile)]));
        let config = resolve_provider_model_config("deep", Provider::OpenCode, &mappings);

        assert_eq!(config.model.as_deref(), Some("anthropic/claude-opus-4-6"));
        assert_eq!(config.variant.as_deref(), Some("max"));
    }

    #[test]
    fn test_object_form_with_reasoning_effort() {
        let mut profile = HashMap::new();
        profile.insert(
            "codex".to_string(),
            serde_json::json!({"model": "gpt-5.3-codex", "reasoning_effort": "xhigh"}),
        );

        let mappings = make_mappings(HashMap::from([("deep".to_string(), profile)]));
        let config = resolve_provider_model_config("deep", Provider::Codex, &mappings);

        assert_eq!(config.model.as_deref(), Some("gpt-5.3-codex"));
        assert_eq!(config.reasoning_effort.as_deref(), Some("xhigh"));
    }

    #[test]
    fn test_missing_profile() {
        let mappings = make_mappings(HashMap::new());
        let config = resolve_provider_model_config("nonexistent", Provider::Claude, &mappings);
        assert!(config.model.is_none());
    }

    #[test]
    fn test_missing_provider_in_profile() {
        let mut profile = HashMap::new();
        profile.insert("claude".to_string(), serde_json::json!("opus"));

        let mappings = make_mappings(HashMap::from([("deep".to_string(), profile)]));
        // cursor is not in the profile
        let config = resolve_provider_model_config("deep", Provider::Cursor, &mappings);
        assert!(config.model.is_none());
    }
}
