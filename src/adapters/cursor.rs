use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::compile::{AdapterConfig, GeneratedFile};
use crate::plan::{FileKind, PostWriteHook};
use crate::presets::ProviderPresetsMap;
use crate::provider::Provider;
use crate::spec::{
    NormalizedAgentSpec, NormalizedRuleSpec, NormalizedSkillSpec, NormalizedSpec, ToolFrontmatter,
};

// See: https://cursor.com/docs/subagents#configuration-fields
#[derive(Serialize)]
struct CursorAgentFrontmatter {
    name: String,
    description: String,
    model: Option<String>,
}

// See: https://cursor.com/docs/skills#frontmatter-fields
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CursorSkillFrontmatter {
    name: String,
    description: String,
    disable_model_invocation: bool,
}

// See: https://cursor.com/docs/rules#rule-file-format
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CursorRuleFrontmatter {
    description: String,
    always_apply: bool,
}

pub fn adapt_cursor(
    spec: NormalizedSpec,
    presets: &ProviderPresetsMap,
    cfg: Option<&AdapterConfig>,
) -> Result<Vec<GeneratedFile>> {
    match spec {
        NormalizedSpec::Agent(s) => adapt_agent_spec(s, presets, cfg),
        NormalizedSpec::Skill(s) => adapt_skill_spec(s, cfg),
        NormalizedSpec::Rule(s) => adapt_rule_spec(s, cfg),
    }
}

fn adapt_agent_spec(
    spec: NormalizedAgentSpec,
    presets: &ProviderPresetsMap,
    cfg: Option<&AdapterConfig>,
) -> Result<Vec<GeneratedFile>> {
    let id = spec.frontmatter.id;
    let description = spec.frontmatter.description;

    let model = spec
        .frontmatter
        .execution
        .and_then(|x| x.preset)
        .and_then(|x| presets.get(&x))
        .and_then(|x| x.cursor.clone())
        .and_then(|x| x.model);

    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let path = Path::new("agents").join(format!("{file_prefix}{id}.md"));

    // Cursor agents get frontmatter name prefix with "-" delimiter
    let name = match cfg.and_then(|c| c.prefix.as_deref()) {
        Some(prefix) => format!("{prefix}-{id}"),
        None => id,
    };

    let frontmatter = CursorAgentFrontmatter {
        name,
        description,
        model,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}---\n\n{body}");

    Ok(vec![GeneratedFile::text(Provider::Cursor, path, content)])
}

fn adapt_skill_spec(
    spec: NormalizedSkillSpec,
    cfg: Option<&AdapterConfig>,
) -> Result<Vec<GeneratedFile>> {
    let id = spec.frontmatter.id;
    let description = spec.frontmatter.description.unwrap_or_default();

    let name = match cfg.and_then(|c| c.prefix.as_deref()) {
        Some(prefix) => format!("{prefix}-{id}"),
        None => id.clone(),
    };

    let frontmatter = CursorSkillFrontmatter {
        name,
        description,
        disable_model_invocation: !spec.frontmatter.agent_invocable,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}---\n\n{body}");

    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let skill_dir = Path::new("skills").join(format!("{file_prefix}{id}"));

    let mut files = vec![GeneratedFile::text(
        Provider::Cursor,
        skill_dir.join("SKILL.md"),
        content,
    )];

    for sf in spec.supporting_files {
        files.push(GeneratedFile::binary(
            Provider::Cursor,
            skill_dir.join(&sf.relative_path),
            sf.content,
            if sf.executable { Some(0o755) } else { None },
        ));
    }

    Ok(files)
}

fn adapt_rule_spec(
    spec: NormalizedRuleSpec,
    cfg: Option<&AdapterConfig>,
) -> Result<Vec<GeneratedFile>> {
    let description = spec.frontmatter.description.unwrap_or_default();

    let frontmatter = CursorRuleFrontmatter {
        description,
        always_apply: true,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}---\n\n{body}");

    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let path = Path::new("rules").join(format!("{file_prefix}{}.mdc", spec.frontmatter.id));

    Ok(vec![GeneratedFile::text(Provider::Cursor, path, content)])
}

/// Resolve a canonical tool to the name a Cursor spec body should reference.
///
/// For tools with no native Cursor equivalent, returns a descriptive phrase
/// (not a real tool name) so the model understands the intent.
pub fn body_tool_name(tool: &ToolFrontmatter) -> &'static str {
    match tool {
        ToolFrontmatter::Read => "read_file",
        ToolFrontmatter::Write | ToolFrontmatter::Edit => "edit_file",
        ToolFrontmatter::Grep => "grep_search",
        ToolFrontmatter::Glob => "file_search",
        ToolFrontmatter::Bash => "run_terminal_cmd",
        ToolFrontmatter::WebSearch => "web_search",
        ToolFrontmatter::WebFetch => "URL fetcher",
        ToolFrontmatter::Question => "question picker",
        ToolFrontmatter::Tasks => "task tracker",
    }
}

