use std::path::Path;

use indexmap::IndexMap;
use serde_json::Value;

use crate::format::render_markdown_with_frontmatter;
use crate::types::{CompileWarning, GeneratedFile, NormalizedSpec, PresetsMap, Provider, SpecKind};

pub fn adapt_cursor(
    spec: &NormalizedSpec,
    _profiles: &PresetsMap,
) -> (Vec<GeneratedFile>, Vec<CompileWarning>) {
    let warnings = Vec::new();

    if spec.kind == SpecKind::Rule {
        let mut fm = IndexMap::new();
        fm.insert(
            "description".to_string(),
            Value::String(spec.description.clone()),
        );
        match &spec.paths {
            Some(paths) => {
                fm.insert(
                    "globs".to_string(),
                    Value::Array(paths.iter().map(|p| Value::String(p.clone())).collect()),
                );
            }
            None => {
                fm.insert("alwaysApply".to_string(), Value::Bool(true));
            }
        }
        let content = render_markdown_with_frontmatter(&fm, &spec.body);
        let path = Path::new("generated")
            .join("cursor")
            .join("rules")
            .join(format!("{}.mdc", spec.id));
        return (
            vec![GeneratedFile::text(Provider::Cursor, path, content)],
            warnings,
        );
    }

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
            paths: None,
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
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");
        // Cursor uses spec.id for name, not spec.name
        assert!(content.contains("name: commit"));
        assert!(!content.contains("name: Commit Changes"));
    }

    #[test]
    fn test_cursor_no_tools_or_model() {
        let (files, _) = adapt_cursor(&test_spec(), &PresetsMap::new());
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");
        assert!(!content.contains("tools"));
        assert!(!content.contains("model"));
        assert!(!content.contains("allowed-tools"));
    }

    fn test_rule() -> NormalizedSpec {
        NormalizedSpec {
            source_path: "/test/rule.md".into(),
            id: "api-design".to_string(),
            kind: SpecKind::Rule,
            paths: None,
            name: "api-design".to_string(),
            description: "API design rules".to_string(),
            version: 1,
            user_invocable: false,
            agent_invocable: false,
            body: "# API Design\n\nValidate inputs.".to_string(),
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
    fn test_rule_without_paths_always_apply() {
        let (files, _) = adapt_cursor(&test_rule(), &PresetsMap::new());
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].path.to_str().expect("expected value"),
            "generated/cursor/rules/api-design.mdc"
        );
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");
        assert!(content.contains("alwaysApply: true"));
        assert!(!content.contains("globs"));
    }

    #[test]
    fn test_rule_with_paths_uses_globs() {
        let mut spec = test_rule();
        spec.paths = Some(vec!["src/api/**".to_string()]);
        let (files, _) = adapt_cursor(&spec, &PresetsMap::new());
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");
        assert!(content.contains("globs:"));
        assert!(content.contains("src/api/**"));
        assert!(!content.contains("alwaysApply"));
    }

    #[test]
    fn test_cursor_skill_path() {
        let (files, _) = adapt_cursor(&test_spec(), &PresetsMap::new());
        assert_eq!(
            files[0].path.to_str().expect("expected value"),
            "generated/cursor/skills/commit/SKILL.md"
        );
    }
}
