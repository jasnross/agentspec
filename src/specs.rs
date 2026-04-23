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
/// [`SpecDirs::ignore_anchor`].
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
    /// Compiled ignore patterns applied to every file walked during load.
    pub ignore: IgnoreMatcher,
    /// Absolute `sources_dir`. Ignore patterns are matched against paths
    /// made relative to this directory.
    pub ignore_anchor: PathBuf,
}

/// A single file or directory that the load stage filtered out.
#[derive(Clone, Debug)]
pub struct IgnoredPath {
    /// Path relative to [`SpecDirs::ignore_anchor`].
    pub rel_path: PathBuf,
    /// Index into [`IgnoreMatcher::patterns`].
    pub pattern_index: usize,
    /// `true` when the path is a directory whose subtree was pruned entirely.
    pub pruned: bool,
}

/// Diagnostic data produced by [`Specs::load`].
///
/// Records which paths were filtered and which patterns matched nothing,
/// so the caller can surface warnings and listings without having to
/// re-walk the spec tree.
#[derive(Debug, Default)]
pub struct LoadReport {
    pub ignored: Vec<IgnoredPath>,
    /// Per-pattern hit counts (index-aligned with [`IgnoreMatcher::patterns`]).
    pub pattern_hits: Vec<u32>,
}

impl LoadReport {
    /// Construct a report sized for `matcher`, with zero hits recorded.
    pub fn with_matcher(matcher: &IgnoreMatcher) -> Self {
        Self {
            ignored: Vec::new(),
            pattern_hits: vec![0; matcher.len()],
        }
    }

    /// Record a single ignored path and bump its pattern's hit count.
    pub fn record(&mut self, rel_path: PathBuf, pattern_index: usize, pruned: bool) {
        self.ignored.push(IgnoredPath {
            rel_path,
            pattern_index,
            pruned,
        });
        if let Some(hit) = self.pattern_hits.get_mut(pattern_index) {
            *hit = hit.saturating_add(1);
        }
    }

    /// Indices of patterns that matched zero files.
    pub fn unused_pattern_indices(&self) -> Vec<usize> {
        self.pattern_hits
            .iter()
            .enumerate()
            .filter_map(|(i, &n)| (n == 0).then_some(i))
            .collect()
    }
}

/// Path-based variant of the `filter_entry` ignore check.
///
/// Returns `true` when the caller should skip the entry (and, for directories,
/// prune the subtree). Records the match in `report`. `is_dir` distinguishes
/// a file match from a directory prune; the distinction surfaces in the
/// ignored-path listing.
///
/// A directory path matches when the path itself matches any pattern *or*
/// a synthetic child under it would match — so a user-supplied pattern like
/// `skills/deploy/**` prunes the `skills/deploy` subtree even though the
/// directory path itself lacks a child component for `**` to bind to.
fn should_ignore_path(
    path: &Path,
    is_dir: bool,
    anchor: &Path,
    ignore: &IgnoreMatcher,
    report: &mut LoadReport,
) -> bool {
    if ignore.is_empty() {
        return false;
    }
    let Ok(rel) = path.strip_prefix(anchor) else {
        return false;
    };
    if let Some(idx) = ignore.matching_index(rel) {
        report.record(rel.to_path_buf(), idx, is_dir);
        return true;
    }
    // For directories, also check whether any child under this dir would
    // match — that lets `foo/**` prune the `foo` subtree entirely.
    if is_dir {
        let probe = rel.join("__agentspec_ignore_probe__");
        if let Some(idx) = ignore.matching_index(&probe) {
            report.record(rel.to_path_buf(), idx, true);
            return true;
        }
    }
    false
}

/// `walkdir::DirEntry` adapter around [`should_ignore_path`].
fn should_ignore_entry(
    entry: &walkdir::DirEntry,
    anchor: &Path,
    ignore: &IgnoreMatcher,
    report: &mut LoadReport,
) -> bool {
    should_ignore_path(
        entry.path(),
        entry.file_type().is_dir(),
        anchor,
        ignore,
        report,
    )
}

/// Stage 1: specs loaded from disk.
///
/// Advance to [`NormalizedSpecs`] by calling [`Specs::normalize`].
pub struct Specs {
    specs: Vec<Spec>,
}

