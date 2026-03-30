use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::compile::GeneratedFile;
use crate::presets::ProviderPresetsMap;
use crate::provider::Provider;
use crate::spec::{
    NormalizedAgentSpec, NormalizedRuleSpec, NormalizedSkillSpec, NormalizedSpec, ToolFrontmatter,
};

// See: https://code.claude.com/docs/en/sub-agents#supported-frontmatter-fields
#[derive(Serialize)]
struct ClaudeAgentFrontmatter {
    name: String,
    description: String,
    model: Option<String>,
    tools: Option<Vec<ClaudeTool>>,
}

// See: https://code.claude.com/docs/en/skills#frontmatter-reference
#[serde_with::skip_serializing_none]
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ClaudeSkillFrontmatter {
    // FIXME: Support executing skills in forked subagents
    name: String,
    description: String,
    model: Option<String>,
    user_invocable: Option<bool>,
    disable_model_invocation: Option<bool>,
    allowed_tools: Option<Vec<ClaudeTool>>,
}

// FIXME: Should we consider setting all default Claude tools in the generated file? Otherwise Claude's default behavior is to disallow any unlisted tools.
// See: https://code.claude.com/docs/en/tools-reference
#[derive(Serialize)]
#[allow(dead_code)] // FIXME: Consider removing unused if we figure out something better
enum ClaudeTool {
    Agent,
    AskUserQuestion,
    Bash, // FIXME: consider merging Bash and PowerShell under one `shell` canonical tool
    CronCreate,
    CronDelete,
    CronList,
    Edit,
    EnterPlanMode,
    EnterWorktree,
    ExitPlanMode,
    ExitWorktree,
    Glob,
    Grep,
    ListMcpResourcesTool,
    Lsp,
    NotebookEdit,
    PowerShell,
    Read,
    ReadMcpResourceTool,
    Skill,
    TaskCreate,
    TaskGet,
    TaskList,
    TaskOutput,
    TaskStop,
    TaskUpdate,
    TodoWrite,
    ToolSearch,
    WebFetch,
    WebSearch,
    Write,
}

pub fn adapt_claude(
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
    let name = spec.frontmatter.id;
    let description = spec.frontmatter.description;

    let model = spec
        .frontmatter
        .execution
        .and_then(|x| x.preset)
        .and_then(|x| presets.get(&x))
        .and_then(|x| x.claude.clone())
        .and_then(|x| x.model);

    let tools: Option<Vec<ClaudeTool>> =
        spec.frontmatter
            .capabilities
            .and_then(|x| x.tools)
            .map(|x| {
                let mut tools: Vec<ClaudeTool> = x.iter().flat_map(adapt_tool).collect();
                // Sort by serialized name — the value that appears in generated files.
                tools.sort_by_key(|t| serde_yml::to_string(t).unwrap_or_default());
                tools
            });

    let path = Path::new("generated")
        .join("claude")
        .join("agents")
        .join(format!("{name}.md"));

    let frontmatter = ClaudeAgentFrontmatter {
        name,
        description,
        model,
        tools,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}---\n\n{body}");

    Ok(vec![GeneratedFile::text(Provider::Claude, path, content)])
}

fn adapt_skill_spec(
    spec: NormalizedSkillSpec,
    presets: &ProviderPresetsMap,
) -> Result<Vec<GeneratedFile>> {
    let name = spec.frontmatter.id;
    let description = spec.frontmatter.description.unwrap_or_default();

    let model = spec
        .frontmatter
        .execution
        .and_then(|x| x.preset)
        .and_then(|x| presets.get(&x))
        .and_then(|x| x.claude.clone())
        .and_then(|x| x.model);

    let allowed_tools: Option<Vec<ClaudeTool>> = spec
        .frontmatter
        .capabilities
        .and_then(|x| x.tools)
        .map(|x| x.iter().flat_map(adapt_tool).collect());

    let user_invocable = if spec.frontmatter.user_invocable {
        None
    } else {
        Some(false)
    };

    let disable_model_invocation = if spec.frontmatter.agent_invocable {
        None
    } else {
        Some(true)
    };

    let skill_dir = Path::new("generated")
        .join("claude")
        .join("skills")
        .join(&name);

    let frontmatter = ClaudeSkillFrontmatter {
        name,
        description,
        model,
        user_invocable,
        disable_model_invocation,
        allowed_tools,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}---\n\n{body}");

    let mut files = vec![GeneratedFile::text(
        Provider::Claude,
        skill_dir.join("SKILL.md"),
        content,
    )];

    for sf in spec.supporting_files {
        files.push(GeneratedFile::binary(
            Provider::Claude,
            skill_dir.join(&sf.relative_path),
            sf.content,
            if sf.executable { Some(0o755) } else { None },
        ));
    }

    Ok(files)
}

