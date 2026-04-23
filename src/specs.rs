use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use gray_matter::Matter;
use gray_matter::engine::YAML;
use walkdir::WalkDir;

use crate::presets::ProviderPresetsMap;
use crate::spec::{
    AgentFrontmatter, AgentSpec, NormalizedSpec, RuleFrontmatter, RuleSpec, SkillFrontmatter,
    SkillSpec, Spec, SupportingFile,
};
use crate::validate::{SemanticError, normalize_specs, validate_semantics};

// ---------------------------------------------------------------------------
// Pipeline stage types
// ---------------------------------------------------------------------------

/// Compiled set of ignore glob patterns, matched against paths relative to
/// the `sources_dir` anchor.
///
/// Patterns are structural globs (see the `globset` crate): `*`, `**`, `?`,
/// character classes, and brace expansion are supported; gitignore negation
/// and directory-trailing-slash sugar are not. Slashless patterns match only
/// top-level entries — `*.bats` does not match `skills/s/test.bats`; users
/// must write `**/*.bats` to match at any depth.
///
/// [`IgnoreMatcher::empty`] constructs a matcher with no patterns — the
/// no-op case.
#[derive(Debug)]
pub struct IgnoreMatcher {
    set: GlobSet,
    patterns: Vec<String>,
}

impl IgnoreMatcher {
    /// A matcher with no patterns — matches nothing.
    pub fn empty() -> Self {
        Self {
            set: GlobSet::empty(),
            patterns: Vec::new(),
        }
    }

    /// Compile a list of glob patterns. Returns an error identifying the
    /// first malformed pattern.
    ///
    /// Globs are compiled with `literal_separator(true)` so that `*` and `?`
    /// never match `/` — slashless patterns like `*.bats` only match at the
    /// top level, and users must write `**/*.bats` to match at any depth.
    pub fn compile(patterns: &[String]) -> Result<Self> {
        let mut builder = GlobSetBuilder::new();
        for pat in patterns {
            let glob = GlobBuilder::new(pat)
                .literal_separator(true)
                .build()
                .with_context(|| format!("invalid ignore pattern '{pat}'"))?;
            builder.add(glob);
        }
        let set = builder.build().context("failed to build ignore glob set")?;
        Ok(Self {
            set,
            patterns: patterns.to_vec(),
        })
    }

    /// Returns `true` when the matcher holds no patterns (the no-op case).
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Number of compiled patterns.
    pub fn len(&self) -> usize {
        self.patterns.len()
    }

    /// Returns the lowest index of a matching pattern, if any.
    pub fn matching_index(&self, rel_path: &Path) -> Option<usize> {
        self.set.matches(rel_path).into_iter().next()
    }

    /// Returns the raw pattern source at `index`, or `None` if out of bounds.
    pub fn pattern(&self, index: usize) -> Option<&str> {
        self.patterns.get(index).map(String::as_str)
    }

    /// All compiled pattern sources, in the order they were supplied.
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }
}

/// Directories from which specs are loaded.
///
/// Constructing this from an `AgentspecConfig` is the binary's responsibility;
/// the spec pipeline has no dependency on the config format.
pub struct SpecDirs {
    pub agents: PathBuf,
    pub skills: PathBuf,
    pub rules: PathBuf,
}

/// Stage 1: specs loaded from disk.
///
/// Advance to [`NormalizedSpecs`] by calling [`Specs::normalize`].
pub struct Specs {
    specs: Vec<Spec>,
}

impl Specs {
    /// Load all agent, skill, and rule specs from the given directories.
    pub fn load(dirs: &SpecDirs) -> Result<Self> {
        let specs = load_specs_from_dirs(&dirs.agents, &dirs.skills, &dirs.rules)?;
        Ok(Self { specs })
    }

    /// Apply defaults and convert raw frontmatter to fully-typed normalized structs.
    pub fn normalize(self) -> NormalizedSpecs {
        let specs = normalize_specs(self.specs);
        NormalizedSpecs { specs }
    }
}

/// Stage 2: frontmatter is normalized; ready for semantic validation.
///
/// Advance to [`ValidatedSpecs`] by calling [`NormalizedSpecs::validate`].
pub struct NormalizedSpecs {
    specs: Vec<NormalizedSpec>,
}

impl NormalizedSpecs {
    /// Run semantic checks (duplicate IDs, unknown presets, etc.).
    ///
    /// Returns `Err(errors)` listing every violation found so the caller can
    /// format and report them; returns `Ok(ValidatedSpecs)` if all checks pass.
    pub fn validate(
        self,
        presets: &ProviderPresetsMap,
    ) -> Result<ValidatedSpecs, Vec<SemanticError>> {
        let errors = validate_semantics(&self.specs, presets);
        if errors.is_empty() {
            Ok(ValidatedSpecs { specs: self.specs })
        } else {
            Err(errors)
        }
    }

    /// Access the normalized specs without advancing the stage.
    pub fn specs(&self) -> &[NormalizedSpec] {
        &self.specs
    }
}

