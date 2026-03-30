use std::path::Path;

use anyhow::Result;
use indexmap::IndexMap;
use serde::Serialize;
use strum::VariantArray as _;

use crate::compile::GeneratedFile;
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
    description: String,
    agent: &'static str,
    subtask: bool,
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

// TODO: Remember that for OpenCode we need to add the rules as instructions in opencode.json when syncing
// See: https://opencode.ai/docs/rules/#custom-instructions

pub fn adapt_opencode(
    spec: NormalizedSpec,
    presets: &ProviderPresetsMap,
) -> Result<Vec<GeneratedFile>> {
    match spec {
        NormalizedSpec::Agent(s) => adapt_agent_spec(s, presets),
        NormalizedSpec::Skill(s) => adapt_skill_spec(s, presets),
        NormalizedSpec::Rule(s) => Ok(adapt_rule_spec(&s)),
    }
}

fn adapt_agent_spec(
    spec: NormalizedAgentSpec,
    presets: &ProviderPresetsMap,
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
    let content = format!("---\n{frontmatter_str}\n---\n\n{}", body.trim()).into_bytes();

    Ok(vec![GeneratedFile {
        provider: Provider::OpenCode,
        path: Path::new("generated")
            .join("opencode")
            .join("agents")
            .join(format!("{id}.md")),
        content,
        mode: None,
    }])
}

fn adapt_skill_spec(
    spec: NormalizedSkillSpec,
    presets: &ProviderPresetsMap,
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
        let frontmatter = OpenCodeCommandFrontmatter {
            description: description.clone(),
            agent: "build", // FIXME: support `agent` in config
            subtask: true,  // FIXME: support `background` in config
            model: model.clone(),
        };
        let frontmatter_str = serde_yml::to_string(&frontmatter)?;
        let content = format!("---\n{frontmatter_str}\n---\n\n{}", body.trim()).into_bytes();
        files.push(GeneratedFile {
            provider: Provider::OpenCode,
            path: Path::new("generated")
                .join("opencode")
                .join("commands")
                .join(format!("{id}.md")),
            content,
            mode: None,
        });
    }

    if agent_invocable {
        let frontmatter = OpenCodeSkillFrontmatter {
            name: id.clone(),
            description,
            model,
            variant,
            tools,
        };
        let frontmatter_str = serde_yml::to_string(&frontmatter)?;
        let content = format!("---\n{frontmatter_str}\n---\n\n{}", body.trim()).into_bytes();

        let skill_dir = Path::new("generated")
            .join("opencode")
            .join("skills")
            .join(&id);

        files.push(GeneratedFile {
            provider: Provider::OpenCode,
            path: skill_dir.join("SKILL.md"),
            content,
            mode: None,
        });

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

fn adapt_rule_spec(spec: &NormalizedRuleSpec) -> Vec<GeneratedFile> {
    let content = format!("{}\n", spec.body.trim()).into_bytes();
    let path = Path::new("generated")
        .join("opencode")
        .join("rules")
        .join(&spec.frontmatter.id)
        .join("AGENTS.md");

    vec![GeneratedFile {
        provider: Provider::OpenCode,
        path,
        content,
        mode: None,
    }]
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
    use super::*;

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
}
