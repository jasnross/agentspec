use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use gray_matter::Matter;
use gray_matter::engine::YAML;
use indexmap::IndexMap;
use serde::Deserialize;
use walkdir::WalkDir;

use crate::presets::ProviderPresetsMap;
use crate::spec::{
    AgentFrontmatter, AgentSpec, HookFrontmatter, HookSpec, RuleFrontmatter, RuleSpec,
    SkillFrontmatter, SkillSpec, Spec, SupportingFile,
};
use crate::validate::{ValidationError, validate_semantics};

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
    /// Directory containing `hooks.toml` and the `scripts/` subdirectory.
    /// Absent directory is not an error — hook authoring is opt-in.
    pub hooks: PathBuf,
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

fn validate_in_tree_symlink(entry: &walkdir::DirEntry, anchor: &Path) -> Result<()> {
    if !entry.path_is_symlink() {
        return Ok(());
    }
    let source = entry.path().display();
    let canonical_target = fs::canonicalize(entry.path())
        .with_context(|| format!("failed to resolve symlink {source}"))?;
    let canonical_anchor = fs::canonicalize(anchor)
        .with_context(|| format!("failed to canonicalize spec root {}", anchor.display()))?;
    if !canonical_target.starts_with(&canonical_anchor) {
        let literal_display = fs::read_link(entry.path())
            .ok()
            .map_or_else(|| "<unreadable>".to_string(), |p| p.display().to_string());
        let target_display = canonical_target.display();
        let anchor_display = canonical_anchor.display();
        bail!(
            "{source}: symlink target {literal_display} resolves to {target_display}, \
             which is outside the spec tree at {anchor_display}"
        );
    }
    Ok(())
}

/// Stage 1: specs loaded from disk.
///
/// Advance to [`ValidatedSpecs`] by calling [`Specs::validate`].
pub struct Specs {
    specs: Vec<Spec>,
}

impl Specs {
    /// Load all agent, skill, and rule specs from the given directories.
    ///
    /// Returns the loaded specs alongside a [`LoadReport`] that records which
    /// files (and subtrees) were skipped by `dirs.ignore` and which patterns
    /// matched nothing. The report is produced here because [`Specs::validate`]
    /// consumes `self` — the diagnostic data can't live on a later stage.
    pub fn load(dirs: &SpecDirs) -> Result<(Self, LoadReport)> {
        let mut report = LoadReport::with_matcher(&dirs.ignore);
        let specs = load_specs_from_dirs(dirs, &mut report)?;
        Ok((Self { specs }, report))
    }

    /// Run semantic checks (duplicate IDs, unknown presets, etc.).
    ///
    /// Returns `Err(errors)` listing every violation found so the caller can
    /// format and report them; returns `Ok(ValidatedSpecs)` if all checks pass.
    pub fn validate(
        self,
        presets: &ProviderPresetsMap,
        config_path: &Path,
    ) -> Result<ValidatedSpecs, Vec<ValidationError>> {
        let errors = validate_semantics(&self.specs, presets, config_path);
        if errors.is_empty() {
            Ok(ValidatedSpecs {
                specs: self.specs,
                presets: presets.clone(),
            })
        } else {
            Err(errors)
        }
    }
}

/// Stage 2: all checks passed; ready for compilation.
///
/// Pass to [`compile::run`](crate::compile::run), which handles template
/// resolution internally before dispatching to provider adapters.
pub struct ValidatedSpecs {
    specs: Vec<Spec>,
    /// The preset map these specs were validated against.
    ///
    /// Carried rather than re-supplied at compile time so `compile::run` cannot
    /// be handed a map that never passed [`Specs::validate`]. Taking it as a
    /// separate parameter let a caller validate one map and compile with
    /// another.
    ///
    /// This closes the `compile::run` path only. `Provider::adapter()` and
    /// `Adapter::compile` are public, so a consumer invoking an adapter directly
    /// still supplies its own `CompileCtx.presets` and is guarded only by the
    /// adapter's `debug_assert!`s.
    presets: ProviderPresetsMap,
}

impl ValidatedSpecs {
    /// Consume self and return the inner specs.
    ///
    /// Used by the templating module to take ownership of the validated data.
    pub fn into_specs(self) -> Vec<Spec> {
        self.specs
    }

    /// The preset map these specs were validated against.
    pub fn presets(&self) -> &ProviderPresetsMap {
        &self.presets
    }

