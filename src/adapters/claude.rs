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
    name: Option<String>,
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

    let model = spec
        .frontmatter
        .execution
        .and_then(|x| x.preset)
        .and_then(|x| presets.get(&x))
        .and_then(|x| x.claude.clone())
        .and_then(|x| x.model);

    let tools: Option<Vec<ClaudeTool>> = spec
        .frontmatter
        .capabilities
        .and_then(|x| x.tools)
        .map(|tool_specs| -> Result<Vec<ClaudeTool>> {
            // Sort by serialized name — the value that appears in generated files.
            let mut keyed: Vec<(String, ClaudeTool)> = tool_specs
                .iter()
                .flat_map(adapt_tool)
                .map(|t| Ok((serde_yml::to_string(&t)?, t)))
                .collect::<Result<_>>()?;
            keyed.sort_by(|(a, _), (b, _)| a.cmp(b));
            Ok(keyed.into_iter().map(|(_, t)| t).collect())
        })
        .transpose()?;

    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let path = Path::new("agents").join(format!("{file_prefix}{id}.md"));

    // Claude agents get frontmatter name prefix with ":" delimiter
    let name = match cfg.and_then(|c| c.prefix.as_deref()) {
        Some(prefix) => format!("{prefix}:{id}"),
        None => id,
    };

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
    cfg: Option<&AdapterConfig>,
) -> Result<Vec<GeneratedFile>> {
    let id = spec.frontmatter.id;
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

    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let skill_dir = Path::new("skills").join(format!("{file_prefix}{id}"));

    // Claude skills: strip name entirely, prefix with ":" delimiter, or plain
    let name = match cfg {
        Some(c) if c.strip_name => None,
        Some(c) => Some(match c.prefix.as_deref() {
            Some(prefix) => format!("{prefix}:{id}"),
            None => id.clone(),
        }),
        None => Some(id.clone()),
    };

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

fn adapt_rule_spec(spec: &NormalizedRuleSpec, cfg: Option<&AdapterConfig>) -> Vec<GeneratedFile> {
    let content = format!("{}\n", spec.body.trim()).into_bytes();
    let file_prefix = cfg.and_then(AdapterConfig::file_prefix).unwrap_or_default();
    let path = Path::new("rules").join(format!("{file_prefix}{}.md", spec.frontmatter.id));

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

pub fn post_write_hook(
    _kind: FileKind,
    _dest: &Path,
    _config_dir: &Path,
) -> Option<Box<dyn PostWriteHook>> {
    None
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde::Deserialize;

    use super::*;
    use crate::spec::{
        CapabilitiesFrontmatter, NormalizedAgentFrontmatter, NormalizedAgentSpec,
        NormalizedRuleFrontmatter, NormalizedRuleSpec, NormalizedSkillFrontmatter,
        NormalizedSkillSpec,
    };

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

        let files = adapt_claude(spec, &HashMap::new(), None).expect("expected value");
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

        let files = adapt_claude(spec, &HashMap::new(), None).expect("expected value");
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

    #[test]
    fn test_adapt_agent_with_prefix() {
        let cfg = AdapterConfig {
            prefix: Some("tw".to_string()),
            strip_name: false,
        };
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

        let files = adapt_claude(spec, &HashMap::new(), Some(&cfg)).expect("expected value");
        assert_eq!(files[0].path.to_str(), Some("agents/tw-test-agent.md"));

        let content = String::from_utf8(files[0].content.clone()).expect("expected value");
        assert!(
            content.contains("name: tw:test-agent"),
            "frontmatter should contain prefixed name, got: {content}"
        );
    }

    #[test]
    fn test_adapt_skill_with_strip_name() {
        let cfg = AdapterConfig {
            prefix: None,
            strip_name: true,
        };
        let spec = NormalizedSpec::Skill(NormalizedSkillSpec {
            path: "test.md".into(),
            frontmatter: NormalizedSkillFrontmatter {
                id: "test-skill".to_string(),
                description: Some("A test skill".to_string()),
                execution: None,
                capabilities: None,
                user_invocable: true,
                agent_invocable: true,
            },
            body: "Body.".to_string(),
            supporting_files: vec![],
        });

        let files = adapt_claude(spec, &HashMap::new(), Some(&cfg)).expect("expected value");
        // Path should NOT be prefixed (no prefix configured)
        assert_eq!(files[0].path.to_str(), Some("skills/test-skill/SKILL.md"));

        let content = String::from_utf8(files[0].content.clone()).expect("expected value");
        assert!(
            !content.contains("name:"),
            "frontmatter should not contain name: when strip_name is true, got: {content}"
        );
    }

    #[test]
    fn test_adapt_rule_with_prefix() {
        let cfg = AdapterConfig {
            prefix: Some("tw".to_string()),
            strip_name: false,
        };
        let spec = NormalizedSpec::Rule(NormalizedRuleSpec {
            path: "test.md".into(),
            frontmatter: NormalizedRuleFrontmatter {
                id: "test-rule".to_string(),
                description: Some("A test rule".to_string()),
            },
            body: "Rule body.".to_string(),
        });

        let files = adapt_claude(spec, &HashMap::new(), Some(&cfg)).expect("expected value");
        assert_eq!(files[0].path.to_str(), Some("rules/tw-test-rule.md"));
    }
}
