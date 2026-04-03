use std::path::Path;

use anyhow::Result;
use indexmap::IndexMap;
use serde::Serialize;
use strum::VariantArray as _;

use crate::compile::{AdapterConfig, GeneratedFile};
use crate::presets::ProviderPresetsMap;
use crate::provider::Provider;
use crate::spec::{
    NormalizedAgentSpec, NormalizedRuleSpec, NormalizedSkillSpec, NormalizedSpec, ToolFrontmatter,
};

// See: https://opencode.ai/docs/agents/#markdown
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
struct OpenCodeAgentFrontmatter {
    description: String,
    mode: &'static str,
    model: Option<String>,
    variant: Option<String>,
    tools: IndexMap<String, bool>,
}

// See: https://opencode.ai/docs/commands/#markdown
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
struct OpenCodeCommandFrontmatter {
    // FIXME: Support executing commands in forked subagents
    description: String,
    model: Option<String>,
}

// See: https://opencode.ai/docs/skills/#write-frontmatter
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
struct OpenCodeSkillFrontmatter {
    name: String,
    description: String,
    model: Option<String>,
    variant: Option<String>,
    tools: IndexMap<String, bool>,
}

pub fn adapt_opencode(
    spec: NormalizedSpec,
    presets: &ProviderPresetsMap,
    cfg: Option<&AdapterConfig>,
) -> Result<Vec<GeneratedFile>> {
    match spec {
        NormalizedSpec::Agent(s) => adapt_agent_spec(s, presets, cfg),
        NormalizedSpec::Skill(s) => adapt_skill_spec(s, presets, cfg),
        NormalizedSpec::Rule(s) => Ok(adapt_rule_spec(&s, cfg)),
    }
}

fn adapt_agent_spec(
    spec: NormalizedAgentSpec,
    presets: &ProviderPresetsMap,
    cfg: Option<&AdapterConfig>,
) -> Result<Vec<GeneratedFile>> {
    let id = spec.frontmatter.id;
    let description = spec.frontmatter.description;

    let preset = spec
        .frontmatter
        .execution
        .and_then(|x| x.preset)
        .and_then(|x| presets.get(&x))
        .and_then(|x| x.opencode.clone());
    let model = preset.as_ref().and_then(|x| x.model.clone());
    let variant = preset.as_ref().and_then(|x| x.variant.clone());

    let tools: Vec<ToolFrontmatter> = spec
        .frontmatter
        .capabilities
        .and_then(|x| x.tools)
        .into_iter()
        .flatten()
        .collect();

    let tools = build_tool_map(&tools);

    let frontmatter = OpenCodeAgentFrontmatter {
        description,
        mode: "subagent",
        model,
        variant,
        tools,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body;
    let content = format!("---\n{frontmatter_str}---\n\n{}", body.trim());

    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();

    Ok(vec![GeneratedFile::text(
        Provider::OpenCode,
        Path::new("agents").join(format!("{file_prefix}{id}.md")),
        content,
    )])
}

fn adapt_skill_spec(
    spec: NormalizedSkillSpec,
    presets: &ProviderPresetsMap,
    cfg: Option<&AdapterConfig>,
) -> Result<Vec<GeneratedFile>> {
    let id = spec.frontmatter.id;
    let description = spec.frontmatter.description.unwrap_or_default();
    let user_invocable = spec.frontmatter.user_invocable;
    let agent_invocable = spec.frontmatter.agent_invocable;

    let preset = spec
        .frontmatter
        .execution
        .and_then(|x| x.preset)
        .and_then(|x| presets.get(&x))
        .and_then(|x| x.opencode.clone());
    let model = preset.as_ref().and_then(|x| x.model.clone());
    let variant = preset.as_ref().and_then(|x| x.variant.clone());

    let tools: Vec<ToolFrontmatter> = spec
        .frontmatter
        .capabilities
        .and_then(|x| x.tools)
        .into_iter()
        .flatten()
        .collect();

    let tools = build_tool_map(&tools);

    let body = spec.body;
    let supporting_files = spec.supporting_files;

    let mut files = Vec::new();

    if user_invocable {
        // OpenCode commands: prefix becomes a subdirectory, not a file prefix
        let cmd_path = match cfg.and_then(|c| c.prefix.as_deref()) {
            Some(prefix) => Path::new("commands").join(prefix).join(format!("{id}.md")),
            None => Path::new("commands").join(format!("{id}.md")),
        };

        let frontmatter = OpenCodeCommandFrontmatter {
            description: description.clone(),
            model: model.clone(),
        };
        let frontmatter_str = serde_yml::to_string(&frontmatter)?;
        let content = format!("---\n{frontmatter_str}---\n\n{}", body.trim());
        files.push(GeneratedFile::text(Provider::OpenCode, cmd_path, content));
    }

    if agent_invocable {
        let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();

        // OpenCode requires `name` — strip_name is a no-op for this provider
        let frontmatter = OpenCodeSkillFrontmatter {
            name: id.clone(),
            description,
            model,
            variant,
            tools,
        };
        let frontmatter_str = serde_yml::to_string(&frontmatter)?;
        let content = format!("---\n{frontmatter_str}---\n\n{}", body.trim());

        let skill_dir = Path::new("skills").join(format!("{file_prefix}{id}"));

        files.push(GeneratedFile::text(
            Provider::OpenCode,
            skill_dir.join("SKILL.md"),
            content,
        ));

        for sf in supporting_files {
            files.push(GeneratedFile::binary(
                Provider::OpenCode,
                skill_dir.join(&sf.relative_path),
                sf.content,
                if sf.executable { Some(0o755) } else { None },
            ));
        }
    }

    Ok(files)
}

fn adapt_rule_spec(spec: &NormalizedRuleSpec, cfg: Option<&AdapterConfig>) -> Vec<GeneratedFile> {
    let content = format!("{}\n", spec.body.trim());
    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let path = Path::new("rules")
        .join(format!("{file_prefix}{}", spec.frontmatter.id))
        .join("AGENTS.md");

    vec![GeneratedFile::text(Provider::OpenCode, path, content)]
}

/// Map a canonical tool to its `OpenCode` tool name.
fn opencode_tool_name(tool: &ToolFrontmatter) -> &'static str {
    match tool {
        ToolFrontmatter::Read => "read",
        ToolFrontmatter::Write => "write",
        ToolFrontmatter::Edit => "edit",
        ToolFrontmatter::Grep => "grep",
        ToolFrontmatter::Glob => "glob",
        ToolFrontmatter::Bash => "bash",
        ToolFrontmatter::WebFetch => "webfetch",
        ToolFrontmatter::WebSearch => "websearch",
        ToolFrontmatter::Question => "question",
        ToolFrontmatter::Tasks => "todowrite",
    }
}