    /// Access the validated specs directly (e.g. for the `validate` command).
    pub fn specs(&self) -> &[Spec] {
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
    specs.extend(load_hook_specs(
        &dirs.hooks,
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

    let mut supporting_files = IndexMap::new();
    let walker = WalkDir::new(skill_dir)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| !should_ignore_entry(e, anchor, ignore, report));
    for result in walker {
        let entry = match result {
            Ok(e) => e,
            Err(e) => {
                if let Some(ancestor) = e.loop_ancestor() {
                    let source = e
                        .path()
                        .map_or_else(|| "<unknown>".to_string(), |p| p.display().to_string());
                    bail!(
                        "{source}: symlink loop detected (cycles back to {})",
                        ancestor.display()
                    );
                }
                if e.io_error()
                    .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
                {
                    let source = e
                        .path()
                        .map_or_else(|| "<unknown>".to_string(), |p| p.display().to_string());
                    bail!("{source}: symlink target does not exist");
                }
                return Err(e).with_context(|| format!("error walking {}", skill_dir.display()));
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let entry_path = entry.path();

        if entry_path == md_path.as_path() {
            continue;
        }

        validate_in_tree_symlink(&entry, anchor)?;

        let Ok(relative_path) = entry_path.strip_prefix(skill_dir) else {
            continue;
        };
        let relative_path = relative_path.to_path_buf();

        let file_content = fs::read(entry_path)
            .with_context(|| format!("failed to read {}", entry_path.display()))?;
        let metadata = entry
            .metadata()
            .with_context(|| format!("failed to stat {}", entry_path.display()))?;
        let mode = metadata.permissions().mode() & 0o0777;

        supporting_files.insert(
            relative_path,
            SupportingFile {
                content: file_content,
                mode,
            },
        );
    }
    supporting_files.sort_keys();

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
        // Colocated .md content: prefer SKILL.md as the primary spec file.
        let skill_md = md_files.iter().find(|e| {
            e.file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("skill.md")
        });
        if let Some(entry) = skill_md {
            return Ok(Some(entry.path()));
        }
        bail!(
            "skill directory {} contains multiple .md files and none is named SKILL.md",
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

/// On-disk shape of `hooks.toml`.
///
/// Authors write `[hooks.<id>]` tables; the outer `hooks` map's keys are
/// captured into [`HookFrontmatter::id`] after deserialization. Using
/// `IndexMap` preserves authoring order, which propagates through to the
/// emitted `hooks.json` group ordering.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HookSpecFile {
    #[serde(default)]
    hooks: IndexMap<String, HookFrontmatter>,
}

/// Validate a hook id against the bare-key regex `^[a-z][a-z0-9_-]*$`.
///
/// Hooks share the spec-id namespace with agents/skills/rules; the same
/// kebab-case convention applies. Empty ids are rejected.
fn validate_hook_id(id: &str) -> Result<()> {
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        bail!("hook id is empty");
    };
    if !first.is_ascii_lowercase() {
        bail!("hook id '{id}' must start with a lowercase letter (allowed: a-z, then a-z0-9_-)");
    }
    for ch in chars {
        if !(ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_') {
            bail!("hook id '{id}' contains invalid character '{ch}' (allowed: a-z, 0-9, -, _)");
        }
    }
    Ok(())
}

/// Loads hook specs from a single `hooks.toml`, with each `[hooks.<id>]` table
/// becoming one [`Spec::Hook`]. Walks `scripts/` once and attaches the full
/// file list to every emitted spec — emission is deduplicated downstream by
/// emitting from a single provider-level synthesis pass.
///
/// Behavior:
/// - Returns `Ok(empty)` when `dir` does not exist (hook authoring is opt-in).
/// - Errors when `hooks.toml` is absent but `scripts/` exists (orphaned scripts).
/// - Errors when a script under `scripts/` starts with `_agentspec_` (reserved).
/// - Errors when `frontmatter.script` escapes the hooks dir or does not resolve to a file.
/// - Returns `Ok(empty)` when `hooks.toml` is absent and no `scripts/` exists.
fn load_hook_specs(
    dir: &Path,
    ignore: &IgnoreMatcher,
    anchor: &Path,
    report: &mut LoadReport,
) -> Result<Vec<Spec>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    if should_ignore_path(dir, true, anchor, ignore, report) {
        return Ok(Vec::new());
    }

    let toml_path = dir.join("hooks.toml");
    let scripts_dir = dir.join("scripts");

    if !toml_path.is_file() {
        if scripts_dir.is_dir() {
            bail!(
                "{} exists but {} is missing — orphaned scripts (add hooks.toml or remove the directory)",
                scripts_dir.display(),
                toml_path.display()
            );
        }
        return Ok(Vec::new());
    }

    // Walk `scripts/` once: enforce the `_agentspec_*` reserved-prefix rule
    // and collect every non-ignored file as a `SupportingFile`. The same list
    // is attached to every `HookSpec` produced from this `hooks.toml` — emission
    // happens once per provider in `synthesize_hooks`, not once per hook.
    let supporting_files = collect_hook_scripts(&scripts_dir, ignore, anchor, report)?;

    let content = fs::read_to_string(&toml_path)
        .with_context(|| format!("failed to read {}", toml_path.display()))?;

    let parsed: HookSpecFile = serde_path_to_error::deserialize(toml::de::Deserializer::new(
        &content,
    ))
    .map_err(|error| {
        let path = error.path().to_string();
        let location = if path.is_empty() { "<root>" } else { &path };
        anyhow!(
            "failed to parse {} at `{location}`: {}",
            toml_path.display(),
            error.into_inner()
        )
    })?;

    let mut specs = Vec::new();
    for (id, mut frontmatter) in parsed.hooks {
        validate_hook_id(&id).with_context(|| format!("in {}", toml_path.display()))?;
        validate_hook_script_path(&id, &frontmatter.script, dir, &toml_path)?;
        let script_path = dir.join(&frontmatter.script);
        if !script_path.is_file() {
            bail!(
                "hook '{id}' in {} references script {} which does not exist",
                toml_path.display(),
                script_path.display()
            );
        }
        if frontmatter.events.is_empty() {
            bail!(
                "hook '{id}' in {} has an empty `events` list; at least one event is required",
                toml_path.display()
            );
        }
        let mut seen_events = Vec::with_capacity(frontmatter.events.len());
        for event in &frontmatter.events {
            if seen_events.contains(event) {
                bail!(
                    "hook '{id}' in {} lists event '{}' more than once",
                    toml_path.display(),
                    event.snake_case()
                );
            }
            seen_events.push(*event);
        }
        frontmatter.id = id;
        specs.push(Spec::Hook(HookSpec {
            path: toml_path.clone(),
            frontmatter,
            body: String::new(),
            supporting_files: supporting_files.clone(),
        }));
    }

    Ok(specs)
}

/// Reject `frontmatter.script` paths that escape the hooks directory.
///
/// Without this, `script = "../../etc/passwd"` would pull arbitrary files into
/// `generated/<provider>/hooks/scripts/<basename>` once `adapt_hook_spec` reads
/// them. Component-level rejection (`..`, absolute paths, root prefixes) is
/// sufficient and avoids touching the filesystem at validate time.
fn validate_hook_script_path(
    id: &str,
    script: &Path,
    hooks_dir: &Path,
    toml_path: &Path,
) -> Result<()> {
    use std::path::Component;
    if script.is_absolute() {
        bail!(
            "hook '{id}' in {}: script {} must be a relative path under the hooks directory",
            toml_path.display(),
            script.display()
        );
    }
    for component in script.components() {
        match component {
            Component::ParentDir => bail!(
                "hook '{id}' in {}: script {} escapes the hooks directory ({}/scripts/) via `..`",
                toml_path.display(),
                script.display(),
                hooks_dir.display()
            ),
            Component::RootDir | Component::Prefix(_) => bail!(
                "hook '{id}' in {}: script {} must be relative",
                toml_path.display(),
                script.display()
            ),
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    // Require the first meaningful component to be `scripts`. `collect_hook_scripts`
    // only walks `<hooks_dir>/scripts/`, and the per-provider hook-command anchor
    // builder formats commands as `${ANCHOR}/hooks/scripts/<rel>`. A script outside
    // `scripts/` (e.g., `init.sh` at `spec/hooks/init.sh`) would silently produce
    // a hook entry pointing at a never-emitted file.
    let first = script
        .components()
        .find(|c| !matches!(c, Component::CurDir))
        .and_then(|c| match c {
            Component::Normal(s) => s.to_str(),
            // Other variants are rejected by the loop above; reaching them
            // here is impossible given prior validation, but be explicit
            // rather than wildcard-matching.
            Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_)
            | Component::CurDir => None,
        });
    if first != Some("scripts") {
        let suggested = script.file_name().map_or_else(
            || "<name>.sh".to_string(),
            |f| f.to_string_lossy().into_owned(),
        );
        bail!(
            "hook '{id}' in {}: script {} must live under `scripts/` (e.g., `scripts/{suggested}`)",
            toml_path.display(),
            script.display()
        );
    }
    Ok(())
}

/// Walk the `scripts/` subtree (one walk; respects `[spec].ignore`), enforce
/// the `_agentspec_*` reserved-prefix rule, and return the collected files.
fn collect_hook_scripts(
    scripts_dir: &Path,
    ignore: &IgnoreMatcher,
    anchor: &Path,
    report: &mut LoadReport,
) -> Result<IndexMap<PathBuf, SupportingFile>> {
    if !scripts_dir.is_dir() {
        return Ok(IndexMap::new());
    }

    let hooks_dir = scripts_dir.parent().unwrap_or(scripts_dir);
    let mut files = IndexMap::new();
    let walker = WalkDir::new(scripts_dir)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| !should_ignore_entry(e, anchor, ignore, report));
    for result in walker {
        let entry = match result {
            Ok(e) => e,
            Err(e) => {
                if let Some(ancestor) = e.loop_ancestor() {
                    let source = e
                        .path()
                        .map_or_else(|| "<unknown>".to_string(), |p| p.display().to_string());
                    bail!(
                        "{source}: symlink loop detected (cycles back to {})",
                        ancestor.display()
                    );
                }
                if e.io_error()
                    .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
                {
                    let source = e
                        .path()
                        .map_or_else(|| "<unknown>".to_string(), |p| p.display().to_string());
                    bail!("{source}: symlink target does not exist");
                }
                return Err(e).with_context(|| format!("error walking {}", scripts_dir.display()));
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }

        validate_in_tree_symlink(&entry, anchor)?;

        if let Ok(rel_under_scripts) = entry.path().strip_prefix(scripts_dir) {
            for component in rel_under_scripts.components() {
                if let std::path::Component::Normal(name) = component
                    && name.to_string_lossy().starts_with("_agentspec_")
                {
                    bail!(
                        "{}: path components starting with `_agentspec_` are reserved for future use; rename `{}`",
                        entry.path().display(),
                        name.to_string_lossy()
                    );
                }
            }
        }
        let entry_path = entry.path();
        let Ok(relative_path) = entry_path.strip_prefix(hooks_dir) else {
            continue;
        };
        let content = fs::read(entry_path)
            .with_context(|| format!("failed to read {}", entry_path.display()))?;
        let metadata = entry
            .metadata()
            .with_context(|| format!("failed to stat {}", entry_path.display()))?;
        let mode = metadata.permissions().mode() & 0o0777;
        files.insert(
            relative_path.to_path_buf(),
            SupportingFile { content, mode },
        );
    }
    files.sort_keys();
    Ok(files)
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
            *s.supporting_files.keys().next().expect("expected value"),
            std::path::PathBuf::from("scripts/helper.sh")
        );
        assert_eq!(
            s.supporting_files
                .values()
                .next()
                .expect("expected value")
                .mode,
            0o755
        );
    }

    #[test]
    fn test_load_skill_specs_preserves_non_executable_mode() {
        // Regression guard for verbatim mode preservation: a deliberately
        // non-executable supporting file (e.g., 0o600 for secrets-style
        // helpers) must round-trip through the loader without collapsing
        // to umask-default. Pre-`mode: u32`, the loader stored only an
        // `executable: bool`, losing the exact mode.
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("my-skill");
        fs::create_dir_all(&skill_dir).expect("expected value");

        let spec_content = "---\nid: my-skill\ndescription: A test skill\nuser_invocable: true\nagent_invocable: false\n---\nSkill body.\n";
        fs::write(skill_dir.join("SKILL.md"), spec_content).expect("expected value");

        let scripts_dir = skill_dir.join("scripts");
        fs::create_dir(&scripts_dir).expect("expected value");
        let secret_path = scripts_dir.join("secret.conf");
        fs::write(&secret_path, "token=redacted").expect("expected value");
        fs::set_permissions(&secret_path, fs::Permissions::from_mode(0o600))
            .expect("expected value");

        let specs = load_skills_no_ignore(&skills_dir).expect("expected value");
        assert_eq!(specs.len(), 1);
        let Spec::Skill(ref s) = specs[0] else {
            panic!("expected Skill variant")
        };
        assert_eq!(s.supporting_files.len(), 1);
        assert_eq!(
            s.supporting_files
                .values()
                .next()
                .expect("expected value")
                .mode,
            0o600
        );
    }

    #[test]
    fn test_load_skill_specs_in_tree_symlink_resolved() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("my-skill");
        let scripts_dir = skill_dir.join("scripts");
        fs::create_dir_all(&scripts_dir).expect("expected value");
        let spec_content = "---\nid: my-skill\ndescription: A test skill\nuser_invocable: true\nagent_invocable: false\n---\nSkill body.\n";
        fs::write(skill_dir.join("SKILL.md"), spec_content).expect("expected value");

        let real = skills_dir.join("shared-helper.sh");
        fs::write(&real, "#!/bin/sh\necho shared").expect("expected value");
        fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).expect("expected value");
        std::os::unix::fs::symlink(&real, scripts_dir.join("helper.sh")).expect("expected value");

        let specs = load_skills_no_ignore(&skills_dir).expect("expected value");
        assert_eq!(specs.len(), 1);
        let Spec::Skill(ref s) = specs[0] else {
            panic!("expected Skill variant")
        };
        assert_eq!(s.supporting_files.len(), 1);
        let file = s.supporting_files.values().next().expect("expected value");
        assert_eq!(file.content, b"#!/bin/sh\necho shared");
        assert_eq!(file.mode, 0o755);
    }

