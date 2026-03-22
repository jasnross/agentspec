use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result, bail};
use walkdir::WalkDir;

use crate::config::AgentspecConfig;
use crate::types::{CanonicalSpec, SpecKind, SupportingFile};

/// Split a Markdown file into its YAML frontmatter block and body.
///
/// Expects the file to start with `---\n`, followed by YAML, then `---\n`,
/// then the body. Returns `(yaml_block, body)`.
///
/// Leading whitespace before the opening `---` is tolerated (matching gray-matter behavior).
pub fn split_frontmatter(content: &str) -> Result<(String, String)> {
    let trimmed = content.trim_start();

    // Opening delimiter must be exactly `---` followed by newline (not `----` or `---text`)
    if !trimmed.starts_with("---") {
        bail!("missing opening frontmatter delimiter (---)");
    }
    let after_dashes = &trimmed[3..];
    if !(after_dashes.starts_with('\n')
        || after_dashes.starts_with("\r\n")
        || after_dashes.is_empty())
    {
        bail!("opening frontmatter delimiter must be exactly '---' on its own line");
    }
    let after_open = after_dashes
        .strip_prefix('\n')
        .or_else(|| after_dashes.strip_prefix("\r\n"))
        .unwrap_or(after_dashes);

    // Find the closing `---` — must be on its own line (preceded by newline, followed by
    // newline or EOF). This prevents matching `\n----` or `\n---text`.
    let mut search_from = 0;
    let (yaml_end, delim_end) = loop {
        let newline_pos = match after_open[search_from..].find('\n') {
            Some(p) => search_from + p,
            None => bail!("missing closing frontmatter delimiter (---)"),
        };

        let line_start = newline_pos + 1;
        let rest = &after_open[line_start..];

        // Check if line after newline is exactly `---` (not `----` or `---text`)
        if let Some(after) = rest.strip_prefix("---")
            && (after.is_empty() || after.starts_with('\n') || after.starts_with("\r\n"))
        {
            break (newline_pos, line_start + 3);
        }

        search_from = line_start;
    };

    let yaml_block = after_open[..yaml_end].to_string();

    // Body starts after the closing delimiter line
    let rest = &after_open[delim_end..];
    let body = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))
        .unwrap_or(rest);

    Ok((yaml_block, body.to_string()))
}

/// Parse a YAML string into a `serde_json::Value`, preserving the raw structure
/// for later schema validation.
pub fn parse_yaml_to_json(yaml: &str) -> Result<serde_json::Value> {
    let yaml_value: serde_yml::Value =
        serde_yml::from_str(yaml).context("failed to parse YAML frontmatter")?;
    let json_value = serde_json::to_value(yaml_value).context("failed to convert YAML to JSON")?;
    Ok(json_value)
}

/// Load agent specs from a directory. Walks recursively for `*.md` files.
pub fn load_agent_specs(agents_dir: &Path) -> Result<Vec<CanonicalSpec>> {
    if !agents_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut specs = Vec::new();

    let mut md_paths: Vec<_> = WalkDir::new(agents_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|ext| ext == "md"))
        .map(|e| e.into_path())
        .collect();
    md_paths.sort();

    for path in md_paths {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let (yaml_block, body) = split_frontmatter(&content)
            .with_context(|| format!("failed to parse frontmatter in {}", path.display()))?;
        let fm = parse_yaml_to_json(&yaml_block)
            .with_context(|| format!("invalid YAML in {}", path.display()))?;

        specs.push(CanonicalSpec {
            path,
            fm,
            body,
            kind: SpecKind::Agent,
            supporting_files: Vec::new(),
        });
    }

    Ok(specs)
}