pub fn post_write_hook(
    _kind: FileKind,
    _dest: &Path,
    _config_dir: &Path,
) -> Option<Box<dyn PostWriteHook>> {
    None
}

/// Returns the name the AI model uses to reference this spec.
///
/// For Cursor, all spec types use `{content_prefix}{id}` when a content prefix
/// is configured (either explicitly or derived from `prefix`).
pub fn model_facing_name(spec: &NormalizedSpec, cfg: Option<&AdapterConfig>) -> String {
    let id = spec.id();
    match cfg.and_then(AdapterConfig::content_prefix) {
        Some(prefix) => format!("{prefix}{id}"),
        None => id.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::spec::{
        NormalizedAgentFrontmatter, NormalizedAgentSpec, NormalizedRuleFrontmatter,
        NormalizedRuleSpec, NormalizedSkillFrontmatter, NormalizedSkillSpec,
    };

    #[test]
    fn test_adapt_agent_output_format() {
        let spec = NormalizedSpec::Agent(NormalizedAgentSpec {
            path: "test.md".into(),
            frontmatter: NormalizedAgentFrontmatter {
                id: "test-agent".to_string(),
                description: "Test agent".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: "Body.".to_string(),
        });

        let files = adapt_cursor(spec, &HashMap::new(), None).expect("expected value");
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");

        let expected = concat!(
            "---\n",
            "name: test-agent\n",
            "description: Test agent\n",
            "model: null\n",
            "---\n",
            "\n",
            "Body.",
        );
        assert_eq!(content, expected);
    }

    #[test]
    fn test_adapt_agent_with_prefix() {
        let cfg = AdapterConfig {
            prefix: Some("tw".to_string()),
            content_prefix: None,
        };
        let spec = NormalizedSpec::Agent(NormalizedAgentSpec {
            path: "test.md".into(),
            frontmatter: NormalizedAgentFrontmatter {
                id: "test-agent".to_string(),
                description: "Test agent".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: "Body.".to_string(),
        });

        let files = adapt_cursor(spec, &HashMap::new(), Some(&cfg)).expect("expected value");
        assert_eq!(files[0].path.to_string_lossy(), "agents/tw-test-agent.md");
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");
        assert!(
            content.contains("name: tw-test-agent"),
            "expected prefixed name with '-' delimiter, got: {content}"
        );
    }

    #[test]
    fn test_adapt_skill_with_prefix() {
        let cfg = AdapterConfig {
            prefix: Some("tw".to_string()),
            content_prefix: None,
        };
        let spec = NormalizedSpec::Skill(NormalizedSkillSpec {
            path: "test.md".into(),
            frontmatter: NormalizedSkillFrontmatter {
                id: "test-skill".to_string(),
                description: Some("A test skill".to_string()),
                tags: None,
                execution: None,
                capabilities: None,
                user_invocable: true,
                agent_invocable: true,
            },
            body: "Body.".to_string(),
            supporting_files: vec![],
        });

        let files = adapt_cursor(spec, &HashMap::new(), Some(&cfg)).expect("expected value");
        assert_eq!(
            files[0].path.to_string_lossy(),
            "skills/tw-test-skill/SKILL.md"
        );
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");
        assert!(
            content.contains("name: tw-test-skill"),
            "expected prefixed name with '-' delimiter, got: {content}"
        );
    }

    #[test]
    fn test_body_tool_name_full_mapping() {
        assert_eq!(body_tool_name(&ToolFrontmatter::Read), "read_file");
        assert_eq!(body_tool_name(&ToolFrontmatter::Write), "edit_file");
        assert_eq!(body_tool_name(&ToolFrontmatter::Edit), "edit_file");
        assert_eq!(body_tool_name(&ToolFrontmatter::Grep), "grep_search");
        assert_eq!(body_tool_name(&ToolFrontmatter::Glob), "file_search");
        assert_eq!(body_tool_name(&ToolFrontmatter::Bash), "run_terminal_cmd");
        assert_eq!(body_tool_name(&ToolFrontmatter::WebSearch), "web_search");
        assert_eq!(body_tool_name(&ToolFrontmatter::WebFetch), "URL fetcher");
        assert_eq!(
            body_tool_name(&ToolFrontmatter::Question),
            "question picker"
        );
        assert_eq!(body_tool_name(&ToolFrontmatter::Tasks), "task tracker");
    }

    #[test]
    fn test_adapt_rule_with_prefix() {
        let cfg = AdapterConfig {
            prefix: Some("tw".to_string()),
            content_prefix: None,
        };
        let spec = NormalizedSpec::Rule(NormalizedRuleSpec {
            path: "test.md".into(),
            frontmatter: NormalizedRuleFrontmatter {
                id: "test-rule".to_string(),
                description: Some("A test rule".to_string()),
                tags: None,
            },
            body: "Rule body.".to_string(),
        });

        let files = adapt_cursor(spec, &HashMap::new(), Some(&cfg)).expect("expected value");
        assert_eq!(files[0].path.to_str(), Some("rules/tw-test-rule.mdc"));
    }
}