    #[test]
    fn test_load_skill_specs_out_of_tree_symlink_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("my-skill");
        fs::create_dir_all(&skill_dir).expect("expected value");
        let spec_content = "---\nid: my-skill\ndescription: A test skill\nuser_invocable: true\nagent_invocable: false\n---\nSkill body.\n";
        fs::write(skill_dir.join("SKILL.md"), spec_content).expect("expected value");

        let outside = tmp.path().join("outside-tree.sh");
        fs::write(&outside, "#!/bin/sh\n").expect("expected value");
        std::os::unix::fs::symlink(&outside, skill_dir.join("helper.sh")).expect("expected value");

        let err = load_skills_no_ignore(&skills_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(full.contains("outside the spec tree"), "error: {full}");
    }

    #[test]
    fn test_load_skill_specs_dangling_symlink_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("my-skill");
        fs::create_dir_all(&skill_dir).expect("expected value");
        let spec_content = "---\nid: my-skill\ndescription: A test skill\nuser_invocable: true\nagent_invocable: false\n---\nSkill body.\n";
        fs::write(skill_dir.join("SKILL.md"), spec_content).expect("expected value");

        std::os::unix::fs::symlink(
            skill_dir.join("nonexistent.sh"),
            skill_dir.join("helper.sh"),
        )
        .expect("expected value");

