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
#[serde(rename_all = "camelCase")]
struct CursorRuleFrontmatter {
    description: String,
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

    let path = Path::new("agents").join(format!("{name}.md"));

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

fn adapt_skill_spec(spec: NormalizedSkillSpec) -> Result<Vec<GeneratedFile>> {
    let id = spec.frontmatter.id;
    let description = spec.frontmatter.description.unwrap_or_default();

    let frontmatter = CursorSkillFrontmatter {
        name: id.clone(),
        description,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}---\n\n{body}");

    let skill_dir = Path::new("skills").join(&id);

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

fn adapt_rule_spec(spec: NormalizedRuleSpec) -> Result<Vec<GeneratedFile>> {
    let description = spec.frontmatter.description.unwrap_or_default();

    let frontmatter = CursorRuleFrontmatter {
        description,
        always_apply: true,
    };

    let frontmatter_str = serde_yml::to_string(&frontmatter)?;
    let body = spec.body.trim();
    let content = format!("---\n{frontmatter_str}---\n\n{body}");

    let path = Path::new("rules").join(format!("{}.mdc", spec.frontmatter.id));

    Ok(vec![GeneratedFile::text(Provider::Cursor, path, content)])
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::spec::{NormalizedAgentFrontmatter, NormalizedAgentSpec};

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

        let files = adapt_cursor(spec, &HashMap::new()).expect("expected value");
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
}