/// Stage 3: all checks passed; ready for compilation.
///
/// Pass to [`compile::run`](crate::compile::run), which handles template
/// resolution internally before dispatching to provider adapters.
pub struct ValidatedSpecs {
    specs: Vec<NormalizedSpec>,
}

impl ValidatedSpecs {
    /// Consume self and return the inner specs.
    ///
    /// Used by the templating module to take ownership of the validated data.
    pub fn into_specs(self) -> Vec<NormalizedSpec> {
        self.specs
    }

    /// Access the validated specs directly (e.g. for the `validate` command).
    pub fn specs(&self) -> &[NormalizedSpec] {
        &self.specs
    }
}

// ---------------------------------------------------------------------------
// Spec loading
// ---------------------------------------------------------------------------

fn load_specs_from_dirs(
    agents_dir: &Path,
    skills_dir: &Path,
    rules_dir: &Path,
) -> Result<Vec<Spec>> {
    let mut specs = load_agent_specs(agents_dir)?;
    specs.extend(load_skill_specs(skills_dir)?);
    specs.extend(load_rule_specs(rules_dir)?);
    Ok(specs)
}

fn load_agent_specs(dir: &Path) -> Result<Vec<Spec>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut md_paths: Vec<_> = WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|ext| ext == "md"))
        .map(walkdir::DirEntry::into_path)
        .collect();
    md_paths.sort();

    let matter = Matter::<YAML>::new();

    let mut specs = Vec::new();

    for path in md_paths {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;

        let parsed = matter
            .parse::<AgentFrontmatter>(&content)
            .with_context(|| format!("failed to parse frontmatter in {}", path.display()))?;
        let frontmatter = parsed
            .data
            .ok_or_else(|| anyhow!("missing spec for {}", path.display()))?;
        let body = parsed.content;

        specs.push(Spec::Agent(AgentSpec {
            path,
            frontmatter,
            body,
        }));
    }

    Ok(specs)
}