impl Specs {
    /// Load all agent, skill, and rule specs from the given directories.
    ///
    /// Returns the loaded specs alongside a [`LoadReport`] that records which
    /// files (and subtrees) were skipped by `dirs.ignore` and which patterns
    /// matched nothing. The report is produced here because [`Specs::normalize`]
    /// consumes `self` — the diagnostic data can't live on a later stage.
    pub fn load(dirs: &SpecDirs) -> Result<(Self, LoadReport)> {
        let mut report = LoadReport::with_matcher(&dirs.ignore);
        let specs = load_specs_from_dirs(dirs, &mut report)?;
        Ok((Self { specs }, report))
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

fn load_specs_from_dirs(dirs: &SpecDirs, report: &mut LoadReport) -> Result<Vec<Spec>> {
    let mut specs = load_agent_specs(&dirs.agents, &dirs.ignore, &dirs.ignore_anchor, report)?;
    specs.extend(load_skill_specs(
        &dirs.skills,
        &dirs.ignore,
        &dirs.ignore_anchor,
        report,
    )?);
    specs.extend(load_rule_specs(
        &dirs.rules,
        &dirs.ignore,
        &dirs.ignore_anchor,
        report,
    )?);
    Ok(specs)
}

fn load_agent_specs(
    dir: &Path,
    ignore: &IgnoreMatcher,
    anchor: &Path,
    report: &mut LoadReport,
) -> Result<Vec<Spec>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut md_paths: Vec<_> = WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| !should_ignore_entry(e, anchor, ignore, report))
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

fn load_skill_specs(
    dir: &Path,
    ignore: &IgnoreMatcher,
    anchor: &Path,
    report: &mut LoadReport,
) -> Result<Vec<Spec>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    // Prune the skills root itself if a pattern covers it (mirrors the
    // behavior of `WalkDir::new(dir).filter_entry(...)` in the agent/rule
    // loaders — they prune the root when e.g. `agents` or `rules` is ignored).
    if should_ignore_path(dir, true, anchor, ignore, report) {
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
        // Top-level prune: if the whole skill directory matches ignore, skip
        // it entirely (neither read its contents nor report inner entries).
        if should_ignore_path(&skill_dir, true, anchor, ignore, report) {
            continue;
        }

        if let Some(spec) = load_single_skill(&skill_dir, &matter, ignore, anchor, report)? {
            specs.push(spec);
        }
    }

    Ok(specs)
}