        let err = load_skills_no_ignore(&skills_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(full.contains("does not exist"), "error: {full}");
    }

    #[test]
    fn test_load_skill_specs_symlink_loop_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("my-skill");
        let scripts_dir = skill_dir.join("scripts");
        fs::create_dir_all(&scripts_dir).expect("expected value");
        let spec_content = "---\nid: my-skill\ndescription: A test skill\nuser_invocable: true\nagent_invocable: false\n---\nSkill body.\n";
        fs::write(skill_dir.join("SKILL.md"), spec_content).expect("expected value");

        std::os::unix::fs::symlink(&skill_dir, scripts_dir.join("loop-back"))
            .expect("expected value");

        let err = load_skills_no_ignore(&skills_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(full.contains("symlink loop"), "error: {full}");
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
    fn test_load_skill_specs_multiple_md_prefers_skill_md() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("multi-md");
        fs::create_dir_all(&skill_dir).expect("expected value");
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nid: a\ndescription: test\nuser_invocable: false\nagent_invocable: false\n---\nbody",
        )
        .expect("expected value");
        fs::write(skill_dir.join("detail.md"), "colocated content").expect("expected value");

        let result = load_skills_no_ignore(&skills_dir).expect("expected value");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id(), "a");
    }

    #[test]
    fn test_load_skill_specs_multiple_md_no_skill_md_errors() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("multi-md");
        fs::create_dir_all(&skill_dir).expect("expected value");
        fs::write(skill_dir.join("one.md"), "---\nid: a\n---\nbody").expect("expected value");
        fs::write(skill_dir.join("two.md"), "---\nid: b\n---\nbody").expect("expected value");

        let err = load_skills_no_ignore(&skills_dir).expect_err("expected error");
        let msg = err.to_string();
        assert!(msg.contains("multiple .md files"), "error: {msg}");
        assert!(msg.contains("SKILL.md"), "error: {msg}");
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
                Spec::Skill(_) | Spec::Rule(_) | Spec::Hook(_) => {
                    panic!("expected Agent variant")
                }
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
        let supporting_paths: Vec<_> = s.supporting_files.keys().cloned().collect();
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
                Spec::Agent(_) | Spec::Rule(_) | Spec::Hook(_) => {
                    panic!("expected Skill variant")
                }
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

    // -----------------------------------------------------------------------
    // Hook loading tests
    // -----------------------------------------------------------------------

    fn write_hook_fixture(dir: &Path, id: &str, event: &str, script_name: &str) {
        let toml =
            format!("[hooks.{id}]\nevents = [\"{event}\"]\nscript = \"scripts/{script_name}\"\n");
        fs::create_dir_all(dir.join("scripts")).expect("expected value");
        fs::write(dir.join("hooks.toml"), toml).expect("expected value");
        fs::write(
            dir.join("scripts").join(script_name),
            "#!/bin/sh\necho hi\n",
        )
        .expect("expected value");
    }

    fn load_hooks_no_ignore(dir: &Path) -> Result<Vec<Spec>> {
        let mut report = LoadReport::default();
        load_hook_specs(dir, &IgnoreMatcher::empty(), dir, &mut report)
    }

    #[test]
    fn test_load_hook_specs_parses_single_hook() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        write_hook_fixture(&hooks_dir, "init", "user_prompt_submit", "init.sh");

        let specs = load_hooks_no_ignore(&hooks_dir).expect("expected value");
        assert_eq!(specs.len(), 1);
        let Spec::Hook(ref h) = specs[0] else {
            panic!("expected Hook variant")
        };
        assert_eq!(h.frontmatter.id, "init");
    }

    #[test]
    fn test_load_hook_specs_preserves_authoring_order() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        fs::create_dir_all(hooks_dir.join("scripts")).expect("expected value");
        // Two hooks in a deliberately non-sorted order in the TOML; IndexMap
        // must preserve insertion order.
        let toml = "