fn load_skill_specs(dir: &Path) -> Result<Vec<Spec>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut skill_dirs: Vec<_> = fs::read_dir(dir)
        .with_context(|| format!("failed to read {}", dir.display()))?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    skill_dirs.sort();

    let matter = Matter::<YAML>::new();

    let mut specs = Vec::new();

    for skill_dir in skill_dirs {
        let entries: Vec<_> = fs::read_dir(&skill_dir)
            .with_context(|| format!("failed to read {}", skill_dir.display()))?
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
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
        let md_path_display = md_path.display().to_string();

        let content = fs::read_to_string(&md_path)
            .with_context(|| format!("failed to read {md_path_display}"))?;

        let parsed = matter
            .parse::<SkillFrontmatter>(&content)
            .with_context(|| format!("failed to parse frontmatter in {md_path_display}"))?;
        let frontmatter = parsed
            .data
            .ok_or_else(|| anyhow!("missing frontmatter for {md_path_display}"))?;
        let body = parsed.content;

        // Collect supporting files (non-.md files anywhere under the skill directory).
        // WalkDir recurses into subdirectories (e.g., scripts/), preserving the path
        // relative to the skill root so adapters emit the correct nested layout.
        let mut supporting_files = Vec::new();
        for entry in WalkDir::new(&skill_dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
        {
            let entry_path = entry.path();

            // Skip the spec file itself
            if entry_path == md_path.as_path() {
                continue;
            }

            let Ok(relative_path) = entry_path.strip_prefix(&skill_dir) else {
                continue;
            };
            let relative_path = relative_path.to_path_buf();

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

        specs.push(Spec::Skill(SkillSpec {
            path: md_path,
            frontmatter,
            body,
            supporting_files,
        }));
    }

    Ok(specs)
}

fn load_rule_specs(dir: &Path) -> Result<Vec<Spec>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut md_paths: Vec<_> = WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|ext| ext == "md"))
        .map(walkdir::DirEntry::into_path)
        .collect();
    md_paths.sort();

    let matter = Matter::<YAML>::new();

    let mut specs = Vec::new();

    for path in md_paths {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;

        let parsed = matter
            .parse::<RuleFrontmatter>(&content)
            .with_context(|| format!("failed to parse frontmatter in {}", path.display()))?;
        let frontmatter = parsed
            .data
            .ok_or_else(|| anyhow!("missing spec for {}", path.display()))?;
        let body = parsed.content;

        specs.push(Spec::Rule(RuleSpec {
            path,
            frontmatter,
            body,
        }));
    }

    Ok(specs)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn test_load_agent_specs() {
        let tmp = tempfile::tempdir().expect("expected value");
        let agents_dir = tmp.path().join("agents");
        fs::create_dir(&agents_dir).expect("expected value");

        let spec_content = "---\nid: test-agent\ndescription: A test\n---\nAgent body here.\n";
        fs::write(agents_dir.join("test-agent.md"), spec_content).expect("expected value");

        let specs = load_agent_specs(&agents_dir).expect("expected value");
        assert_eq!(specs.len(), 1);
        let Spec::Agent(ref s) = specs[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(s.frontmatter.id, "test-agent");
        assert_eq!(s.frontmatter.description, "A test");
        assert_eq!(s.body, "Agent body here.");
    }

    #[test]
    fn test_load_agent_specs_with_tags() {
        let tmp = tempfile::tempdir().expect("expected value");
        let agents_dir = tmp.path().join("agents");
        fs::create_dir(&agents_dir).expect("expected value");

        let spec_content = "---\nid: tagged\ndescription: A tagged agent\ntags:\n  - research\n  - codebase\n---\nBody.\n";
        fs::write(agents_dir.join("tagged.md"), spec_content).expect("expected value");

        let specs = load_agent_specs(&agents_dir).expect("expected value");
        let Spec::Agent(ref s) = specs[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(
            s.frontmatter.tags.as_deref(),
            Some(["research".to_string(), "codebase".to_string()].as_slice())
        );
    }

    #[test]
    fn test_load_agent_specs_parse_error_includes_file_path() {
        let tmp = tempfile::tempdir().expect("expected value");
        let agents_dir = tmp.path().join("agents");
        fs::create_dir(&agents_dir).expect("expected value");

        let spec_content = r"---
id: bad-agent
description: Broken tools
capabilities:
  tools:
    - ls
---
Agent body.
";
        let spec_path = agents_dir.join("bad-agent.md");
        fs::write(&spec_path, spec_content).expect("expected value");

        let err = load_agent_specs(&agents_dir).expect_err("expected parse error");
        let full = format!("{err:#}");
        assert!(
            full.contains("failed to parse frontmatter in"),
            "error: {full}"
        );
        assert!(full.contains("bad-agent.md"), "error: {full}");
    }

    #[test]
    fn test_load_skill_specs_with_supporting_file() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("my-skill");
        fs::create_dir_all(&skill_dir).expect("expected value");

        let spec_content = "---\nid: my-skill\ndescription: A test skill\nuser_invocable: true\nagent_invocable: false\n---\nSkill body.\n";
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
        let Spec::Skill(ref s) = specs[0] else {
            panic!("expected Skill variant")
        };
        assert_eq!(s.frontmatter.id, "my-skill");
        assert_eq!(s.supporting_files.len(), 1);
        assert_eq!(
            s.supporting_files[0].relative_path,
            std::path::PathBuf::from("scripts/helper.sh")
        );
        assert!(s.supporting_files[0].executable);
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

    // -----------------------------------------------------------------------
    // IgnoreMatcher tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_ignore_matcher_empty_matches_nothing() {
        let matcher = IgnoreMatcher::empty();
        assert!(matcher.is_empty());
        assert_eq!(matcher.len(), 0);
        assert_eq!(matcher.matching_index(Path::new("anything")), None);
        assert_eq!(
            matcher.matching_index(Path::new("deeply/nested/file.bats")),
            None,
        );
    }

    #[test]
    fn test_ignore_matcher_double_star_matches_at_any_depth() {
        let matcher = IgnoreMatcher::compile(&["**/*.bats".to_string()]).expect("expected value");
        assert_eq!(
            matcher.matching_index(Path::new("skills/s/test.bats")),
            Some(0),
        );
        assert_eq!(matcher.matching_index(Path::new("skills/s/test.md")), None);
    }

    #[test]
    fn test_ignore_matcher_slashless_pattern_is_top_level_only() {
        let matcher = IgnoreMatcher::compile(&["*.bats".to_string()]).expect("expected value");
        assert_eq!(matcher.matching_index(Path::new("test.bats")), Some(0));
        assert_eq!(
            matcher.matching_index(Path::new("skills/s/test.bats")),
            None
        );
    }

    #[test]
    fn test_ignore_matcher_compile_error_names_offending_pattern() {
        let err = IgnoreMatcher::compile(&["[".to_string()]).expect_err("expected parse error");
        let full = format!("{err:#}");
        assert!(full.contains("invalid ignore pattern"), "error: {full}");
        assert!(full.contains("'['"), "error: {full}");
    }

    #[test]
    fn test_ignore_matcher_pattern_accessor() {
        let patterns = vec!["**/*.bats".to_string(), "**/fixtures/**".to_string()];
        let matcher = IgnoreMatcher::compile(&patterns).expect("expected value");
        assert_eq!(matcher.len(), 2);
        assert_eq!(matcher.pattern(0), Some("**/*.bats"));
        assert_eq!(matcher.pattern(1), Some("**/fixtures/**"));
        assert_eq!(matcher.pattern(2), None);
        assert_eq!(matcher.patterns(), patterns.as_slice());
    }

    #[test]
    fn test_ignore_matcher_returns_lowest_matching_index() {
        // Two patterns that both match the same path.
        let matcher = IgnoreMatcher::compile(&["**/*.bats".to_string(), "skills/**".to_string()])
            .expect("expected value");
        // "skills/s/test.bats" matches both; lowest index wins.
        assert_eq!(
            matcher.matching_index(Path::new("skills/s/test.bats")),
            Some(0),
        );
    }
}
