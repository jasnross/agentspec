use crate::types::{ModelConfig, PresetsMap, Provider};

/// Resolve the model configuration for a given profile and provider.
///
/// Mirrors the TypeScript `resolveProviderModelConfig`:
/// - String shorthand (e.g., `"opus"`) → `ModelConfig { model: "opus", .. }`
/// - Object form (e.g., `{ model: "...", variant: "max" }`) → extract known fields
/// - Missing profile or provider key → `ModelConfig { model: None, .. }`
pub fn resolve_provider_model_config(
    profile_name: &str,
    provider: Provider,
    profiles: &PresetsMap,
) -> ModelConfig {
    let Some(profile) = profiles.get(profile_name) else {
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
    use std::collections::HashMap;

    use super::*;

    fn make_profiles(profiles: HashMap<String, HashMap<String, serde_json::Value>>) -> PresetsMap {
        profiles
    }

    #[test]
    fn test_string_shorthand() {
        let mut profile = HashMap::new();
        profile.insert("claude".to_string(), serde_json::json!("opus"));

        let profiles = make_profiles(HashMap::from([("deep".to_string(), profile)]));
        let config = resolve_provider_model_config("deep", Provider::Claude, &profiles);

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

        let profiles = make_profiles(HashMap::from([("deep".to_string(), profile)]));
        let config = resolve_provider_model_config("deep", Provider::OpenCode, &profiles);

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

        let profiles = make_profiles(HashMap::from([("deep".to_string(), profile)]));
        let config = resolve_provider_model_config("deep", Provider::Codex, &profiles);

        assert_eq!(config.model.as_deref(), Some("gpt-5.3-codex"));
        assert_eq!(config.reasoning_effort.as_deref(), Some("xhigh"));
    }

    #[test]
    fn test_missing_profile() {
        let profiles = make_profiles(HashMap::new());
        let config = resolve_provider_model_config("nonexistent", Provider::Claude, &profiles);
        assert!(config.model.is_none());
    }

    #[test]
    fn test_missing_provider_in_profile() {
        let mut profile = HashMap::new();
        profile.insert("claude".to_string(), serde_json::json!("opus"));

        let profiles = make_profiles(HashMap::from([("deep".to_string(), profile)]));
        // cursor is not in the profile
        let config = resolve_provider_model_config("deep", Provider::Cursor, &profiles);
        assert!(config.model.is_none());
    }
}
