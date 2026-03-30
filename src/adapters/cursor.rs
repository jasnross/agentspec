use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::compile::GeneratedFile;
use crate::presets::ProviderPresetsMap;
use crate::provider::Provider;
use crate::spec::{NormalizedAgentSpec, NormalizedRuleSpec, NormalizedSkillSpec, NormalizedSpec};

// See: https://cursor.com/docs/subagents#configuration-fields
#[derive(Serialize)]
struct CursorAgentFrontmatter {
    name: String,
    description: String,
    model: Option<String>,
}

// See: https://cursor.com/docs/skills#frontmatter-fields
#[derive(Serialize)]
struct CursorSkillFrontmatter {
    name: String,
    description: String,
}

// See: https://cursor.com/docs/rules#rule-file-format
#[derive(Serialize)]
struct CursorRuleFrontmatter {
    description: String,
    #[serde(rename = "alwaysApply")]
    always_apply: bool,
}

pub fn adapt_cursor(
    spec: NormalizedSpec,
    presets: &ProviderPresetsMap,
) -> Result<Vec<GeneratedFile>> {
    match spec {
        NormalizedSpec::Agent(s) => adapt_agent_spec(s, presets),
        NormalizedSpec::Skill(s) => adapt_skill_spec(s),
        NormalizedSpec::Rule(s) => adapt_rule_spec(s),
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
        .and_then(|x| x.cursor.clone())
        .and_then(|x| x.model);

    let path = Path::new("generated")
        .join("cursor")
        .join("agents")
        .join(format!("{name}.md"));

    let frontmatter = CursorAgentFrontmatter {
        name,
        description,
        model,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}\n---\n\n{body}").into_bytes();

    Ok(vec![GeneratedFile {
        provider: Provider::Cursor,
        path,
        content,
        mode: None,
    }])
}

fn adapt_skill_spec(spec: NormalizedSkillSpec) -> Result<Vec<GeneratedFile>> {
    let id = spec.frontmatter.id;
    let description = spec.frontmatter.description.unwrap_or_default();

    let frontmatter = CursorSkillFrontmatter {
        name: id.clone(),
        description,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}\n---\n\n{body}").into_bytes();

    let skill_dir = Path::new("generated")
        .join("cursor")
        .join("skills")
        .join(&id);

    let mut files = vec![GeneratedFile {
        provider: Provider::Cursor,
        path: skill_dir.join("SKILL.md"),
        content,
        mode: None,
    }];

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

fn adapt_rule_spec(spec: NormalizedRuleSpec) -> Result<Vec<GeneratedFile>> {
    let description = spec.frontmatter.description.unwrap_or_default();

    let frontmatter = CursorRuleFrontmatter {
        description,
        always_apply: true,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}\n---\n\n{body}").into_bytes();

    let path = Path::new("generated")
        .join("cursor")
        .join("rules")
        .join(format!("{}.mdc", spec.frontmatter.id));

    Ok(vec![GeneratedFile {
        provider: Provider::Cursor,
        path,
        content,
        mode: None,
    }])
}
