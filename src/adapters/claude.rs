use std::path::Path;

use indexmap::IndexMap;
use serde_json::Value;

use crate::format::render_markdown_with_frontmatter;
use crate::model::resolve_provider_model_config;
use crate::types::{
    CompileWarning, GeneratedFile, MappingBundle, NormalizedSpec, Provider, SpecKind, WarnKind,
};

/// Map canonical tool IDs to Claude-specific tool names.
fn map_tools(
    spec: &NormalizedSpec,
    mappings: &MappingBundle,
    warnings: &mut Vec<CompileWarning>,
) -> Vec<String> {
    spec.tools
        .iter()
        .filter_map(|tool| {
            let mapped = mappings
                .tools
                .tools
                .get(tool.as_str())
                .and_then(|m| m.get("claude"));

            match mapped {
                None => {
                    // No mapping entry at all → warning
                    warnings.push(CompileWarning {
                        code: WarnKind::MissingMapping,
                        provider: Provider::Claude,
                        spec_id: spec.id.clone(),
                        field: format!("capabilities.tools.{tool}"),
                        message: format!("No Claude tool mapping for '{tool}'."),
                    });
                    None
                }
                Some(None) => {
                    // Explicitly null → intentionally unsupported, silently drop
                    None
                }
                Some(Some(name)) => Some(name.clone()),
            }
        })
        .collect()
}

