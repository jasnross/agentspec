use serde_json::Value;

/// Embedded JSON schemas, compiled into the binary via `include_str!()`.
///
/// Only the canonical spec schema is embedded; tool/feature/model mapping schemas
/// were removed when mappings were moved into agentspec.toml as `[profiles.*]`.
pub const CANONICAL_SCHEMA_STR: &str = include_str!("../schemas/canonical.schema.json");

/// Parse the embedded canonical schema into `serde_json::Value`. Called once at startup.
pub fn load_schemas() -> Schemas {
    Schemas {
        canonical: serde_json::from_str(CANONICAL_SCHEMA_STR)
            .expect("embedded canonical schema is valid JSON"),
    }
}

/// Pre-parsed JSON schemas for validation.
pub struct Schemas {
    pub canonical: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schemas_parse_successfully() {
        let schemas = load_schemas();
        assert!(schemas.canonical.is_object());
        assert_eq!(
            schemas.canonical["$schema"],
            "http://json-schema.org/draft-07/schema#"
        );
    }
}