fn adapt_rule_spec(spec: &NormalizedRuleSpec) -> Vec<GeneratedFile> {
    let content = format!("{}\n", spec.body.trim()).into_bytes();
    let path = Path::new("generated")
        .join("claude")
        .join("rules")
        .join(format!("{}.md", spec.frontmatter.id));

    vec![GeneratedFile {
        provider: Provider::Claude,
        path,
        content,
        mode: None,
    }]
}

fn adapt_tool(tool: &ToolFrontmatter) -> Vec<ClaudeTool> {
    match tool {
        ToolFrontmatter::Read => vec![ClaudeTool::Read],
        ToolFrontmatter::Write => vec![ClaudeTool::Write],
        ToolFrontmatter::Edit => vec![ClaudeTool::Edit],
        ToolFrontmatter::Grep => vec![ClaudeTool::Grep],
        ToolFrontmatter::Glob => vec![ClaudeTool::Glob],
        ToolFrontmatter::Bash => vec![ClaudeTool::Bash],
        ToolFrontmatter::WebFetch => vec![ClaudeTool::WebFetch],
        ToolFrontmatter::WebSearch => vec![ClaudeTool::WebSearch],
        ToolFrontmatter::Question => vec![ClaudeTool::AskUserQuestion],
        ToolFrontmatter::Tasks => vec![
            ClaudeTool::TaskCreate,
            ClaudeTool::TaskGet,
            ClaudeTool::TaskList,
            ClaudeTool::TaskUpdate,
            ClaudeTool::TaskStop,
            ClaudeTool::TodoWrite,
        ],
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde::Deserialize;

    use super::*;
    use crate::spec::{CapabilitiesFrontmatter, NormalizedAgentFrontmatter, NormalizedAgentSpec};

    #[test]
    fn test_adapt_agent_tools_are_sorted() {
        #[derive(Deserialize)]
        struct Frontmatter {
            tools: Option<Vec<String>>,
        }

        // Tools provided in reverse alphabetical order to confirm sorting.
        let spec = NormalizedSpec::Agent(NormalizedAgentSpec {
            path: "test.md".into(),
            frontmatter: NormalizedAgentFrontmatter {
                id: "test-agent".to_string(),
                description: "Test agent".to_string(),
                execution: None,
                capabilities: Some(CapabilitiesFrontmatter {
                    tools: Some(vec![
                        ToolFrontmatter::Write,
                        ToolFrontmatter::Read,
                        ToolFrontmatter::Bash,
                    ]),
                }),
            },
            body: "Body.".to_string(),
        });

        let files = adapt_claude(spec, &HashMap::new()).expect("expected value");
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");

        // Parse the tools list back out of the generated YAML frontmatter.
        let yaml = content
            .strip_prefix("---\n")
            .and_then(|s| s.split_once("\n---\n"))
            .map(|(fm, _)| fm)
            .expect("expected YAML frontmatter");

        let fm: Frontmatter = serde_yml::from_str(yaml).expect("expected value");
        let tools = fm.tools.expect("expected tools list");

        let mut sorted = tools.clone();
        sorted.sort_unstable();
        assert_eq!(
            tools, sorted,
            "tools should be sorted alphabetically in generated output"
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

        let files = adapt_claude(spec, &HashMap::new()).expect("expected value");
        let content = String::from_utf8(files[0].content.clone()).expect("expected value");

        let expected = concat!(
            "---\n",
            "name: test-agent\n",
            "description: Test agent\n",
            "model: null\n",
            "tools: null\n",
            "---\n",
            "\n",
            "Body.",
        );
        assert_eq!(content, expected);
    }
}
