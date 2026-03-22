use std::path::Path;

use indexmap::IndexMap;
use serde_json::Value;

use crate::format::render_markdown_with_frontmatter;
use crate::model::resolve_provider_model_config;
use crate::types::{CompileWarning, GeneratedFile, NormalizedSpec, ProfilesMap, Provider};

pub fn adapt_codex(
    spec: &NormalizedSpec,
    profiles: &ProfilesMap,
) -> (Vec<GeneratedFile>, Vec<CompileWarning>) {
    let warnings = Vec::new();

    let resolved_model = spec
        .execution
        .model_profile
        .as_ref()
        .map(|profile| resolve_provider_model_config(profile, Provider::Codex, profiles))
        .unwrap_or_default();

    let mut fm = IndexMap::new();
    fm.insert("name".to_string(), Value::String(spec.id.clone()));
    fm.insert(
        "description".to_string(),
        Value::String(spec.description.clone()),
    );

    if let Some(model) = resolved_model.model {
        fm.insert("model".to_string(), Value::String(model));
    }

    if let Some(reasoning_effort) = resolved_model.reasoning_effort {
        fm.insert(
            "model_reasoning_effort".to_string(),
            Value::String(reasoning_effort),
        );
    }

    let skill_dir = Path::new("generated")
        .join("codex")
        .join("skills")
        .join(&spec.id);

    let mut files = vec![GeneratedFile::text(
        Provider::Codex,
        skill_dir.join("SKILL.md"),
        render_markdown_with_frontmatter(&fm, &spec.body),
    )];

    for sf in &spec.supporting_files {
        files.push(GeneratedFile::binary(
            Provider::Codex,
            skill_dir.join(&sf.relative_path),
            sf.content.clone(),
            if sf.executable { Some(0o755) } else { None },
        ));
    }

    (files, warnings)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::types::{Execution, SpecKind};

    fn test_spec() -> NormalizedSpec {
        NormalizedSpec {
            source_path: "/test/spec.md".into(),
            id: "commit".to_string(),
            kind: SpecKind::Skill,
            name: "commit".to_string(),
            description: "Create commits".to_string(),
            version: 1,
            user_invocable: true,
            agent_invocable: false,
            body: "# Commit\n\nBody.".to_string(),
            execution: Execution {
                model_profile: Some("deep".to_string()),
                ..Default::default()
            },
            tools: vec![],
            skill: None,
            supporting_files: vec![],
            targets: vec![Provider::Codex],
            provider_overrides: HashMap::new(),
            routing: None,
        }
    }

    fn test_profiles() -> ProfilesMap {
        let mut profile = HashMap::new();
        profile.insert(
            "codex".to_string(),
            serde_json::json!({"model": "gpt-5.3-codex", "reasoning_effort": "medium"}),
        );
        HashMap::from([("deep".to_string(), profile)])
    }

    #[test]
    fn test_codex_includes_model_and_reasoning() {
        let (files, _) = adapt_codex(&test_spec(), &test_profiles());
        let content = String::from_utf8(files[0].content.clone()).unwrap();
        assert!(content.contains("model: gpt-5.3-codex"));
        assert!(content.contains("model_reasoning_effort: medium"));
    }

    #[test]
    fn test_codex_uses_id_for_name() {
        let (files, _) = adapt_codex(&test_spec(), &test_profiles());
        let content = String::from_utf8(files[0].content.clone()).unwrap();
        assert!(content.contains("name: commit"));
    }

    #[test]
    fn test_codex_path() {
        let (files, _) = adapt_codex(&test_spec(), &test_profiles());
        assert_eq!(
            files[0].path.to_str().unwrap(),
            "generated/codex/skills/commit/SKILL.md"
        );
    }
}