[hooks.zeta]
events = [\"session_start\"]
script = \"scripts/zeta.sh\"

[hooks.alpha]
events = [\"session_end\"]
script = \"scripts/alpha.sh\"
";
        fs::write(hooks_dir.join("hooks.toml"), toml).expect("expected value");
        for name in ["zeta.sh", "alpha.sh"] {
            fs::write(hooks_dir.join("scripts").join(name), "#!/bin/sh\necho hi\n")
                .expect("expected value");
        }

        let specs = load_hooks_no_ignore(&hooks_dir).expect("expected value");
        let ids: Vec<&str> = specs
            .iter()
            .map(|s| match s {
                Spec::Hook(h) => h.frontmatter.id.as_str(),
                Spec::Agent(_) | Spec::Skill(_) | Spec::Rule(_) => {
                    panic!("expected Hook variant")
                }
            })
            .collect();
        assert_eq!(ids, vec!["zeta", "alpha"]);
    }

    #[test]
    fn test_load_hook_specs_missing_toml_with_scripts_errors() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        fs::create_dir_all(hooks_dir.join("scripts")).expect("expected value");
        fs::write(hooks_dir.join("scripts").join("orphan.sh"), "#!/bin/sh\n")
            .expect("expected value");

        let err = load_hooks_no_ignore(&hooks_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(full.contains("orphaned"), "error: {full}");
    }

    #[test]
    fn test_load_hook_specs_no_dir_returns_empty() {
        let tmp = tempfile::tempdir().expect("expected value");
        let specs = load_hooks_no_ignore(&tmp.path().join("nonexistent"))
            .expect("missing dir should be ok");
        assert!(specs.is_empty());
    }

    #[test]
    fn test_load_hook_specs_duplicate_table_header_is_toml_error() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        fs::create_dir_all(hooks_dir.join("scripts")).expect("expected value");
        let toml = "
[hooks.foo]
events = [\"session_start\"]
script = \"scripts/foo.sh\"

[hooks.foo]
events = [\"session_end\"]
script = \"scripts/foo.sh\"
";
        fs::write(hooks_dir.join("hooks.toml"), toml).expect("expected value");
        fs::write(hooks_dir.join("scripts").join("foo.sh"), "#!/bin/sh\n").expect("expected value");

        let err = load_hooks_no_ignore(&hooks_dir).expect_err("expected parse error");
        let full = format!("{err:#}");
        assert!(full.contains("failed to parse"), "error: {full}");
    }

    #[test]
    fn test_load_hook_specs_invalid_id_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        fs::create_dir_all(hooks_dir.join("scripts")).expect("expected value");
        // Uppercase id — rejected by `validate_hook_id`.
        let toml = "
[hooks.BadID]
events = [\"session_start\"]
script = \"scripts/x.sh\"
";
        fs::write(hooks_dir.join("hooks.toml"), toml).expect("expected value");
        fs::write(hooks_dir.join("scripts").join("x.sh"), "#!/bin/sh\n").expect("expected value");

        let err = load_hooks_no_ignore(&hooks_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(full.contains("BadID"), "error: {full}");
    }

    #[test]
    fn test_load_hook_specs_reserved_prefix_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        fs::create_dir_all(hooks_dir.join("scripts")).expect("expected value");
        fs::write(hooks_dir.join("hooks.toml"), "").expect("expected value");
        fs::write(
            hooks_dir.join("scripts").join("_agentspec_envelope.sh"),
            "#!/bin/sh\n",
        )
        .expect("expected value");

        let err = load_hooks_no_ignore(&hooks_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(full.contains("_agentspec_"), "error: {full}");
        assert!(full.contains("reserved"), "error: {full}");
    }

    #[test]
    fn test_load_hook_specs_script_outside_scripts_rejected() {
        // A `script` path that doesn't live under `scripts/` would silently
        // produce a broken hook entry: the file isn't collected by
        // `collect_hook_scripts` (which only walks `scripts/`), but the
        // command anchor still points under `${ANCHOR}/hooks/scripts/`.
        // Reject at validate time with a clear message.
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        fs::create_dir_all(hooks_dir.join("scripts")).expect("expected value");
        // Place a real file at `spec/hooks/init.sh` — it would pass the
        // existence check but live outside `scripts/`.
        fs::write(hooks_dir.join("init.sh"), "#!/bin/sh\n").expect("expected value");
        let toml = "
[hooks.init]
events = [\"session_start\"]
script = \"init.sh\"
";
        fs::write(hooks_dir.join("hooks.toml"), toml).expect("expected value");

        let err = load_hooks_no_ignore(&hooks_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(full.contains("must live under `scripts/`"), "error: {full}");
    }

    #[test]
    fn test_load_hook_specs_in_tree_symlink_resolved() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        let scripts_dir = hooks_dir.join("scripts");
        fs::create_dir_all(&scripts_dir).expect("expected value");

        let real = scripts_dir.join("real-init.sh");
        fs::write(&real, "#!/bin/sh\necho init").expect("expected value");
        fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).expect("expected value");
        std::os::unix::fs::symlink(&real, scripts_dir.join("init-link.sh"))
            .expect("expected value");

        let toml = "
[hooks.init]
events = [\"session_start\"]
script = \"scripts/real-init.sh\"
";
        fs::write(hooks_dir.join("hooks.toml"), toml).expect("expected value");

        let specs = load_hooks_no_ignore(&hooks_dir).expect("expected value");
        let hook_spec = specs
            .iter()
            .find_map(|s| match s {
                Spec::Hook(h) => Some(h),
                Spec::Agent(_) | Spec::Skill(_) | Spec::Rule(_) => None,
            })
            .expect("expected hook spec");
        let link_file = hook_spec
            .supporting_files
            .get(&PathBuf::from("scripts/init-link.sh"))
            .expect("symlinked file should appear in supporting_files");
        assert_eq!(link_file.content, b"#!/bin/sh\necho init");
        assert_eq!(link_file.mode, 0o755);
    }

    #[test]
    fn test_load_hook_specs_out_of_tree_symlink_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        let scripts_dir = hooks_dir.join("scripts");
        fs::create_dir_all(&scripts_dir).expect("expected value");

        let outside = tmp.path().join("outside.sh");
        fs::write(&outside, "#!/bin/sh\n").expect("expected value");
        std::os::unix::fs::symlink(&outside, scripts_dir.join("init.sh")).expect("expected value");

        let toml = "
[hooks.init]
events = [\"session_start\"]
script = \"scripts/init.sh\"
";
        fs::write(hooks_dir.join("hooks.toml"), toml).expect("expected value");

        let err = load_hooks_no_ignore(&hooks_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(full.contains("outside the spec tree"), "error: {full}");
    }

    #[test]
    fn test_load_hook_specs_dangling_symlink_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        let scripts_dir = hooks_dir.join("scripts");
        fs::create_dir_all(&scripts_dir).expect("expected value");

        std::os::unix::fs::symlink(
            scripts_dir.join("nonexistent.sh"),
            scripts_dir.join("init.sh"),
        )
        .expect("expected value");

        let toml = "
[hooks.init]
events = [\"session_start\"]
script = \"scripts/init.sh\"
";
        fs::write(hooks_dir.join("hooks.toml"), toml).expect("expected value");

        let err = load_hooks_no_ignore(&hooks_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(full.contains("does not exist"), "error: {full}");
    }

    #[test]
    fn test_hook_frontmatter_rejects_id_field_in_table_body() {
        // `HookFrontmatter::id` is `#[serde(skip)]` — it's populated from the
        // `[hooks.<id>]` table key, never from the inner table body. Combined
        // with `#[serde(deny_unknown_fields)]` on `HookSpecFile`, writing
        // `id = "x"` inside `[hooks.foo]` must error rather than silently
        // overwriting the captured key.
        let toml = "
[hooks.foo]
id = \"bar\"
events = [\"session_start\"]
script = \"scripts/init.sh\"
";
        let result: Result<HookSpecFile, _> = toml::from_str(toml);
        let err = result.expect_err("expected unknown-field error for body `id`");
        let msg = format!("{err}");
        assert!(
            msg.contains("unknown field") && msg.contains("id"),
            "expected unknown-field error mentioning `id`, got: {msg}"
        );
    }

    #[test]
    fn test_load_hook_specs_reserved_prefix_directory_rejected() {
        // The `_agentspec_*` reservation owns the whole namespace, not just
        // leaf filenames. A directory like `scripts/_agentspec_helpers/` would
        // otherwise leak into emitted output under the reserved prefix.
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        let reserved_subdir = hooks_dir.join("scripts").join("_agentspec_helpers");
        fs::create_dir_all(&reserved_subdir).expect("expected value");
        fs::write(reserved_subdir.join("foo.sh"), "#!/bin/sh\n").expect("expected value");
        fs::write(hooks_dir.join("hooks.toml"), "").expect("expected value");

        let err = load_hooks_no_ignore(&hooks_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(full.contains("_agentspec_"), "error: {full}");
        assert!(full.contains("reserved"), "error: {full}");
    }

    #[test]
    fn test_load_hook_specs_missing_script_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        fs::create_dir_all(hooks_dir.join("scripts")).expect("expected value");
        let toml = "
[hooks.init]
events = [\"session_start\"]
script = \"scripts/missing.sh\"
";
        fs::write(hooks_dir.join("hooks.toml"), toml).expect("expected value");

        let err = load_hooks_no_ignore(&hooks_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(full.contains("does not exist"), "error: {full}");
    }

    #[test]
    fn test_load_hook_specs_ignore_pattern_prunes_tests_subdir() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        write_hook_fixture(&hooks_dir, "init", "session_start", "init.sh");
        fs::create_dir_all(hooks_dir.join("scripts").join("tests")).expect("expected value");
        // A reserved-prefix file under tests/ would error if walked — but
        // `ignore = ["**/scripts/tests/**"]` should prune the subtree first.
        fs::write(
            hooks_dir
                .join("scripts")
                .join("tests")
                .join("_agentspec_x.sh"),
            "#!/bin/sh\n",
        )
        .expect("expected value");

        let patterns = vec!["**/scripts/tests/**".to_string()];
        let ignore = IgnoreMatcher::compile(&patterns).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs =
            load_hook_specs(&hooks_dir, &ignore, tmp.path(), &mut report).expect("expected value");
        assert_eq!(specs.len(), 1);
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