/// Load skill specs from a directory. Each skill is a subdirectory containing
/// exactly one `*.md` file and zero or more supporting files.
pub fn load_skill_specs(skills_dir: &Path) -> Result<Vec<CanonicalSpec>> {
    if !skills_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut skill_dirs: Vec<_> = fs::read_dir(skills_dir)
        .with_context(|| format!("failed to read {}", skills_dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    skill_dirs.sort();

    let mut specs = Vec::new();

    for skill_dir in skill_dirs {
        let entries: Vec<_> = fs::read_dir(&skill_dir)
            .with_context(|| format!("failed to read {}", skill_dir.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .collect();

        let md_files: Vec<_> = entries
            .iter()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
            .collect();

        if md_files.is_empty() {
            bail!(
                "skill directory {} contains no .md file",
                skill_dir.display()
            );
        }
        if md_files.len() > 1 {
            bail!(
                "skill directory {} contains multiple .md files (expected exactly one)",
                skill_dir.display()
            );
        }

        let md_path = md_files[0].path();
        let content = fs::read_to_string(&md_path)
            .with_context(|| format!("failed to read {}", md_path.display()))?;
        let (yaml_block, body) = split_frontmatter(&content)
            .with_context(|| format!("failed to parse frontmatter in {}", md_path.display()))?;
        let fm = parse_yaml_to_json(&yaml_block)
            .with_context(|| format!("invalid YAML in {}", md_path.display()))?;

        // Collect supporting files (non-.md files anywhere under the skill directory).
        // WalkDir recurses into subdirectories (e.g., scripts/), preserving the path
        // relative to the skill root so adapters emit the correct nested layout.
        let mut supporting_files = Vec::new();
        for entry in WalkDir::new(&skill_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let entry_path = entry.path();
            // Skip SKILL.md itself and any other .md files (instructional content,
            // not supporting files)
            if entry_path.extension().is_some_and(|ext| ext == "md") {
                continue;
            }
            let relative_path = entry_path
                .strip_prefix(&skill_dir)
                .expect("WalkDir entry must be under skill_dir")
                .to_path_buf();
            let file_content = fs::read(entry_path)
                .with_context(|| format!("failed to read {}", entry_path.display()))?;
            let metadata = fs::metadata(entry_path)
                .with_context(|| format!("failed to stat {}", entry_path.display()))?;
            let executable = metadata.permissions().mode() & 0o111 != 0;

            supporting_files.push(SupportingFile {
                relative_path,
                content: file_content,
                executable,
            });
        }
        supporting_files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

        specs.push(CanonicalSpec {
            path: md_path,
            fm,
            body,
            kind: SpecKind::Skill,
            supporting_files,
        });
    }

    Ok(specs)
}

/// Load all canonical specs (agents + skills) from the config paths.
pub fn load_canonical_specs(config: &AgentspecConfig) -> Result<Vec<CanonicalSpec>> {
    let agents_dir = config.resolve(&config.spec.agents_dir);
    let skills_dir = config.resolve(&config.spec.skills_dir);

    let mut agents = load_agent_specs(&agents_dir).context("failed to load agent specs")?;
    let skills = load_skill_specs(&skills_dir).context("failed to load skill specs")?;

    // Agents sorted by path (already sorted in load_agent_specs)
    // Skills sorted by directory name (already sorted in load_skill_specs)
    // Combine: agents first, then skills
    agents.extend(skills);
    Ok(agents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn test_split_frontmatter_valid() {
        let content = "---\nid: test\nkind: agent\n---\nThis is the body.\n";
        let (yaml, body) = split_frontmatter(content).expect("expected value");
        assert_eq!(yaml, "id: test\nkind: agent");
        assert_eq!(body, "This is the body.\n");
    }

    #[test]
    fn test_split_frontmatter_empty_body() {
        let content = "---\nid: test\n---\n";
        let (yaml, body) = split_frontmatter(content).expect("expected value");
        assert_eq!(yaml, "id: test");
        assert_eq!(body, "");
    }

    #[test]
    fn test_split_frontmatter_missing_open() {
        let content = "no frontmatter here";
        assert!(split_frontmatter(content).is_err());
    }

    #[test]
    fn test_split_frontmatter_missing_close() {
        let content = "---\nid: test\nno closing delimiter";
        assert!(split_frontmatter(content).is_err());
    }

    #[test]
    fn test_split_frontmatter_rejects_four_dashes_open() {
        let content = "----\nid: test\n---\nbody";
        assert!(split_frontmatter(content).is_err());
    }

    #[test]
    fn test_split_frontmatter_rejects_four_dashes_close() {
        // `----` as closing delimiter should not match
        let content = "---\nid: test\n----\nbody\n---\nreal body";
        let (yaml, body) = split_frontmatter(content).expect("expected value");
        // The YAML block should include `----` and `body` since they're not valid closers
        assert!(yaml.contains("----"));
        assert_eq!(body, "real body");
    }

    #[test]
    fn test_split_frontmatter_rejects_dashes_with_text() {
        // `---text` as closing delimiter should not match
        let content = "---\nid: test\n---not-a-delimiter\nstill yaml\n---\nreal body";
        let (yaml, body) = split_frontmatter(content).expect("expected value");
        assert!(yaml.contains("---not-a-delimiter"));
        assert_eq!(body, "real body");
    }

    #[test]
    fn test_split_frontmatter_closing_at_eof() {
        // Closing `---` at end of file (no trailing newline)
        let content = "---\nid: test\n---";
        let (yaml, body) = split_frontmatter(content).expect("expected value");
        assert_eq!(yaml, "id: test");
        assert_eq!(body, "");
    }

    #[test]
    fn test_parse_yaml_to_json() {
        let yaml = "id: my-agent\nkind: agent\nversion: 1";
        let value = parse_yaml_to_json(yaml).expect("expected value");
        assert_eq!(value["id"], "my-agent");
        assert_eq!(value["kind"], "agent");
        assert_eq!(value["version"], 1);
    }

    #[test]
    fn test_load_agent_specs() {
        let tmp = tempfile::tempdir().expect("expected value");
        let agents_dir = tmp.path().join("agents");
        fs::create_dir(&agents_dir).expect("expected value");

        let spec_content = "---\nid: test-agent\nkind: agent\ndescription: A test\nversion: 1\n---\nAgent body here.\n";
        fs::write(agents_dir.join("test-agent.md"), spec_content).expect("expected value");

        let specs = load_agent_specs(&agents_dir).expect("expected value");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].kind, SpecKind::Agent);
        assert_eq!(specs[0].fm["id"], "test-agent");
        assert_eq!(specs[0].body, "Agent body here.\n");
        assert!(specs[0].supporting_files.is_empty());
    }

    #[test]
    fn test_load_skill_specs_with_supporting_file() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("my-skill");
        fs::create_dir_all(&skill_dir).expect("expected value");

        let spec_content = "---\nid: my-skill\nkind: skill\ndescription: A test skill\nversion: 1\n---\nSkill body.\n";
        fs::write(skill_dir.join("SKILL.md"), spec_content).expect("expected value");

        // Create a supporting script file inside scripts/ subdirectory
        let scripts_dir = skill_dir.join("scripts");
        fs::create_dir(&scripts_dir).expect("expected value");
        let script_path = scripts_dir.join("helper.sh");
        fs::write(&script_path, "#!/bin/bash\necho hello").expect("expected value");
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755))
            .expect("expected value");

        let specs = load_skill_specs(&skills_dir).expect("expected value");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].kind, SpecKind::Skill);
        assert_eq!(specs[0].fm["id"], "my-skill");
        assert_eq!(specs[0].supporting_files.len(), 1);
        assert_eq!(
            specs[0].supporting_files[0].relative_path,
            std::path::PathBuf::from("scripts/helper.sh")
        );
        assert!(specs[0].supporting_files[0].executable);
    }

    #[test]
    fn test_load_skill_specs_no_md_file() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("empty-skill");
        fs::create_dir_all(&skill_dir).expect("expected value");
        fs::write(skill_dir.join("readme.txt"), "not a spec").expect("expected value");

        let result = load_skill_specs(&skills_dir);
        assert!(result.is_err());
        assert!(
            result
                .expect_err("expected error")
                .to_string()
                .contains("no .md file")
        );
    }

    #[test]
    fn test_load_skill_specs_multiple_md_files() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("multi-md");
        fs::create_dir_all(&skill_dir).expect("expected value");
        fs::write(skill_dir.join("SKILL.md"), "---\nid: a\n---\nbody").expect("expected value");
        fs::write(skill_dir.join("OTHER.md"), "---\nid: b\n---\nbody").expect("expected value");

        let result = load_skill_specs(&skills_dir);
        assert!(result.is_err());
        assert!(
            result
                .expect_err("expected error")
                .to_string()
                .contains("multiple .md files")
        );
    }

    #[test]
    fn test_nonexistent_dir_returns_empty() {
        let tmp = tempfile::tempdir().expect("expected value");
        let specs = load_agent_specs(&tmp.path().join("nonexistent")).expect("expected value");
        assert!(specs.is_empty());
    }
}
