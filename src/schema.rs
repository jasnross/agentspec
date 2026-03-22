use serde_json::Value;

/// Embedded JSON schemas, compiled into the binary via `include_str!()`.
///
/// These are the same schemas used by the TypeScript compiler in
/// `agent-config/spec/schema/`. They are parsed once at startup and reused
/// for all validation calls.
pub const CANONICAL_SCHEMA_STR: &str = include_str!("../schemas/canonical.schema.json");
pub const MODELS_MAPPING_SCHEMA_STR: &str = include_str!("../schemas/models-mapping.schema.json");
pub const TOOLS_MAPPING_SCHEMA_STR: &str = include_str!("../schemas/tools-mapping.schema.json");
pub const FEATURES_MAPPING_SCHEMA_STR: &str =
    include_str!("../schemas/features-mapping.schema.json");

/// Parse all embedded schemas into `serde_json::Value`. Called once at startup.
pub fn load_schemas() -> Schemas {
    Schemas {
        canonical: serde_json::from_str(CANONICAL_SCHEMA_STR)
            .expect("embedded canonical schema is valid JSON"),
        models_mapping: serde_json::from_str(MODELS_MAPPING_SCHEMA_STR)
            .expect("embedded models-mapping schema is valid JSON"),
        tools_mapping: serde_json::from_str(TOOLS_MAPPING_SCHEMA_STR)
            .expect("embedded tools-mapping schema is valid JSON"),
        features_mapping: serde_json::from_str(FEATURES_MAPPING_SCHEMA_STR)
            .expect("embedded features-mapping schema is valid JSON"),
    }
}

/// Pre-parsed JSON schemas for validation.
#[allow(dead_code)] // mapping schemas used in Phase 5
pub struct Schemas {
    pub canonical: Value,
    pub models_mapping: Value,
    pub tools_mapping: Value,
    pub features_mapping: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schemas_parse_successfully() {
        let schemas = load_schemas();
        // All schemas should be objects with a "$schema" key
        assert!(schemas.canonical.is_object());
        assert!(schemas.models_mapping.is_object());
        assert!(schemas.tools_mapping.is_object());
        assert!(schemas.features_mapping.is_object());
        assert_eq!(
            schemas.canonical["$schema"],
            "http://json-schema.org/draft-07/schema#"
        );
    }
}
