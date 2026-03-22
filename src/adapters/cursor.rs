use std::path::Path;

use indexmap::IndexMap;
use serde_json::Value;

use crate::format::render_markdown_with_frontmatter;
use crate::types::{CompileWarning, GeneratedFile, NormalizedSpec, PresetsMap, Provider};

pub fn adapt_cursor(
    spec: &NormalizedSpec,
    _profiles: &PresetsMap,
) -> (Vec<GeneratedFile>, Vec<CompileWarning>) {
    let warnings = Vec::new();

    let mut fm = IndexMap::new();
    fm.insert("name".to_string(), Value::String(spec.id.clone()));
    fm.insert(
        "description".to_string(),
        Value::String(spec.description.clone()),
    );

    let skill_dir = Path::new("generated")
        .join("cursor")
        .join("skills")
        .join(&spec.id);

    let mut files = vec![GeneratedFile::text(
        Provider::Cursor,
        skill_dir.join("SKILL.md"),
        render_markdown_with_frontmatter(&fm, &spec.body),
    )];

    for sf in &spec.supporting_files {
        files.push(GeneratedFile::binary(
            Provider::Cursor,
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
            name: "Commit Changes".to_string(),
            description: "Create commits".to_string(),
            version: 1,
            user_invocable: true,
            agent_invocable: false,
            body: "# Commit\n\nBody.".to_string(),
            execution: Execution::default(),
            tools: vec![],
            skill: None,
            supporting_files: vec![],
            targets: vec![Provider::Cursor],
            provider_overrides: HashMap::new(),
            routing: None,
        }
    }

    #[test]
    fn test_cursor_uses_id_not_name() {
        let (files, _) = adapt_cursor(&test_spec(), &PresetsMap::new());
        let content = String::from_utf8(files[0].content.clone()).unwrap();
        // Cursor uses spec.id for name, not spec.name
        assert!(content.contains("name: commit"));
        assert!(!content.contains("name: Commit Changes"));
    }

    #[test]
    fn test_cursor_no_tools_or_model() {
        let (files, _) = adapt_cursor(&test_spec(), &PresetsMap::new());
        let content = String::from_utf8(files[0].content.clone()).unwrap();
        assert!(!content.contains("tools"));
        assert!(!content.contains("model"));
        assert!(!content.contains("allowed-tools"));
    }

    #[test]
    fn test_cursor_skill_path() {
        let (files, _) = adapt_cursor(&test_spec(), &PresetsMap::new());
        assert_eq!(
            files[0].path.to_str().unwrap(),
            "generated/cursor/skills/commit/SKILL.md"
        );
    }
}