/// Build the boolean tool map used by `OpenCode` agents and agent-invocable skills.
///
/// Initializes all ToolFrontmatter-expressible `OpenCode` tools to false, then enables the ones
/// listed in the spec. Tools outside this set (list, lsp, patch, skill) are omitted and use
/// `OpenCode`'s default (all enabled).
fn build_tool_map(tools: &[ToolFrontmatter]) -> IndexMap<String, bool> {
    let mut map: IndexMap<String, bool> = ToolFrontmatter::VARIANTS
        .iter()
        .map(|t| (opencode_tool_name(t).to_string(), false))
        .collect();

    for tool in tools {
        map.insert(opencode_tool_name(tool).to_string(), true);
    }

    map.sort_keys();

    map
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::spec::{
        NormalizedAgentFrontmatter, NormalizedAgentSpec, NormalizedSkillFrontmatter,
        NormalizedSkillSpec,
    };

    #[test]
    fn test_build_tool_map_keys_are_sorted() {
        let tools = &[ToolFrontmatter::Write, ToolFrontmatter::Read];
        let map = build_tool_map(tools);
        let keys: Vec<&str> = map.keys().map(String::as_str).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(
            keys, sorted,
            "tool map keys should be in alphabetical order"
        );
    }

    #[test]
    fn test_adapt_agent_output_format() {
        let spec = NormalizedSpec::Agent(NormalizedAgentSpec {
            path: "test.md".into(),
            frontmatter: NormalizedAgentFrontmatter {
                id: "test-agent".to_string(),
                description: "Test agent".to_string(),
                execution: None,
                capabilities: None,
            },
            body: "Body.".to_string(),
        });

        let files = adapt_opencode(spec, &HashMap::new(), None).expect("expected value");
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");

        let expected = concat!(
            "---\n",
            "description: Test agent\n",
            "mode: subagent\n",
            "tools:\n",
            "  bash: false\n",
            "  edit: false\n",
            "  glob: false\n",
            "  grep: false\n",
            "  question: false\n",
            "  read: false\n",
            "  todowrite: false\n",
            "  webfetch: false\n",
            "  websearch: false\n",
            "  write: false\n",
            "---\n",
            "\n",
            "Body.",
        );
        assert_eq!(content, expected);
    }

    #[test]
    fn test_adapt_skill_command_with_prefix_uses_subdirectory() {
        let cfg = AdapterConfig {
            prefix: Some("tw".to_string()),
            strip_name: false,
        };
        let spec = NormalizedSpec::Skill(NormalizedSkillSpec {
            path: "test.md".into(),
            frontmatter: NormalizedSkillFrontmatter {
                id: "basic-skill".to_string(),
                description: Some("A basic skill".to_string()),
                execution: None,
                capabilities: None,
                user_invocable: true,
                agent_invocable: false,
            },
            body: "Body.".to_string(),
            supporting_files: vec![],
        });

        let files = adapt_opencode(spec, &HashMap::new(), Some(&cfg)).expect("expected value");
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].path.to_str(),
            Some("commands/tw/basic-skill.md"),
            "OpenCode commands should use prefix as subdirectory"
        );
    }
}