/// Load a single skill directory. Returns `Ok(None)` when the skill is
/// entirely skipped because its `.md` was ignored.
fn load_single_skill(
    skill_dir: &Path,
    matter: &Matter<YAML>,
    ignore: &IgnoreMatcher,
    anchor: &Path,
    report: &mut LoadReport,
) -> Result<Option<Spec>> {
    let entries: Vec<_> = fs::read_dir(skill_dir)
        .with_context(|| format!("failed to read {}", skill_dir.display()))?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .collect();

    let Some(md_path) = select_spec_md(skill_dir, &entries, ignore, anchor, report)? else {
        return Ok(None);
    };
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
    for entry in WalkDir::new(skill_dir)
        .into_iter()
        .filter_entry(|e| !should_ignore_entry(e, anchor, ignore, report))
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let entry_path = entry.path();

        // Skip the spec file itself
        if entry_path == md_path.as_path() {
            continue;
        }

        let Ok(relative_path) = entry_path.strip_prefix(skill_dir) else {
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

    Ok(Some(Spec::Skill(SkillSpec {
        path: md_path,
        frontmatter,
        body,
        supporting_files,
    })))
}

/// Pick the single `.md` file that becomes the skill spec.
///
/// Returns `Ok(None)` when every `.md` in the directory was ignored (the
/// skill is absent from the pipeline). Records those ignored `.md` files
/// in `report` exactly once — the caller's subsequent `WalkDir` pass won't
/// run in this case.
///
/// Uses a non-recording `matching_index` lookup for the `.md` filter to
/// avoid double-counting when the skill proceeds (the `WalkDir` pass would
/// otherwise record the same ignored `.md` a second time).
fn select_spec_md(
    skill_dir: &Path,
    entries: &[fs::DirEntry],
    ignore: &IgnoreMatcher,
    anchor: &Path,
    report: &mut LoadReport,
) -> Result<Option<PathBuf>> {
    let any_md_present = entries
        .iter()
        .any(|e| e.path().extension().is_some_and(|ext| ext == "md"));

    let ignore_match_index = |path: &Path| -> Option<usize> {
        path.strip_prefix(anchor)
            .ok()
            .and_then(|rel| ignore.matching_index(rel))
    };

    let md_files: Vec<_> = entries
        .iter()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
        .filter(|e| ignore_match_index(&e.path()).is_none())
        .collect();

    if md_files.is_empty() {
        if any_md_present {
            // All .md files in this skill were ignored — record them once
            // (the WalkDir pass in the caller won't run) and signal absent.
            for e in entries {
                let path = e.path();
                if path.extension().is_some_and(|ext| ext == "md")
                    && let Some(idx) = ignore_match_index(&path)
                    && let Ok(rel) = path.strip_prefix(anchor)
                {
                    report.record(rel.to_path_buf(), idx, false);
                }
            }
            return Ok(None);
        }
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

    Ok(Some(md_files[0].path()))
}

fn load_rule_specs(
    dir: &Path,
    ignore: &IgnoreMatcher,
    anchor: &Path,
    report: &mut LoadReport,
) -> Result<Vec<Spec>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut md_paths: Vec<_> = WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| !should_ignore_entry(e, anchor, ignore, report))
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

    /// Load agents with no ignore patterns, rooted at `dir` as the anchor.
    fn load_agents_no_ignore(dir: &Path) -> Result<Vec<Spec>> {
        let mut report = LoadReport::default();
        load_agent_specs(dir, &IgnoreMatcher::empty(), dir, &mut report)
    }

    /// Load skills with no ignore patterns, rooted at `dir` as the anchor.
    fn load_skills_no_ignore(dir: &Path) -> Result<Vec<Spec>> {
        let mut report = LoadReport::default();
        load_skill_specs(dir, &IgnoreMatcher::empty(), dir, &mut report)
    }

    #[test]
    fn test_load_agent_specs() {
        let tmp = tempfile::tempdir().expect("expected value");
        let agents_dir = tmp.path().join("agents");
        fs::create_dir(&agents_dir).expect("expected value");

        let spec_content = "---\nid: test-agent\ndescription: A test\n---\nAgent body here.\n";
        fs::write(agents_dir.join("test-agent.md"), spec_content).expect("expected value");

        let specs = load_agents_no_ignore(&agents_dir).expect("expected value");
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

        let specs = load_agents_no_ignore(&agents_dir).expect("expected value");
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

        let err = load_agents_no_ignore(&agents_dir).expect_err("expected parse error");
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

        let specs = load_skills_no_ignore(&skills_dir).expect("expected value");
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

        let result = load_skills_no_ignore(&skills_dir);
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

        let result = load_skills_no_ignore(&skills_dir);
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
        let specs = load_agents_no_ignore(&tmp.path().join("nonexistent")).expect("expected value");
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

    // -----------------------------------------------------------------------
    // Load-stage filtering tests
    // -----------------------------------------------------------------------

    /// Write an agent spec file at `<dir>/<name>.md` with `id = name`.
    fn write_agent_md(dir: &Path, name: &str) {
        let content = format!("---\nid: {name}\ndescription: test\n---\nbody.\n");
        fs::write(dir.join(format!("{name}.md")), content).expect("expected value");
    }

    #[test]
    fn test_should_ignore_path_empty_matcher_short_circuits() {
        let tmp = tempfile::tempdir().expect("expected value");
        let anchor = tmp.path();
        let ignore = IgnoreMatcher::empty();
        let mut report = LoadReport::with_matcher(&ignore);

        assert!(!should_ignore_path(
            &anchor.join("agents/a.md"),
            false,
            anchor,
            &ignore,
            &mut report,
        ));
        assert!(report.ignored.is_empty());
    }

    #[test]
    fn test_load_agent_specs_skips_ignored_file() {
        let tmp = tempfile::tempdir().expect("expected value");
        let agents_dir = tmp.path().join("agents");
        fs::create_dir(&agents_dir).expect("expected value");
        write_agent_md(&agents_dir, "kept");
        write_agent_md(&agents_dir, "ignored");

        let patterns = vec!["agents/ignored.md".to_string()];
        let ignore = IgnoreMatcher::compile(&patterns).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs = load_agent_specs(&agents_dir, &ignore, tmp.path(), &mut report)
            .expect("expected value");

        let ids: Vec<&str> = specs
            .iter()
            .map(|s| match s {
                Spec::Agent(a) => a.frontmatter.id.as_str(),
                Spec::Skill(_) | Spec::Rule(_) => panic!("expected Agent variant"),
            })
            .collect();
        assert_eq!(ids, vec!["kept"]);
        assert_eq!(report.ignored.len(), 1);
        assert_eq!(
            report.ignored[0].rel_path,
            PathBuf::from("agents/ignored.md")
        );
        assert!(!report.ignored[0].pruned);
        assert_eq!(report.pattern_hits, vec![1]);
    }

    #[test]
    fn test_load_skill_specs_skips_ignored_supporting_file() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("s");
        fs::create_dir_all(&skill_dir).expect("expected value");

        let spec_content = "---\nid: s\ndescription: skill\nuser_invocable: true\nagent_invocable: false\n---\nbody.\n";
        fs::write(skill_dir.join("SKILL.md"), spec_content).expect("expected value");
        fs::write(skill_dir.join("helper.sh"), "#!/bin/sh\n").expect("expected value");
        fs::write(skill_dir.join("test.bats"), "bats test").expect("expected value");

        let patterns = vec!["**/*.bats".to_string()];
        let ignore = IgnoreMatcher::compile(&patterns).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs = load_skill_specs(&skills_dir, &ignore, tmp.path(), &mut report)
            .expect("expected value");

        assert_eq!(specs.len(), 1);
        let Spec::Skill(ref s) = specs[0] else {
            panic!("expected Skill variant")
        };
        let supporting_paths: Vec<_> = s
            .supporting_files
            .iter()
            .map(|f| f.relative_path.clone())
            .collect();
        assert_eq!(supporting_paths, vec![PathBuf::from("helper.sh")]);
        assert_eq!(report.ignored.len(), 1);
        assert_eq!(report.pattern_hits, vec![1]);
    }

    #[test]
    fn test_load_skill_specs_prunes_ignored_skill_dir() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let kept_dir = skills_dir.join("kept");
        let deploy_dir = skills_dir.join("deploy");
        fs::create_dir_all(&kept_dir).expect("expected value");
        fs::create_dir_all(&deploy_dir).expect("expected value");

        fs::write(
            kept_dir.join("SKILL.md"),
            "---\nid: kept\ndescription: s\nuser_invocable: true\nagent_invocable: false\n---\nbody.\n",
        )
        .expect("expected value");
        fs::write(
            deploy_dir.join("SKILL.md"),
            "---\nid: deploy\ndescription: s\nuser_invocable: true\nagent_invocable: false\n---\nbody.\n",
        )
        .expect("expected value");
        fs::write(deploy_dir.join("helper.sh"), "#!/bin/sh\n").expect("expected value");

        let patterns = vec!["skills/deploy/**".to_string()];
        let ignore = IgnoreMatcher::compile(&patterns).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs = load_skill_specs(&skills_dir, &ignore, tmp.path(), &mut report)
            .expect("expected value");

        let ids: Vec<&str> = specs
            .iter()
            .map(|s| match s {
                Spec::Skill(sk) => sk.frontmatter.id.as_str(),
                Spec::Agent(_) | Spec::Rule(_) => panic!("expected Skill variant"),
            })
            .collect();
        assert_eq!(ids, vec!["kept"]);
        // Whole dir pruned — exactly one report entry with pruned=true.
        assert_eq!(report.ignored.len(), 1);
        assert!(report.ignored[0].pruned);
        assert_eq!(report.ignored[0].rel_path, PathBuf::from("skills/deploy"));
    }

    #[test]
    fn test_load_skill_specs_whole_skill_absent_when_md_ignored() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("hidden");
        fs::create_dir_all(&skill_dir).expect("expected value");

        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nid: hidden\ndescription: s\nuser_invocable: true\nagent_invocable: false\n---\nbody.\n",
        )
        .expect("expected value");
        // Non-ignored non-.md file in the skill — ensures we're not hitting
        // the "pruned subtree" path, just the .md-ignored path.
        fs::write(skill_dir.join("helper.sh"), "#!/bin/sh\n").expect("expected value");

        let patterns = vec!["skills/hidden/SKILL.md".to_string()];
        let ignore = IgnoreMatcher::compile(&patterns).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs = load_skill_specs(&skills_dir, &ignore, tmp.path(), &mut report)
            .expect("expected value");

        assert!(specs.is_empty());
        assert_eq!(report.pattern_hits, vec![1]);
    }

    #[test]
    fn test_load_skill_specs_truly_missing_md_still_errors() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("empty");
        fs::create_dir_all(&skill_dir).expect("expected value");
        fs::write(skill_dir.join("readme.txt"), "nope").expect("expected value");

        // No ignore pattern — the .md really doesn't exist.
        let ignore = IgnoreMatcher::empty();
        let mut report = LoadReport::default();
        let err = load_skill_specs(&skills_dir, &ignore, tmp.path(), &mut report)
            .expect_err("expected error");
        assert!(err.to_string().contains("no .md file"), "error: {err}");
    }

    #[test]
    fn test_load_skill_specs_ignored_extra_md_recorded_exactly_once() {
        // Regression: a skill dir with two .md files where one is ignored
        // used to be recorded twice (pre-filter + WalkDir supporting-file
        // pass). After the fix, the ignored .md appears in `report.ignored`
        // exactly once, and `pattern_hits == [1]`.
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("s");
        fs::create_dir_all(&skill_dir).expect("expected value");

        // The legitimate spec file.
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nid: s\ndescription: s\nuser_invocable: true\nagent_invocable: false\n---\nbody.\n",
        )
        .expect("expected value");
        // Another .md colocated in the dir, which the user wants to ignore.
        fs::write(skill_dir.join("extra.md"), "# notes").expect("expected value");

        let patterns = vec!["**/extra.md".to_string()];
        let ignore = IgnoreMatcher::compile(&patterns).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs = load_skill_specs(&skills_dir, &ignore, tmp.path(), &mut report)
            .expect("expected value");

        // Skill loads normally — only one surviving `.md` after the ignore filter.
        assert_eq!(specs.len(), 1);
        // The ignored `.md` is recorded exactly once.
        assert_eq!(report.ignored.len(), 1);
        assert_eq!(
            report.ignored[0].rel_path,
            PathBuf::from("skills/s/extra.md")
        );
        assert_eq!(report.pattern_hits, vec![1]);
    }

    #[test]
    fn test_load_skill_specs_prunes_skills_root() {
        // `skills` as an ignore pattern prunes the whole skills root (parity
        // with how `agents` / `rules` prune their roots via WalkDir).
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("s");
        fs::create_dir_all(&skill_dir).expect("expected value");

        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nid: s\ndescription: s\nuser_invocable: true\nagent_invocable: false\n---\nbody.\n",
        )
        .expect("expected value");

        let patterns = vec!["skills".to_string()];
        let ignore = IgnoreMatcher::compile(&patterns).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs = load_skill_specs(&skills_dir, &ignore, tmp.path(), &mut report)
            .expect("expected value");

        assert!(specs.is_empty());
        assert_eq!(report.ignored.len(), 1);
        assert!(report.ignored[0].pruned);
        assert_eq!(report.ignored[0].rel_path, PathBuf::from("skills"));
    }

    #[test]
    fn test_load_report_pattern_hits_zero_for_unused_pattern() {
        let tmp = tempfile::tempdir().expect("expected value");
        let agents_dir = tmp.path().join("agents");
        fs::create_dir(&agents_dir).expect("expected value");
        write_agent_md(&agents_dir, "kept");

        let patterns = vec!["**/*.bats".to_string(), "**/never-matches".to_string()];
        let ignore = IgnoreMatcher::compile(&patterns).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        load_agent_specs(&agents_dir, &ignore, tmp.path(), &mut report).expect("expected value");

        // No `.bats` nor `never-matches` file exists — both should be 0.
        assert_eq!(report.pattern_hits, vec![0, 0]);
        assert_eq!(report.unused_pattern_indices(), vec![0, 1]);
    }
}
