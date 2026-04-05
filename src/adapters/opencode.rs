use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde::Serialize;
use strum::VariantArray as _;
use walkdir::WalkDir;

use crate::compile::{AdapterConfig, GeneratedFile};
use crate::plan::{FileKind, PostWriteHook};
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

/// Post-write hook that patches `opencode.json` instructions with rule file paths.
#[derive(Debug)]
pub struct OpenCodeInstructionsPatch {
    rules_dest_dir: PathBuf,
    config_dir: PathBuf,
}

impl PostWriteHook for OpenCodeInstructionsPatch {
    fn run(&self, dry_run: bool) -> Result<()> {
        patch_opencode_instructions(&self.rules_dest_dir, &self.config_dir, dry_run)
    }
}

pub fn post_write_hook(
    kind: FileKind,
    dest: &Path,
    config_dir: &Path,
) -> Option<Box<dyn PostWriteHook>> {
    if kind != FileKind::Rules {
        return None;
    }
    Some(Box::new(OpenCodeInstructionsPatch {
        rules_dest_dir: dest.to_path_buf(),
        config_dir: config_dir.to_path_buf(),
    }))
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

/// Patches the `instructions` array in `opencode_config_dir/opencode.json`.
///
/// Ownership contract: agentspec owns any entry whose path falls under `rules_dest_dir`.
/// On each sync those entries are replaced wholesale; all other entries are preserved.
///
/// If `opencode.json` does not exist, it is created with just the `instructions` key.
///
/// When `dry_run` is true, prints the planned diff but does not write the file.
fn patch_opencode_instructions(
    rules_dest_dir: &Path,
    opencode_config_dir: &Path,
    dry_run: bool,
) -> Result<()> {
    let config_path = opencode_config_dir.join("opencode.json");

    // Read existing config (or start with empty object)
    let mut config: serde_json::Value = if config_path.exists() {
        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("failed to read {}", config_path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("failed to parse {}", config_path.display()))?
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };

    // Get the existing instructions array (default to [])
    let existing: Vec<String> = config
        .get("instructions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    // Split into user-owned and agentspec-owned
    let user_entries: Vec<String> = existing
        .into_iter()
        .filter(|p| !Path::new(p).starts_with(rules_dest_dir))
        .collect();

    // Enumerate current rule files in rules_dest_dir
    let mut new_rule_paths: Vec<String> = if rules_dest_dir.is_dir() {
        WalkDir::new(rules_dest_dir)
            .min_depth(1)
            .follow_links(true)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file() && e.file_name() == "AGENTS.md")
            .map(|e| e.path().to_string_lossy().into_owned())
            .collect()
    } else {
        Vec::new()
    };
    new_rule_paths.sort();

    let mut updated_instructions = user_entries;
    updated_instructions.extend(new_rule_paths);

    // Skip writing entirely when the file doesn't exist yet and there's nothing to record.
    // This avoids creating a spurious `opencode.json` when no rules have ever been synced.
    if !config_path.exists() && updated_instructions.is_empty() {
        return Ok(());
    }

    if dry_run {
        eprintln!(
            "would write {} instructions to {}",
            updated_instructions.len(),
            config_path.display()
        );
        return Ok(());
    }

    // Update the instructions key
    let instructions_value: Vec<serde_json::Value> = updated_instructions
        .into_iter()
        .map(serde_json::Value::String)
        .collect();
    if let Some(obj) = config.as_object_mut() {
        obj.insert(
            "instructions".to_string(),
            serde_json::Value::Array(instructions_value),
        );
    }

    let content =
        serde_json::to_string_pretty(&config).context("failed to serialize opencode.json")?;
    fs::write(&config_path, format!("{content}\n"))
        .with_context(|| format!("failed to write {}", config_path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

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

    // patch_opencode_instructions tests

    #[test]
    fn test_patch_no_prior_config_creates_file() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir_all(rules_dir.join("my-rule")).expect("expected value");
        fs::write(rules_dir.join("my-rule/AGENTS.md"), "rule").expect("expected value");

        patch_opencode_instructions(&rules_dir, tmp.path(), false).expect("expected value");

        let config_path = tmp.path().join("opencode.json");
        assert!(config_path.exists());
        let content = fs::read_to_string(&config_path).expect("expected value");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("expected value");
        let instructions = parsed["instructions"].as_array().expect("expected array");
        assert_eq!(instructions.len(), 1);
        assert!(
            instructions[0]
                .as_str()
                .expect("expected str")
                .contains("my-rule")
        );
    }

    #[test]
    fn test_patch_preserves_user_entries() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir_all(rules_dir.join("my-rule")).expect("expected value");
        fs::write(rules_dir.join("my-rule/AGENTS.md"), "rule").expect("expected value");

        let config_path = tmp.path().join("opencode.json");
        fs::write(
            &config_path,
            r#"{"instructions": ["/user/custom/AGENTS.md"]}"#,
        )
        .expect("expected value");

        patch_opencode_instructions(&rules_dir, tmp.path(), false).expect("expected value");

        let content = fs::read_to_string(&config_path).expect("expected value");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("expected value");
        let instructions = parsed["instructions"].as_array().expect("expected array");
        let paths: Vec<&str> = instructions
            .iter()
            .map(|v| v.as_str().expect("expected str"))
            .collect();
        assert!(
            paths.contains(&"/user/custom/AGENTS.md"),
            "user entry preserved"
        );
        assert!(
            paths.iter().any(|p| p.contains("my-rule")),
            "agentspec entry added"
        );
    }

    #[test]
    fn test_patch_replaces_stale_agentspec_entries() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir_all(rules_dir.join("new-rule")).expect("expected value");
        fs::write(rules_dir.join("new-rule/AGENTS.md"), "rule").expect("expected value");

        let config_path = tmp.path().join("opencode.json");
        let stale_path = rules_dir.join("old-rule/AGENTS.md");
        let existing = serde_json::json!({
            "instructions": [
                stale_path.to_string_lossy(),
                "/user/AGENTS.md"
            ]
        });
        fs::write(
            &config_path,
            serde_json::to_string(&existing).expect("expected value"),
        )
        .expect("expected value");

        patch_opencode_instructions(&rules_dir, tmp.path(), false).expect("expected value");

        let content = fs::read_to_string(&config_path).expect("expected value");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("expected value");
        let instructions = parsed["instructions"].as_array().expect("expected array");
        let paths: Vec<&str> = instructions
            .iter()
            .map(|v| v.as_str().expect("expected str"))
            .collect();
        assert!(
            !paths.iter().any(|p| p.contains("old-rule")),
            "stale entry removed"
        );
        assert!(
            paths.iter().any(|p| p.contains("new-rule")),
            "new entry present"
        );
        assert!(paths.contains(&"/user/AGENTS.md"), "user entry preserved");
    }

    #[test]
    fn test_patch_empty_rules_dir_removes_agentspec_entries() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir_all(&rules_dir).expect("expected value");

        let config_path = tmp.path().join("opencode.json");
        let stale_path = rules_dir.join("old-rule/AGENTS.md");
        let existing = serde_json::json!({
            "instructions": [stale_path.to_string_lossy(), "/user/AGENTS.md"]
        });
        fs::write(
            &config_path,
            serde_json::to_string(&existing).expect("expected value"),
        )
        .expect("expected value");

        patch_opencode_instructions(&rules_dir, tmp.path(), false).expect("expected value");

        let content = fs::read_to_string(&config_path).expect("expected value");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("expected value");
        let instructions = parsed["instructions"].as_array().expect("expected array");
        assert_eq!(instructions.len(), 1);
        assert_eq!(
            instructions[0].as_str().expect("expected str"),
            "/user/AGENTS.md"
        );
    }

    #[test]
    fn test_patch_dry_run_no_file_written() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");

        patch_opencode_instructions(&rules_dir, tmp.path(), true).expect("expected value");

        assert!(
            !tmp.path().join("opencode.json").exists(),
            "dry_run must not create file"
        );
    }
}
