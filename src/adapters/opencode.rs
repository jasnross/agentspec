use std::path::Path;

use indexmap::IndexMap;
use serde_json::Value;

use crate::format::render_markdown_with_frontmatter;
use crate::model::resolve_provider_model_config;
use crate::tools::{all_tool_names, tool_name};
use crate::types::{
    CompileWarning, GeneratedFile, NormalizedSpec, PresetsMap, Provider, SpecKind, WarnKind,
};

/// Build the boolean tool map used by `OpenCode` agents and agent-invocable skills.
///
/// Creates a universe of all tools that have an `OpenCode` mapping, sets them all to false,
/// then sets the spec's tools to true. This matches the TypeScript behavior exactly.
fn build_tool_map(
    spec: &NormalizedSpec,
    warnings: &mut Vec<CompileWarning>,
) -> serde_json::Map<String, Value> {
    // Build sorted universe of all OpenCode tool names (excludes intentionally unsupported)
    let tool_universe = all_tool_names(Provider::OpenCode);

    // Initialize all to false
    let mut tools_map = serde_json::Map::new();
    for name in &tool_universe {
        tools_map.insert((*name).to_string(), Value::Bool(false));
    }

    // Set spec's tools to true
    for tool in &spec.tools {
        match tool_name(tool.as_str(), Provider::OpenCode) {
            None => {
                warnings.push(CompileWarning {
                    code: WarnKind::MissingMapping,
                    provider: Provider::OpenCode,
                    spec_id: spec.id.clone(),
                    field: format!("capabilities.tools.{tool}"),
                    message: format!("Tool '{tool}' does not map to an OpenCode tool."),
                });
            }
            Some(None) => {
                // Intentionally unsupported on OpenCode (e.g., ls) → silently skip
            }
            Some(Some(name)) => {
                tools_map.insert(name.to_string(), Value::Bool(true));
            }
        }
    }

    tools_map
}

pub fn adapt_opencode(
    spec: &NormalizedSpec,
    profiles: &PresetsMap,
) -> (Vec<GeneratedFile>, Vec<CompileWarning>) {
    let mut warnings = Vec::new();

    if spec.kind == SpecKind::Rule {
        // `OpenCode` has no per-file activation trigger, so `paths:` is intentionally dropped.
        // Rule content is emitted as plain body; `instructions.json` is built separately
        // by `build_opencode_instructions` after all specs are compiled.
        let content = format!("{}\n", spec.body.trim());
        let path = Path::new("generated")
            .join("opencode")
            .join("rules")
            .join(&spec.id)
            .join("AGENTS.md");
        return (
            vec![GeneratedFile::text(Provider::OpenCode, path, content)],
            warnings,
        );
    }

    let resolved_model = spec
        .execution
        .preset
        .as_ref()
        .map(|profile| resolve_provider_model_config(profile, Provider::OpenCode, profiles))
        .unwrap_or_default();

    let tools_map = build_tool_map(spec, &mut warnings);

    // Build base frontmatter (used for agents and agent-invocable skills)
    let mut fm = IndexMap::new();
    fm.insert(
        "description".to_string(),
        Value::String(spec.description.clone()),
    );
    fm.insert("tools".to_string(), Value::Object(tools_map));

    if let Some(ref model) = resolved_model.model {
        fm.insert("model".to_string(), Value::String(model.clone()));
    }

    if let Some(ref variant) = resolved_model.variant {
        fm.insert("variant".to_string(), Value::String(variant.clone()));
    }

    if spec.kind == SpecKind::Agent
        && let Some(ref mode) = spec.execution.mode
    {
        fm.insert("mode".to_string(), Value::String(mode.clone()));
    }

    if let Some(temp) = spec.execution.temperature {
        fm.insert(
            "temperature".to_string(),
            Value::Number(
                serde_json::Number::from_f64(temp)
                    .unwrap_or_else(|| serde_json::Number::from(0u64)),
            ),
        );
    }

    // Skills: dual output based on invocability
    if spec.kind == SpecKind::Skill {
        let mut files = Vec::new();

        if spec.user_invocable {
            let mut cmd_fm = IndexMap::new();
            cmd_fm.insert(
                "description".to_string(),
                Value::String(spec.description.clone()),
            );
            if let Some(ref skill_meta) = spec.skill
                && let Some(ref delegate_to) = skill_meta.delegate_to
            {
                cmd_fm.insert("agent".to_string(), Value::String(delegate_to.clone()));
            }
            if let Some(ref model) = resolved_model.model {
                cmd_fm.insert("model".to_string(), Value::String(model.clone()));
            }
            files.push(GeneratedFile::text(
                Provider::OpenCode,
                Path::new("generated")
                    .join("opencode")
                    .join("commands")
                    .join(format!("{}.md", spec.id)),
                render_markdown_with_frontmatter(&cmd_fm, &spec.body),
            ));
        }

        if spec.agent_invocable {
            // Agent-invocable skills get the full frontmatter with tools map
            fm.insert("name".to_string(), Value::String(spec.name.clone()));
            let skill_dir = Path::new("generated")
                .join("opencode")
                .join("skills")
                .join(&spec.id);
            files.push(GeneratedFile::text(
                Provider::OpenCode,
                skill_dir.join("SKILL.md"),
                render_markdown_with_frontmatter(&fm, &spec.body),
            ));
            for sf in &spec.supporting_files {
                files.push(GeneratedFile::binary(
                    Provider::OpenCode,
                    skill_dir.join(&sf.relative_path),
                    sf.content.clone(),
                    if sf.executable { Some(0o755) } else { None },
                ));
            }
        }

        return (files, warnings);
    }

    // Agents → flat files
    let files = vec![GeneratedFile::text(
        Provider::OpenCode,
        Path::new("generated")
            .join("opencode")
            .join("agents")
            .join(format!("{}.md", spec.id)),
        render_markdown_with_frontmatter(&fm, &spec.body),
    )];

    (files, warnings)
}