pub fn adapt_claude(
    spec: &NormalizedSpec,
    mappings: &MappingBundle,
) -> (Vec<GeneratedFile>, Vec<CompileWarning>) {
    let mut warnings = Vec::new();

    if spec.kind == SpecKind::Skill {
        let mapped_tools = map_tools(spec, mappings, &mut warnings);

        let mut fm = IndexMap::new();
        fm.insert("name".to_string(), Value::String(spec.name.clone()));
        fm.insert(
            "description".to_string(),
            Value::String(spec.description.clone()),
        );

        if !mapped_tools.is_empty() {
            fm.insert(
                "allowed-tools".to_string(),
                Value::String(mapped_tools.join(", ")),
            );
        }

        if let Some(profile) = &spec.execution.model_profile {
            let resolved = resolve_provider_model_config(profile, Provider::Claude, mappings);
            if let Some(model) = resolved.model {
                fm.insert("model".to_string(), Value::String(model));
            }
        }

        if !spec.user_invocable {
            fm.insert("user-invocable".to_string(), Value::Bool(false));
        }
        if !spec.agent_invocable {
            fm.insert("disable-model-invocation".to_string(), Value::Bool(true));
        }

        let skill_dir = Path::new("generated")
            .join("claude")
            .join("skills")
            .join(&spec.id);

        let mut files = vec![GeneratedFile::text(
            Provider::Claude,
            skill_dir.join("SKILL.md"),
            render_markdown_with_frontmatter(&fm, &spec.body),
        )];

        for sf in &spec.supporting_files {
            files.push(GeneratedFile::binary(
                Provider::Claude,
                skill_dir.join(&sf.relative_path),
                sf.content.clone(),
                if sf.executable { Some(0o755) } else { None },
            ));
        }

        return (files, warnings);
    }

    // Agents → flat files
    let mapped_tools = map_tools(spec, mappings, &mut warnings);

    let mut fm = IndexMap::new();
    fm.insert("name".to_string(), Value::String(spec.name.clone()));
    fm.insert(
        "description".to_string(),
        Value::String(spec.description.clone()),
    );

    if !mapped_tools.is_empty() {
        fm.insert("tools".to_string(), Value::String(mapped_tools.join(", ")));
    }

    if let Some(profile) = &spec.execution.model_profile {
        let resolved = resolve_provider_model_config(profile, Provider::Claude, mappings);
        if let Some(model) = resolved.model {
            fm.insert("model".to_string(), Value::String(model));
        }
    }

    let files = vec![GeneratedFile::text(
        Provider::Claude,
        Path::new("generated")
            .join("claude")
            .join("agents")
            .join(format!("{}.md", spec.id)),
        render_markdown_with_frontmatter(&fm, &spec.body),
    )];

    (files, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Execution, MappingBundle, ToolsMapping};
    use std::collections::HashMap;

    fn test_mappings() -> MappingBundle {
        let mut tools = HashMap::new();
        let mut bash_map = HashMap::new();
        bash_map.insert("claude".to_string(), Some("Bash".to_string()));
        tools.insert("bash".to_string(), bash_map);

        let mut read_map = HashMap::new();
        read_map.insert("claude".to_string(), Some("Read".to_string()));
        tools.insert("read".to_string(), read_map);

        // task is null for claude (unsupported)
        let mut task_map = HashMap::new();
        task_map.insert("claude".to_string(), None);
        tools.insert("task".to_string(), task_map);

        MappingBundle {
            tools: ToolsMapping { tools },
            ..Default::default()
        }
    }

    fn test_skill() -> NormalizedSpec {
        NormalizedSpec {
            source_path: "/test/spec.md".into(),
            id: "commit".to_string(),
            kind: SpecKind::Skill,
            name: "commit".to_string(),
            description: "Create commits".to_string(),
            version: 1,
            user_invocable: true,
            agent_invocable: false,
            body: "# Commit\n\nBody here.".to_string(),
            execution: Execution::default(),
            tools: vec!["bash".to_string(), "read".to_string()],
            skill: None,
            supporting_files: vec![],
            targets: vec![Provider::Claude],
            provider_overrides: HashMap::new(),
            routing: None,
        }
    }

    #[test]
    fn test_skill_generates_correct_path() {
        let (files, warnings) = adapt_claude(&test_skill(), &test_mappings());
        assert!(warnings.is_empty());
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].path.to_str().unwrap(),
            "generated/claude/skills/commit/SKILL.md"
        );
    }

    #[test]
    fn test_skill_frontmatter_contains_tools() {
        let (files, _) = adapt_claude(&test_skill(), &test_mappings());
        let content = String::from_utf8(files[0].content.clone()).unwrap();
        assert!(content.contains("allowed-tools: Bash, Read"));
    }

    #[test]
    fn test_skill_not_agent_invocable_adds_disable() {
        let (files, _) = adapt_claude(&test_skill(), &test_mappings());
        let content = String::from_utf8(files[0].content.clone()).unwrap();
        assert!(content.contains("disable-model-invocation: true"));
        // user_invocable is true, so no user-invocable: false
        assert!(!content.contains("user-invocable: false"));
    }

    #[test]
    fn test_agent_generates_flat_file() {
        let mut spec = test_skill();
        spec.kind = SpecKind::Agent;
        spec.id = "code-reviewer".to_string();
        spec.name = "code-reviewer".to_string();

        let (files, _) = adapt_claude(&spec, &test_mappings());
        assert_eq!(
            files[0].path.to_str().unwrap(),
            "generated/claude/agents/code-reviewer.md"
        );
        let content = String::from_utf8(files[0].content.clone()).unwrap();
        // Agents use "tools" not "allowed-tools"
        assert!(content.contains("tools: Bash, Read"));
    }

    #[test]
    fn test_null_tool_mapping_silently_dropped() {
        let mut spec = test_skill();
        spec.tools = vec!["task".to_string()]; // null mapping for claude

        let (files, warnings) = adapt_claude(&spec, &test_mappings());
        assert!(warnings.is_empty());
        let content = String::from_utf8(files[0].content.clone()).unwrap();
        assert!(!content.contains("allowed-tools"));
    }

    #[test]
    fn test_missing_tool_mapping_warns() {
        let mut spec = test_skill();
        spec.tools = vec!["unknown_tool".to_string()];

        let (_, warnings) = adapt_claude(&spec, &test_mappings());
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, WarnKind::MissingMapping);
        assert!(warnings[0].message.contains("unknown_tool"));
    }
}