/// Build a standalone `instructions.json` fragment listing all `OpenCode` rule paths.
///
/// Returns `None` if no `OpenCode` rule files are present. The JSON structure is:
/// ```json
/// { "instructions": ["generated/opencode/rules/<id>/AGENTS.md", ...] }
/// ```
pub fn build_opencode_instructions(files: &[GeneratedFile]) -> Option<GeneratedFile> {
    let rules_prefix = std::path::Path::new("generated/opencode/rules");
    let rule_paths: Vec<serde_json::Value> = files
        .iter()
        .filter(|f| f.provider == Provider::OpenCode && f.path.starts_with(rules_prefix))
        .map(|f| serde_json::Value::String(f.path.to_string_lossy().into_owned()))
        .collect();
    if rule_paths.is_empty() {
        return None;
    }
    let json = serde_json::json!({ "instructions": rule_paths });
    let mut content =
        serde_json::to_string_pretty(&json).expect("instructions.json serialization is infallible");
    content.push('\n');
    Some(GeneratedFile::text(
        Provider::OpenCode,
        Path::new("generated")
            .join("opencode")
            .join("instructions.json"),
        content,
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::types::{Execution, SkillMeta};

    fn test_agent() -> NormalizedSpec {
        NormalizedSpec {
            source_path: "/test/agent.md".into(),
            id: "code-reviewer".to_string(),
            kind: SpecKind::Agent,
            paths: None,
            name: "code-reviewer".to_string(),
            description: "Reviews code".to_string(),
            version: 1,
            user_invocable: false,
            agent_invocable: true,
            body: "# Code Reviewer\n\nReview code.".to_string(),
            execution: Execution {
                mode: Some("subagent".to_string()),
                ..Default::default()
            },
            tools: vec!["bash".to_string(), "read".to_string()],
            skill: None,
            supporting_files: vec![],
            targets: vec![Provider::OpenCode],
            provider_overrides: HashMap::new(),
            routing: None,
        }
    }

    fn test_skill() -> NormalizedSpec {
        NormalizedSpec {
            source_path: "/test/skill.md".into(),
            id: "commit".to_string(),
            kind: SpecKind::Skill,
            paths: None,
            name: "commit".to_string(),
            description: "Create commits".to_string(),
            version: 1,
            user_invocable: true,
            agent_invocable: false,
            body: "# Commit\n\nBody.".to_string(),
            execution: Execution::default(),
            tools: vec!["bash".to_string()],
            skill: None,
            supporting_files: vec![],
            targets: vec![Provider::OpenCode],
            provider_overrides: HashMap::new(),
            routing: None,
        }
    }

    #[test]
    fn test_agent_has_boolean_tool_map() {
        let (files, _) = adapt_opencode(&test_agent(), &PresetsMap::new());
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");
        assert!(content.contains("bash: true"));
        assert!(content.contains("read: true"));
        assert!(content.contains("edit: false")); // not in spec's tools
    }

    #[test]
    fn test_agent_has_mode() {
        let (files, _) = adapt_opencode(&test_agent(), &PresetsMap::new());
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");
        assert!(content.contains("mode: subagent"));
    }

    #[test]
    fn test_agent_path() {
        let (files, _) = adapt_opencode(&test_agent(), &PresetsMap::new());
        assert_eq!(
            files[0].path.to_str().expect("expected value"),
            "generated/opencode/agents/code-reviewer.md"
        );
    }

    #[test]
    fn test_user_invocable_skill_creates_command() {
        let (files, _) = adapt_opencode(&test_skill(), &PresetsMap::new());
        assert_eq!(files.len(), 1); // only command, not agent-invocable
        assert_eq!(
            files[0].path.to_str().expect("expected value"),
            "generated/opencode/commands/commit.md"
        );
    }

    #[test]
    fn test_both_invocable_creates_two_files() {
        let mut spec = test_skill();
        spec.agent_invocable = true;

        let (files, _) = adapt_opencode(&spec, &PresetsMap::new());
        assert_eq!(files.len(), 2);
        let paths: Vec<String> = files
            .iter()
            .map(|f| f.path.to_str().expect("expected value").to_string())
            .collect();
        assert!(paths.iter().any(|p| p.contains("commands/")));
        assert!(paths.iter().any(|p| p.contains("skills/")));
    }

    #[test]
    fn test_command_with_delegate_to() {
        let mut spec = test_skill();
        spec.skill = Some(SkillMeta {
            delegate_to: Some("code-reviewer".to_string()),
            ..Default::default()
        });

        let (files, _) = adapt_opencode(&spec, &PresetsMap::new());
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");
        assert!(content.contains("agent: code-reviewer"));
    }

    fn test_rule() -> NormalizedSpec {
        NormalizedSpec {
            source_path: "/test/rule.md".into(),
            id: "api-design".to_string(),
            kind: SpecKind::Rule,
            paths: Some(vec!["src/api/**".to_string()]),
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
            targets: vec![Provider::OpenCode],
            provider_overrides: HashMap::new(),
            routing: None,
        }
    }

    #[test]
    fn test_rule_produces_agents_md() {
        let (files, warnings) = adapt_opencode(&test_rule(), &PresetsMap::new());
        assert!(warnings.is_empty());
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].path.to_str().expect("expected value"),
            "generated/opencode/rules/api-design/AGENTS.md"
        );
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");
        assert!(
            !content.contains("---"),
            "opencode rule should have no frontmatter"
        );
        assert!(
            !content.contains("src/api/**"),
            "paths should be dropped for opencode"
        );
        assert!(content.contains("# API Design"));
    }

    #[test]
    fn test_build_opencode_instructions_with_rules() {
        let rule_file = GeneratedFile::text(
            Provider::OpenCode,
            "generated/opencode/rules/api-design/AGENTS.md",
            "body".to_string(),
        );
        let result = build_opencode_instructions(&[rule_file]);
        assert!(result.is_some());
        let file = result.expect("expected value");
        assert_eq!(
            file.path.to_str().expect("expected value"),
            "generated/opencode/instructions.json"
        );
        let content = String::from_utf8(file.content.clone()).expect("expected value");
        assert!(content.contains("generated/opencode/rules/api-design/AGENTS.md"));
    }

    #[test]
    fn test_build_opencode_instructions_none_without_rules() {
        let skill_file = GeneratedFile::text(
            Provider::OpenCode,
            "generated/opencode/commands/commit.md",
            "body".to_string(),
        );
        let result = build_opencode_instructions(&[skill_file]);
        assert!(result.is_none());
    }

    #[test]
    fn test_missing_tool_mapping_warns() {
        let mut spec = test_agent();
        spec.tools = vec!["unknown_tool".to_string()];

        let (_, warnings) = adapt_opencode(&spec, &PresetsMap::new());
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, WarnKind::MissingMapping);
    }
}
