use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use gray_matter::Matter;
use gray_matter::engine::YAML;
use indexmap::IndexMap;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use walkdir::WalkDir;

use crate::presets::ProviderPresetsMap;
use crate::spec::{
    AgentSpec, HookFrontmatter, HookSpec, RuleSpec, SkillFrontmatter, SkillSpec, Spec,
    SupportingFile,
};
use crate::symlink::{dangling_symlink, is_symlink, resolve_dir_entry, unresolvable_symlink};
use crate::validate::{ValidationError, validate_semantics};

// ---------------------------------------------------------------------------
// Pipeline stage types
// ---------------------------------------------------------------------------

/// Compiled set of ignore glob patterns, matched against paths relative to
/// [`SpecDirs`]'s `sources_dir`.
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
#[derive(Debug)]
pub struct SpecDirs {
    agents: PathBuf,
    skills: PathBuf,
    rules: PathBuf,
    /// Directory containing `hooks.toml` and the `scripts/` subdirectory.
    /// Absent directory is not an error — hook authoring is opt-in.
    hooks: PathBuf,
    /// Compiled ignore patterns applied to every file walked during load.
    ignore: IgnoreMatcher,
    /// The root of the spec library: the four roots above are joins onto it,
    /// ignore patterns are matched against paths made relative to it, and the
    /// templating layer resolves `{% include %}` under it.
    sources_dir: PathBuf,
}

impl SpecDirs {
    /// Derive the four spec roots from `sources`.
    ///
    /// Fails when `sources` is not a directory — absent, unreadable, the wrong
    /// kind, or a symlink that does not resolve to one.
    ///
    /// Each root is a join onto `sources`, so any of those leaves all four
    /// looking merely absent: the load finds no specs, and `sync` treats every
    /// file an earlier run installed as stale and removes it. A mistyped
    /// `sources_dir` is the likeliest way to reach that, which is why an absent
    /// root is an error here even though each of the four roots below it is
    /// optional. Constructing the roots in this one place is what keeps the
    /// check from being bypassed by spelling the joins out again.
    pub fn new(sources: PathBuf, ignore: IgnoreMatcher) -> Result<Self> {
        require_sources_dir(&sources)?;
        Ok(Self {
            agents: sources.join("agents"),
            skills: sources.join("skills"),
            rules: sources.join("rules"),
            hooks: sources.join("hooks"),
            ignore,
            sources_dir: sources,
        })
    }

    /// The root of the spec library these roots were derived from.
    ///
    /// Derived once, in [`SpecDirs::new`], so a caller that needs the path —
    /// the templating layer resolves `{% include %}` under it — reads the
    /// checked value rather than re-resolving it from config.
    pub fn sources_dir(&self) -> &Path {
        &self.sources_dir
    }

    /// The ignore patterns these roots are loaded under.
    pub fn ignore(&self) -> &IgnoreMatcher {
        &self.ignore
    }

    /// The directory holding `hooks.toml` and its `scripts/` subtree.
    pub fn hooks(&self) -> &Path {
        &self.hooks
    }
}

/// A single path that an `[spec].ignore` pattern excluded from the loaded set.
///
/// Carries no claim about what kind of filesystem object sat at the path: a
/// pattern decides membership before anything is resolved, so most sites that
/// record one have never stat'd it.
#[derive(Clone, Debug)]
pub struct IgnoredPath {
    /// Path relative to the `sources_dir` the [`SpecDirs`] was built from.
    pub rel_path: PathBuf,
    /// Index into [`IgnoreMatcher::patterns`].
    pub pattern_index: usize,
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
    /// Paths already recorded, so [`LoadReport::record`] can drop a repeat.
    seen: HashSet<PathBuf>,
}

impl LoadReport {
    /// Construct a report sized for `matcher`, with zero hits recorded.
    pub fn with_matcher(matcher: &IgnoreMatcher) -> Self {
        Self {
            ignored: Vec::new(),
            pattern_hits: vec![0; matcher.len()],
            seen: HashSet::new(),
        }
    }

    /// Record a single ignored path and bump its pattern's hit count.
    ///
    /// This is the single-recording enforcement point: every site records
    /// unconditionally and the first recording wins. Two sites reaching the same
    /// path is one ignored path with one hit, and nothing in [`IgnoredPath`]
    /// varies between sightings — [`IgnoreMatcher::matching_index`] returns the
    /// lowest matching index, so the pattern is the same either time — which is
    /// what makes first-wins unambiguous rather than a tiebreak.
    pub fn record(&mut self, rel_path: PathBuf, pattern_index: usize) {
        if !self.seen.insert(rel_path.clone()) {
            return;
        }
        self.ignored.push(IgnoredPath {
            rel_path,
            pattern_index,
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

/// Decide whether `path` is excluded from the loaded set, recording the match.
///
/// A pattern decides membership before anything is resolved, so every site that
/// could exclude a path asks this first and stats second. Returns `true` when
/// the caller should skip the path — and, where the path turns out to be a
/// directory, prune the subtree.
///
/// A path matches when it matches a pattern itself *or* when a synthetic child
/// under it would, which is what lets `skills/deploy/**` exclude
/// `skills/deploy` even though the directory path has no child component for
/// `**` to bind to. That second test runs for every path, not only for one
/// already known to be a directory: most callers here have not resolved the
/// path and cannot know, and a rule that ran the test only for paths of a known
/// kind would exclude a path at one site and admit it at another — reporting a
/// file as ignored while still emitting it.
///
/// Nothing recorded here distinguishes a file from a directory, for the same
/// reason.
fn should_ignore_path(
    path: &Path,
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
    let Some(idx) = ignore
        .matching_index(rel)
        .or_else(|| ignore.matching_index(&rel.join("__agentspec_ignore_probe__")))
    else {
        return false;
    };
    report.record(rel.to_path_buf(), idx);
    true
}

/// `walkdir::DirEntry` adapter around [`should_ignore_path`].
fn should_ignore_entry(
    entry: &walkdir::DirEntry,
    anchor: &Path,
    ignore: &IgnoreMatcher,
    report: &mut LoadReport,
) -> bool {
    should_ignore_path(entry.path(), anchor, ignore, report)
}

/// Walk `root`, yielding each surviving entry to `visit`.
///
/// A pattern decides membership in the loaded set before anything is resolved,
/// which is why this drives `WalkDir` directly rather than through
/// `filter_entry`: `filter_entry` propagates a failed entry without consulting
/// its predicate, so a pattern could never reach a dangling link or a cycle
/// `walkdir` caught while producing the entry. Driving the iterator by hand also
/// leaves one owner of `report`, which a predicate closure holding it for the
/// walker's lifetime would not.
///
/// Every entry is checked, the root included. An admitted root cannot match a
/// pattern — [`admit_spec_dir`] returns `false` when it does — and
/// [`LoadReport::record`] absorbs a repeat in any case.
fn walk_spec_tree(
    root: &Path,
    ignore: &IgnoreMatcher,
    anchor: &Path,
    report: &mut LoadReport,
    mut visit: impl FnMut(walkdir::DirEntry) -> Result<()>,
) -> Result<()> {
    let mut it = WalkDir::new(root).follow_links(true).into_iter();
    while let Some(result) = it.next() {
        let entry = match result {
            Ok(entry) => entry,
            Err(err) => {
                // An entry that failed to resolve is still matched against the
                // patterns by its own path. `walkdir` returns from `follow`
                // before `push`, so the walk never descended and iteration
                // resumes safely.
                if let Some(path) = err.path()
                    && should_ignore_path(path, anchor, ignore, report)
                {
                    continue;
                }
                return Err(walk_error(err, root));
            }
        };
        if should_ignore_entry(&entry, anchor, ignore, report) {
            if entry.file_type().is_dir() {
                it.skip_current_dir();
            }
            continue;
        }
        // `walkdir` yields a directory before descending into it, so refusing an
        // enclosing target here is what keeps the walk off it — left to
        // `walkdir`, the same link is reported only after the target has been
        // read, and named by the second encounter.
        if entry.file_type().is_dir() && entry.path_is_symlink() {
            reject_ancestor_loop(entry.path())?;
        }
        visit(entry)?;
    }
    Ok(())
}

/// Translates a `walkdir` error into a diagnostic naming the symlink at fault,
/// where `walkdir` supplies the path.
///
/// Walks that set `follow_links` surface loops and dangling targets as errors
/// rather than skipping them, so every such walk routes its failures through
/// here instead of discarding them with `filter_map(Result::ok)`.
///
/// `walkdir` carries a path only on an error raised against a single entry. A
/// failure it cannot attribute — a target that stats but will not open, a
/// read interrupted mid-directory — arrives with none, and falls through to a
/// diagnostic headed by the walk root. Naming the link is the better case, not
/// the guaranteed one.
fn walk_error(err: walkdir::Error, root: &Path) -> anyhow::Error {
    if let Some(ancestor) = err.loop_ancestor() {
        let source = err
            .path()
            .map_or_else(|| "<unknown>".to_string(), |p| p.display().to_string());
        return anyhow!(
            "{source}: symlink loop detected (cycles back to {})",
            ancestor.display()
        );
    }
    if let Some(path) = err.path().filter(|p| is_symlink(p))
        && let Some(io) = err.io_error()
    {
        // Every failure to resolve a link, not just a missing target: a
        // mutually-referential pair fails inside `walkdir`'s own `metadata`
        // call with `ELOOP` before its ancestor tracking runs, so
        // `loop_ancestor` is `None` and the generic context below would head
        // the diagnostic with the walk root instead of the link.
        return anyhow!(unresolvable_symlink(path, io));
    }
    let root = root.display().to_string();
    anyhow::Error::new(err).context(format!("error walking {root}"))
}

/// Fails when `path` is a symlink whose target encloses the directory the link
/// itself sits in.
///
/// Such a link resolves to a real directory, so `metadata()` succeeds and no
/// `ELOOP` is raised — the cycle only appears once something descends. `walkdir`
/// does detect it eventually, but only after walking the target in full, and it
/// reports whatever ancestor it was standing on rather than the link. Naming it
/// here keeps the diagnostic on the link and the walk off a target that may be
/// the whole filesystem, which is why every walk root and every resolved
/// directory entry passes through this.
fn reject_ancestor_loop(path: &Path) -> Result<()> {
    if !is_symlink(path) {
        return Ok(());
    }
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let target = path
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", path.display()))?;
    // The link's own directory, not the link resolved: a spec root authored as
    // a link to a shared pool resolves to its target, which would compare equal
    // to it and read as a cycle.
    let canonical_parent = parent
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", parent.display()))?;
    if canonical_parent.starts_with(&target) {
        bail!(
            "{}: symlink loop detected (cycles back to {})",
            path.display(),
            target.display()
        );
    }
    Ok(())
}

/// The kind of filesystem object a spec root is expected to be.
#[derive(Clone, Copy)]
enum RootKind {
    Dir,
    File,
}

impl RootKind {
    fn matches(self, file_type: fs::FileType) -> bool {
        match self {
            Self::Dir => file_type.is_dir(),
            Self::File => file_type.is_file(),
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Self::Dir => "directory",
            Self::File => "file",
        }
    }
}

/// What a stat of a spec root found.
///
/// The optional roots and the required `sources_dir` need the same four-way
/// distinction and act on it differently, so the distinction is drawn once here
/// and each caller maps it to its own diagnostic.
enum RootState {
    /// Resolves to the expected kind.
    Present,
    /// Nothing is at the path.
    Absent,
    /// Something is at the path, but not of the expected kind.
    WrongKind,
    /// The path could not be stat'd for some other reason.
    Unreadable(std::io::Error),
}

fn spec_root_state(path: &Path, kind: RootKind) -> RootState {
    match path.metadata() {
        Ok(metadata) if kind.matches(metadata.file_type()) => RootState::Present,
        Ok(_) => RootState::WrongKind,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => RootState::Absent,
        Err(e) => RootState::Unreadable(e),
    }
}

/// Fails unless `path` is a usable spec sources directory.
///
/// Unlike the four roots under it, which are optional, this one is required —
/// see [`SpecDirs::new`]. Every message names the setting to correct and leads
/// with what is wrong: the remedy is the same in each case, and the cause is
/// what the user cannot see.
fn require_sources_dir(path: &Path) -> Result<()> {
    const HINT: &str =
        "set `[spec].sources_dir` to the directory holding agents/, skills/, and rules/";
    let source = path.display();
    let linked = is_symlink(path);
    match spec_root_state(path, RootKind::Dir) {
        RootState::Present => Ok(()),
        RootState::Absent if linked => Err(with_hint(&dangling_symlink(path), HINT)),
        RootState::Absent => {
            bail!("{source}: spec sources directory does not exist ({HINT})")
        }
        RootState::WrongKind if linked => {
            bail!("{source}: symlink target is not a directory ({HINT})")
        }
        RootState::WrongKind => {
            bail!("{source}: spec sources path is not a directory ({HINT})")
        }
        RootState::Unreadable(e) if linked => Err(with_hint(&unresolvable_symlink(path, &e), HINT)),
        RootState::Unreadable(e) => Err(with_hint(
            &format!("{source}: spec sources directory could not be read: {e}"),
            HINT,
        )),
    }
}

/// Decide whether a spec root participates in this load.
///
/// Pattern first, then presence, then the ancestor-loop guard. An ignored root
/// leaves the loaded set before it is stat'd, so ignoring `agents` exempts a
/// dangling or wrong-kind `agents` link — the same rule every entry under the
/// root follows.
///
/// Recording precedes the presence check too, so an ignored root that does not
/// exist gets a listing entry and a pattern hit. That follows from the rule: the
/// pattern decided the root's membership, and whether the root exists is a
/// question agentspec no longer asks.
fn admit_spec_dir(
    root: &Path,
    ignore: &IgnoreMatcher,
    anchor: &Path,
    report: &mut LoadReport,
) -> Result<bool> {
    if should_ignore_path(root, anchor, ignore, report) {
        return Ok(false);
    }
    if !spec_dir_present(root)? {
        return Ok(false);
    }
    reject_ancestor_loop(root)?;
    Ok(true)
}

/// Reports whether an optional spec root directory is present, failing when it
/// is a symlink that does not resolve to one.
fn spec_dir_present(path: &Path) -> Result<bool> {
    spec_root_present(path, RootKind::Dir)
}

/// Reports whether an optional spec root file is present, failing when it is a
/// symlink that does not resolve to one.
fn spec_file_present(path: &Path) -> Result<bool> {
    spec_root_present(path, RootKind::File)
}

/// Distinguishes a root the author never created from one authored as a
/// symlink that does not resolve.
///
/// `Path::is_dir` and `Path::is_file` answer `false` for both, and that
/// conflation is what makes a broken root dangerous rather than merely wrong:
/// the load succeeds, the specs under it silently vanish from the output, and
/// `sync` deletes the files a previous run installed from them.
///
/// Only an absent path is read as an unauthored root. A path that exists but is
/// unreadable, or is of the wrong kind, fails: each is a mistake the author can
/// correct, and neither has a reading under which the specs were meant to be
/// missing.
fn spec_root_present(path: &Path, kind: RootKind) -> Result<bool> {
    let linked = is_symlink(path);
    match spec_root_state(path, kind) {
        RootState::Present => Ok(true),
        RootState::Absent if linked => Err(anyhow!(dangling_symlink(path))),
        RootState::WrongKind if linked => bail!(
            "{}: symlink target is not a {}",
            path.display(),
            kind.noun()
        ),
        // Absent is the one benign reading: an optional root simply went
        // unauthored. Something of the wrong kind sitting at the path is an
        // authoring mistake with no benign reading, and letting it pass as
        // absent is how the specs under it vanish without a diagnostic.
        RootState::WrongKind => bail!("{}: is not a {}", path.display(), kind.noun()),
        RootState::Absent => Ok(false),
        // Not folded into the absent reading: a root that cannot be read is a
        // root whose specs cannot be seen, and reporting it as unauthored is
        // how `sync` comes to treat everything installed from it as stale.
        RootState::Unreadable(e) if linked => Err(anyhow!(unresolvable_symlink(path, &e))),
        RootState::Unreadable(e) => {
            Err(e).with_context(|| format!("failed to read {}", path.display()))
        }
    }
}

/// Builds an error reading `message`, with `hint` trailing it.
///
/// Not `anyhow::Context`, which would make the hint the headline and demote the
/// fault to a `Caused by` line — the fault is what the user cannot see. Taking
/// the message rather than an `anyhow::Error` is what keeps that restatement
/// from discarding a cause chain: there is none to discard.
fn with_hint(message: &str, hint: &str) -> anyhow::Error {
    anyhow!("{message} ({hint})")
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
    let mut specs = load_agent_specs(&dirs.agents, &dirs.ignore, &dirs.sources_dir, report)?;
    specs.extend(load_skill_specs(
        &dirs.skills,
        &dirs.ignore,
        &dirs.sources_dir,
        report,
    )?);
    specs.extend(load_rule_specs(
        &dirs.rules,
        &dirs.ignore,
        &dirs.sources_dir,
        report,
    )?);
    specs.extend(load_hook_specs(
        &dirs.hooks,
        &dirs.ignore,
        &dirs.sources_dir,
        report,
    )?);
    Ok(specs)
}

/// Load every `.md` file under `dir` as one flat spec kind.
///
/// Deserializes `F` from each file's YAML frontmatter and delegates variant
/// construction to `make`, so a spec kind that is one `.md` file per spec
/// differs from its siblings only in those two things.
fn load_md_specs<F, M>(
    dir: &Path,
    ignore: &IgnoreMatcher,
    anchor: &Path,
    report: &mut LoadReport,
    make: M,
) -> Result<Vec<Spec>>
where
    F: DeserializeOwned,
    M: Fn(PathBuf, F, String) -> Spec,
{
    if !admit_spec_dir(dir, ignore, anchor, report)? {
        return Ok(Vec::new());
    }

    let mut md_paths = Vec::new();
    walk_spec_tree(dir, ignore, anchor, report, |entry| {
        if entry.file_type().is_file() && entry.path().extension().is_some_and(|ext| ext == "md") {
            md_paths.push(entry.into_path());
        }
        Ok(())
    })?;
    md_paths.sort();

    let matter = Matter::<YAML>::new();
    let mut specs = Vec::new();
    for path in md_paths {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let parsed = matter
            .parse::<F>(&content)
            .with_context(|| format!("failed to parse frontmatter in {}", path.display()))?;
        let frontmatter = parsed
            .data
            .ok_or_else(|| anyhow!("missing spec for {}", path.display()))?;
        specs.push(make(path, frontmatter, parsed.content));
    }
    Ok(specs)
}

fn load_agent_specs(
    dir: &Path,
    ignore: &IgnoreMatcher,
    anchor: &Path,
    report: &mut LoadReport,
) -> Result<Vec<Spec>> {
    load_md_specs(dir, ignore, anchor, report, |path, frontmatter, body| {
        Spec::Agent(AgentSpec {
            path,
            frontmatter,
            body,
        })
    })
}

fn load_skill_specs(
    dir: &Path,
    ignore: &IgnoreMatcher,
    anchor: &Path,
    report: &mut LoadReport,
) -> Result<Vec<Spec>> {
    if !admit_spec_dir(dir, ignore, anchor, report)? {
        return Ok(Vec::new());
    }

    let mut skill_dirs = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("failed to read {}", dir.display()))?;
        let path = entry.path();
        // Prune-first: an ignored entry leaves the loaded set before it is
        // resolved, so a pattern naming a skill directory exempts a link that
        // does not resolve.
        //
        // Every entry reaches this, not only the directories that survive
        // resolution — a plain `skills/README.md` named by a pattern is recorded
        // and counts as a pattern hit.
        if should_ignore_path(&path, anchor, ignore, report) {
            continue;
        }
        // A skill directory may be a symlink to a shared location, so resolve
        // the entry rather than describing the link.
        if resolve_dir_entry(&path)?.is_dir() {
            skill_dirs.push(path);
        }
    }
    skill_dirs.sort();

    let matter = Matter::<YAML>::new();

    let mut specs = Vec::new();

    for skill_dir in skill_dirs {
        // The scan above already decided membership, so this guard only refuses
        // links the patterns admitted.
        reject_ancestor_loop(&skill_dir)?;

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
    let mut entries = Vec::new();
    let mut ignored_md = false;
    for entry in fs::read_dir(skill_dir)
        .with_context(|| format!("failed to read {}", skill_dir.display()))?
    {
        let entry = entry.with_context(|| format!("failed to read {}", skill_dir.display()))?;
        let entry_path = entry.path();
        // Prune-first: an ignored entry is never resolved, so a pattern naming a
        // spec file exempts a link that does not resolve.
        if should_ignore_path(&entry_path, anchor, ignore, report) {
            // `select_spec_md` needs to tell "every `.md` here was ignored"
            // (skip the skill) from "there is no `.md` here" (an authoring
            // error), and it can no longer see the ignored entries to work that
            // out for itself.
            ignored_md |= entry_path.extension().is_some_and(|ext| ext == "md");
            continue;
        }
        // A spec file may itself be a symlink into a shared pool, so resolve
        // the entry rather than describing the link.
        if resolve_dir_entry(&entry_path)?.is_file() {
            entries.push(entry);
        }
    }

    let Some(md_path) = select_spec_md(skill_dir, &entries, ignored_md)? else {
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
    // `load_skill_specs` already admitted this directory, so no `admit_spec_dir`
    // here.
    walk_spec_tree(skill_dir, ignore, anchor, report, |entry| {
        if !entry.file_type().is_file() {
            return Ok(());
        }
        let entry_path = entry.path();

        if entry_path == md_path.as_path() {
            return Ok(());
        }

        let Ok(relative_path) = entry_path.strip_prefix(skill_dir) else {
            return Ok(());
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
        Ok(())
    })?;
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
/// `entries` holds only the files that survived the caller's pattern check, so
/// this sees no ignored path and records nothing. `ignored_md` is what the
/// caller observed while excluding them: `true` when at least one `.md` in the
/// directory was excluded by a pattern, which distinguishes a skill that is
/// absent from the pipeline (`Ok(None)`) from one that is malformed.
fn select_spec_md(
    skill_dir: &Path,
    entries: &[fs::DirEntry],
    ignored_md: bool,
) -> Result<Option<PathBuf>> {
    let md_files: Vec<_> = entries
        .iter()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
        .collect();

    if md_files.is_empty() {
        // Every `.md` in this skill was ignored — the caller's scan recorded
        // them, so the skill is absent from the pipeline rather than malformed.
        if ignored_md {
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
    load_md_specs(dir, ignore, anchor, report, |path, frontmatter, body| {
        Spec::Rule(RuleSpec {
            path,
            frontmatter,
            body,
        })
    })
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
    if !admit_spec_dir(dir, ignore, anchor, report)? {
        return Ok(Vec::new());
    }

    let toml_path = dir.join("hooks.toml");
    let scripts_dir = dir.join("scripts");

    // An ignored `hooks.toml` removes the hook set entirely, scripts included:
    // there is no declaration left to emit them from.
    if should_ignore_path(&toml_path, anchor, ignore, report) {
        return Ok(Vec::new());
    }

    if !spec_file_present(&toml_path)? {
        // `admit_spec_dir` rather than `spec_dir_present`: an ignored `scripts/`
        // tree raises no orphan diagnostic, because an author who excluded it
        // and wrote no `hooks.toml` turned hooks off rather than forgetting
        // something. It also runs `reject_ancestor_loop`, which
        // `spec_dir_present` did not — a `scripts` link enclosing its own parent
        // now fails on the loop, which names the link and is the more specific
        // fault for a directory that cannot be walked at all.
        if admit_spec_dir(&scripts_dir, ignore, anchor, report)? {
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
        // Ask the loaded set, not the disk: a script a pattern excluded is not
        // emitted, so a `hooks.toml` declaring it would otherwise produce a hook
        // command pointing at a file agentspec never writes.
        //
        // `collect_hook_scripts` keys on `strip_prefix(hooks_dir)`, which yields
        // no `.` components, while `validate_hook_script_path` permits
        // `././scripts/x.sh`.
        let script_key: PathBuf = frontmatter
            .script
            .components()
            .filter(|c| !matches!(c, Component::CurDir))
            .collect();
        if !supporting_files.contains_key(&script_key) {
            let script_path = dir.join(&script_key);
            // Ask what the load excluded rather than re-deriving it. A pattern
            // naming an ancestor directory pruned this script without ever
            // matching its own path, so re-running the matcher on the script
            // would report the wrong fault — that it does not exist, about a
            // file sitting on disk.
            let covering = script_path.strip_prefix(anchor).ok().and_then(|rel| {
                report
                    .ignored
                    .iter()
                    .find(|ignored| rel.starts_with(&ignored.rel_path))
            });
            if let Some(ignored) = covering {
                bail!(
                    "hook '{id}' in {}: script {} was excluded from the load by `[spec].ignore` pattern '{}'",
                    toml_path.display(),
                    script_key.display(),
                    ignore.pattern(ignored.pattern_index).unwrap_or("<unknown>")
                );
            }
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
    // An ignored or absent `scripts/` yields an empty map rather than
    // propagating: this is the value the declared-script membership test in
    // `load_hook_specs` reads.
    if !admit_spec_dir(scripts_dir, ignore, anchor, report)? {
        return Ok(IndexMap::new());
    }

    let hooks_dir = scripts_dir.parent().unwrap_or(scripts_dir);
    let mut files = IndexMap::new();
    walk_spec_tree(scripts_dir, ignore, anchor, report, |entry| {
        if !entry.file_type().is_file() {
            return Ok(());
        }

        if let Ok(rel_under_scripts) = entry.path().strip_prefix(scripts_dir) {
            for component in rel_under_scripts.components() {
                if let Component::Normal(name) = component
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
            return Ok(());
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
        Ok(())
    })?;
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

    /// Load rules with no ignore patterns, rooted at `dir` as the anchor.
    fn load_rules_no_ignore(dir: &Path) -> Result<Vec<Spec>> {
        let mut report = LoadReport::default();
        load_rule_specs(dir, &IgnoreMatcher::empty(), dir, &mut report)
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
    fn test_load_agent_specs_out_of_tree_symlink_resolved() {
        let tmp = tempfile::tempdir().expect("expected value");
        let agents_dir = tmp.path().join("agents");
        fs::create_dir(&agents_dir).expect("expected value");

        let outside = tmp.path().join("shared-agent.md");
        let spec_content =
            "---\nid: shared-agent\ndescription: A shared agent\n---\nShared body.\n";
        fs::write(&outside, spec_content).expect("expected value");
        std::os::unix::fs::symlink(&outside, agents_dir.join("shared-agent.md"))
            .expect("expected value");

        let specs = load_agents_no_ignore(&agents_dir).expect("expected value");
        assert_eq!(specs.len(), 1);
        let Spec::Agent(ref s) = specs[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(s.frontmatter.id, "shared-agent");
        assert_eq!(s.body, "Shared body.");
    }

    #[test]
    fn test_load_agent_specs_dangling_symlink_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let agents_dir = tmp.path().join("agents");
        fs::create_dir(&agents_dir).expect("expected value");

        std::os::unix::fs::symlink(tmp.path().join("gone.md"), agents_dir.join("broken.md"))
            .expect("expected value");

        let err = load_agents_no_ignore(&agents_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.contains("symlink target does not exist"),
            "error: {full}"
        );
    }

    #[test]
    fn test_load_skill_specs_symlinked_skill_directory_resolved() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        fs::create_dir(&skills_dir).expect("expected value");

        let outside = tmp.path().join("shared-skill");
        let scripts_dir = outside.join("scripts");
        fs::create_dir_all(&scripts_dir).expect("expected value");
        let spec_content = "---\nid: shared-skill\ndescription: A shared skill\nuser_invocable: true\nagent_invocable: false\n---\nSkill body.\n";
        fs::write(outside.join("SKILL.md"), spec_content).expect("expected value");
        let script = scripts_dir.join("run.sh");
        fs::write(&script, "#!/bin/sh\necho shared").expect("expected value");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("expected value");

        std::os::unix::fs::symlink(&outside, skills_dir.join("shared-skill"))
            .expect("expected value");

        let specs = load_skills_no_ignore(&skills_dir).expect("expected value");
        assert_eq!(specs.len(), 1);
        let Spec::Skill(ref s) = specs[0] else {
            panic!("expected Skill variant")
        };
        assert_eq!(s.frontmatter.id, "shared-skill");
        let file = s
            .supporting_files
            .get(&PathBuf::from("scripts/run.sh"))
            .expect("bundled script should load through the directory symlink");
        assert_eq!(file.content, b"#!/bin/sh\necho shared");
        assert_eq!(file.mode, 0o755);
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
    fn test_load_skill_specs_out_of_tree_symlink_resolved() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("my-skill");
        let scripts_dir = skill_dir.join("scripts");
        fs::create_dir_all(&scripts_dir).expect("expected value");
        let spec_content = "---\nid: my-skill\ndescription: A test skill\nuser_invocable: true\nagent_invocable: false\n---\nSkill body.\n";
        fs::write(skill_dir.join("SKILL.md"), spec_content).expect("expected value");

        let outside = tmp.path().join("shared-pool.sh");
        fs::write(&outside, "#!/bin/sh\necho pooled").expect("expected value");
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).expect("expected value");
        std::os::unix::fs::symlink(&outside, scripts_dir.join("helper.sh"))
            .expect("expected value");

        let specs = load_skills_no_ignore(&skills_dir).expect("expected value");
        assert_eq!(specs.len(), 1);
        let Spec::Skill(ref s) = specs[0] else {
            panic!("expected Skill variant")
        };
        assert_eq!(s.supporting_files.len(), 1);
        let file = s.supporting_files.values().next().expect("expected value");
        assert_eq!(file.content, b"#!/bin/sh\necho pooled");
        assert_eq!(file.mode, 0o755);
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
    fn test_load_skill_specs_symlinked_spec_md_resolved() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        let skill_dir = skills_dir.join("my-skill");
        fs::create_dir_all(&skill_dir).expect("expected value");

        let outside = tmp.path().join("SKILL.md");
        let spec_content = "---\nid: my-skill\ndescription: A shared skill\nuser_invocable: true\nagent_invocable: false\n---\nShared skill body.\n";
        fs::write(&outside, spec_content).expect("expected value");
        std::os::unix::fs::symlink(&outside, skill_dir.join("SKILL.md")).expect("expected value");
        // A second, non-spec `.md` must not be promoted in the symlink's place.
        fs::write(skill_dir.join("notes.md"), "notes").expect("expected value");

        let specs = load_skills_no_ignore(&skills_dir).expect("expected value");
        assert_eq!(specs.len(), 1);
        let Spec::Skill(ref sk) = specs[0] else {
            panic!("expected Skill variant")
        };
        assert_eq!(sk.frontmatter.id, "my-skill");
        assert_eq!(sk.body, "Shared skill body.");
    }

    #[test]
    fn test_load_skill_specs_dangling_directory_symlink_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        fs::create_dir_all(&skills_dir).expect("expected value");

        std::os::unix::fs::symlink(tmp.path().join("gone"), skills_dir.join("shared-skill"))
            .expect("expected value");

        let err = load_skills_no_ignore(&skills_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.contains("symlink target does not exist"),
            "error: {full}"
        );
    }

    #[test]
    fn test_load_rule_specs_out_of_tree_symlink_resolved() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir(&rules_dir).expect("expected value");

        let outside = tmp.path().join("shared-rule.md");
        let spec_content =
            "---\nid: shared-rule\ndescription: A shared rule\n---\nShared rule body.\n";
        fs::write(&outside, spec_content).expect("expected value");
        std::os::unix::fs::symlink(&outside, rules_dir.join("shared-rule.md"))
            .expect("expected value");

        let specs = load_rules_no_ignore(&rules_dir).expect("expected value");
        assert_eq!(specs.len(), 1);
        let Spec::Rule(ref r) = specs[0] else {
            panic!("expected Rule variant")
        };
        assert_eq!(r.frontmatter.id, "shared-rule");
        assert_eq!(r.body, "Shared rule body.");
    }

    #[test]
    fn test_load_rule_specs_dangling_symlink_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");
        fs::create_dir(&rules_dir).expect("expected value");

        std::os::unix::fs::symlink(tmp.path().join("gone.md"), rules_dir.join("broken.md"))
            .expect("expected value");

        let err = load_rules_no_ignore(&rules_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.contains("symlink target does not exist"),
            "error: {full}"
        );
    }

    #[test]
    fn test_load_agent_specs_dangling_symlink_under_pruned_subtree_never_reached() {
        let tmp = tempfile::tempdir().expect("expected value");
        let agents_dir = tmp.path().join("agents");
        let vendor_dir = agents_dir.join("vendor");
        fs::create_dir_all(&vendor_dir).expect("expected value");
        let spec_content = "---\nid: good-agent\ndescription: A test\n---\nAgent body.\n";
        fs::write(agents_dir.join("good-agent.md"), spec_content).expect("expected value");
        std::os::unix::fs::symlink(tmp.path().join("gone.md"), vendor_dir.join("broken.md"))
            .expect("expected value");

        // Pruning the parent keeps the walk from ever descending to the link.
        let ignore = IgnoreMatcher::compile(&["vendor/**".to_string()]).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs = load_agent_specs(&agents_dir, &ignore, &agents_dir, &mut report)
            .expect("a pruned subtree is never descended into");

        assert_eq!(specs.len(), 1);
    }

    #[test]
    fn test_load_agent_specs_dangling_symlink_named_by_ignore_is_pruned() {
        let tmp = tempfile::tempdir().expect("expected value");
        let agents_dir = tmp.path().join("agents");
        fs::create_dir(&agents_dir).expect("expected value");
        std::os::unix::fs::symlink(tmp.path().join("gone.md"), agents_dir.join("broken.md"))
            .expect("expected value");

        // A pattern decides membership before anything is resolved, so naming
        // the link exempts it exactly as pruning its parent does.
        let ignore = IgnoreMatcher::compile(&["broken.md".to_string()]).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs = load_agent_specs(&agents_dir, &ignore, &agents_dir, &mut report)
            .expect("a pattern naming a dangling link exempts it");

        assert!(specs.is_empty());
        assert!(
            report.unused_pattern_indices().is_empty(),
            "the pattern was consulted for the link and hit"
        );
    }

    #[test]
    fn test_spec_dirs_new_rejects_missing_sources_dir() {
        let tmp = tempfile::tempdir().expect("expected value");
        let err = SpecDirs::new(tmp.path().join("spce"), IgnoreMatcher::empty())
            .expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.contains("spec sources directory does not exist"),
            "error: {full}"
        );
    }

    #[test]
    fn test_spec_dirs_new_rejects_sources_dir_that_is_a_file() {
        let tmp = tempfile::tempdir().expect("expected value");
        let notes = tmp.path().join("notes.md");
        fs::write(&notes, "").expect("expected value");

        let err = SpecDirs::new(notes.clone(), IgnoreMatcher::empty()).expect_err("expected error");
        let full = format!("{err:#}");
        // The whole prefix, not just the kind: the symlink arm's message ends
        // the same way, so a substring match would pass while the diagnostic
        // blamed a link that is not there.
        assert!(
            full.starts_with(&format!(
                "{}: spec sources path is not a directory",
                notes.display()
            )),
            "a path that exists must not be reported as missing: {full}"
        );
    }

    #[test]
    fn test_spec_dirs_new_rejects_dangling_sources_dir_symlink() {
        let tmp = tempfile::tempdir().expect("expected value");
        let sources = tmp.path().join("spec");
        std::os::unix::fs::symlink(tmp.path().join("gone"), &sources).expect("expected value");

        let err = SpecDirs::new(sources, IgnoreMatcher::empty()).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.contains("symlink target does not exist"),
            "error: {full}"
        );
        assert!(
            full.contains("`[spec].sources_dir`"),
            "the setting to correct should be named: {full}"
        );
    }

    #[test]
    fn test_spec_dirs_new_rejects_sources_dir_symlinked_to_wrong_kind() {
        let tmp = tempfile::tempdir().expect("expected value");
        let target = tmp.path().join("notes.md");
        fs::write(&target, "").expect("expected value");
        let sources = tmp.path().join("spec");
        std::os::unix::fs::symlink(&target, &sources).expect("expected value");

        let err =
            SpecDirs::new(sources.clone(), IgnoreMatcher::empty()).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.starts_with(&format!(
                "{}: symlink target is not a directory",
                sources.display()
            )),
            "a resolving target of the wrong kind must be blamed on the link: {full}"
        );
        assert!(
            full.contains("`[spec].sources_dir`"),
            "the setting to correct should be named: {full}"
        );
    }

    #[test]
    fn test_spec_dirs_new_rejects_sources_dir_symlink_cycle() {
        let tmp = tempfile::tempdir().expect("expected value");
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::os::unix::fs::symlink(&b, &a).expect("expected value");
        std::os::unix::fs::symlink(&a, &b).expect("expected value");

        let err = SpecDirs::new(a.clone(), IgnoreMatcher::empty()).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.starts_with(&format!(
                "{}: symlink target could not be resolved",
                a.display()
            )),
            "a cycle must not read as an unreadable directory: {full}"
        );
        assert!(
            full.contains("`[spec].sources_dir`"),
            "the setting to correct should be named: {full}"
        );
    }

    #[test]
    fn test_spec_dirs_new_reports_an_unreadable_sources_dir() {
        let tmp = tempfile::tempdir().expect("expected value");
        let locked = tmp.path().join("locked");
        let sources = locked.join("spec");
        fs::create_dir_all(&sources).expect("expected value");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("expected value");

        let denied = permissions_deny(&sources);
        let result = SpecDirs::new(sources, IgnoreMatcher::empty());

        // Restore before asserting: under a root uid the mode denies nothing,
        // so a panic here would leave a directory `TempDir` cannot clean up.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("expected value");
        if !denied {
            return;
        }
        let Err(err) = result else {
            panic!("expected error")
        };
        let full = format!("{err:#}");
        assert!(
            full.contains("spec sources directory could not be read"),
            "an unreadable path must not be reported as missing: {full}"
        );
    }

    #[test]
    fn test_load_agent_specs_unreadable_root_is_not_read_as_absent() {
        let tmp = tempfile::tempdir().expect("expected value");
        let sources = tmp.path().join("spec");
        let agents_dir = sources.join("agents");
        fs::create_dir_all(&agents_dir).expect("expected value");
        // Stat of the root itself must fail, which needs the parent to be
        // non-traversable — `chmod 000` on the root leaves `stat` succeeding.
        fs::set_permissions(&sources, fs::Permissions::from_mode(0o644)).expect("expected value");

        let denied = permissions_deny(&agents_dir);
        let result = load_agents_no_ignore(&agents_dir);

        fs::set_permissions(&sources, fs::Permissions::from_mode(0o755)).expect("expected value");
        if !denied {
            return;
        }
        let Err(err) = result else {
            panic!("expected error")
        };
        let full = format!("{err:#}");
        assert!(
            full.contains("failed to read"),
            "an unreadable spec root must not load as an empty one: {full}"
        );
    }

    /// Whether the permission fixture actually denies access to `path`.
    ///
    /// A root uid ignores the mode, and these tests have nothing to assert when
    /// it does. Probing the filesystem rather than the call's own result is
    /// what keeps the skip from swallowing a regression in the code under test.
    fn permissions_deny(path: &Path) -> bool {
        path.metadata().is_err()
    }

    #[test]
    fn test_spec_dirs_new_accepts_an_empty_sources_dir() {
        let tmp = tempfile::tempdir().expect("expected value");
        let sources = tmp.path().join("spec");
        fs::create_dir(&sources).expect("expected value");

        let dirs = SpecDirs::new(sources.clone(), IgnoreMatcher::empty()).expect("expected value");
        assert_eq!(dirs.hooks(), sources.join("hooks"));
        let (specs, _report) = Specs::load(&dirs).expect("an empty spec set is not an error");
        assert!(specs.specs.is_empty());
    }

    #[test]
    fn test_load_specs_dangling_spec_root_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let agents_dir = tmp.path().join("agents");
        std::os::unix::fs::symlink(tmp.path().join("gone"), &agents_dir).expect("expected value");

        let err = load_agents_no_ignore(&agents_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.contains("symlink target does not exist"),
            "a broken spec root must not read as an absent one: {full}"
        );
    }

    #[test]
    fn test_load_hook_specs_dangling_root_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        std::os::unix::fs::symlink(tmp.path().join("gone"), &hooks_dir).expect("expected value");

        let err = load_hooks_no_ignore(&hooks_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.contains("symlink target does not exist"),
            "error: {full}"
        );
    }

    #[test]
    fn test_load_specs_absent_spec_root_is_not_an_error() {
        let tmp = tempfile::tempdir().expect("expected value");
        let specs = load_agents_no_ignore(&tmp.path().join("agents")).expect("expected value");
        assert!(specs.is_empty());
    }

    #[test]
    fn test_load_hook_specs_dangling_toml_symlink_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        fs::create_dir_all(&hooks_dir).expect("expected value");
        std::os::unix::fs::symlink(tmp.path().join("gone.toml"), hooks_dir.join("hooks.toml"))
            .expect("expected value");

        let err = load_hooks_no_ignore(&hooks_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.contains("symlink target does not exist"),
            "error: {full}"
        );
    }

    #[test]
    fn test_load_specs_spec_root_symlinked_to_wrong_kind_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let target = tmp.path().join("not-a-dir");
        fs::write(&target, "").expect("expected value");
        let agents_dir = tmp.path().join("agents");
        std::os::unix::fs::symlink(&target, &agents_dir).expect("expected value");

        let err = load_agents_no_ignore(&agents_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.contains("symlink target is not a directory"),
            "a resolving target of the wrong kind must not read as missing: {full}"
        );
    }

    #[test]
    fn test_load_specs_spec_root_of_wrong_kind_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let agents_dir = tmp.path().join("agents");
        fs::write(&agents_dir, "").expect("expected value");

        let err = load_agents_no_ignore(&agents_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.starts_with(&format!("{}: is not a directory", agents_dir.display())),
            "a file sitting at a spec root must not read as missing: {full}"
        );
    }

    #[test]
    fn test_load_hook_specs_toml_of_wrong_kind_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        fs::create_dir_all(hooks_dir.join("hooks.toml")).expect("expected value");

        let err = load_hooks_no_ignore(&hooks_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.starts_with(&format!(
                "{}: is not a file",
                hooks_dir.join("hooks.toml").display()
            )),
            "a directory sitting at hooks.toml must not read as missing: {full}"
        );
    }

    #[test]
    fn test_load_specs_spec_root_symlink_loop_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::os::unix::fs::symlink(&b, &a).expect("expected value");
        std::os::unix::fs::symlink(&a, &b).expect("expected value");

        let err = load_agents_no_ignore(&a).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.contains("could not be resolved"),
            "a cycle must not read as a missing target: {full}"
        );
    }

    #[test]
    fn test_ignore_prunes_a_loop_inside_a_skill_directory() {
        let tmp = tempfile::tempdir().expect("expected value");
        let spec = tmp.path().join("spec");
        let skills_dir = spec.join("skills");
        let skill_dir = skills_dir.join("s1");
        let sub = skill_dir.join("sub");
        fs::create_dir_all(&sub).expect("expected value");
        let spec_content = "---\nid: s1\ndescription: A test skill\nuser_invocable: true\nagent_invocable: false\n---\nSkill body.\n";
        fs::write(skill_dir.join("SKILL.md"), spec_content).expect("expected value");
        std::os::unix::fs::symlink(&skill_dir, sub.join("loop")).expect("expected value");

        // A pattern reaches every entry of every walk, a skill directory's own
        // walk included, so a loop named by one is pruned like any other path.
        let ignore =
            IgnoreMatcher::compile(&["skills/s1/sub/loop".to_string()]).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs = load_skill_specs(&skills_dir, &ignore, &spec, &mut report)
            .expect("a pattern naming a loop inside a skill directory prunes it");

        assert_eq!(specs.len(), 1);
        let Spec::Skill(ref s) = specs[0] else {
            panic!("expected Skill variant")
        };
        assert_eq!(s.frontmatter.id, "s1");
    }

    #[test]
    fn test_ignore_prunes_a_loop_walkdir_catches_first() {
        let tmp = tempfile::tempdir().expect("expected value");
        let spec = tmp.path().join("spec");
        let agents_dir = spec.join("agents");
        fs::create_dir_all(&agents_dir).expect("expected value");
        std::os::unix::fs::symlink(&agents_dir, agents_dir.join("loop")).expect("expected value");

        let ignore = IgnoreMatcher::compile(&["agents/loop".to_string()]).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        // `walkdir` detects a link cycling back into the walk while producing
        // the entry, so this is the case manual driving exists for: a
        // `filter_entry` predicate is never consulted for a failed entry and
        // could not have reached this link.
        let specs = load_agent_specs(&agents_dir, &ignore, &spec, &mut report)
            .expect("a pattern naming a loop walkdir catches first prunes it");

        assert!(specs.is_empty());
        assert!(report.unused_pattern_indices().is_empty());
    }

    #[test]
    fn test_ignore_prunes_an_enclosing_symlink_this_guard_catches() {
        let tmp = tempfile::tempdir().expect("expected value");
        let spec = tmp.path().join("spec");
        let skills_dir = spec.join("skills");
        let agents_sub = spec.join("agents").join("sub");
        fs::create_dir_all(&skills_dir).expect("expected value");
        fs::create_dir_all(&agents_sub).expect("expected value");
        std::os::unix::fs::symlink(tmp.path(), skills_dir.join("up")).expect("expected value");
        std::os::unix::fs::symlink(tmp.path(), agents_sub.join("up")).expect("expected value");

        // A pattern decides membership before anything is resolved, so an
        // enclosing link named by one is pruned wherever it sits — under
        // `skills/`, where `reject_ancestor_loop` is what would catch it, and
        // under `agents/`, inside a walk.
        let ignore =
            IgnoreMatcher::compile(&["skills/up".to_string(), "agents/sub/up".to_string()])
                .expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        load_skill_specs(&skills_dir, &ignore, &spec, &mut report)
            .expect("an ignored enclosing link under skills/ is pruned");
        load_agent_specs(&spec.join("agents"), &ignore, &spec, &mut report)
            .expect("an ignored enclosing link under agents/ is pruned");
    }

    #[test]
    fn test_load_agent_specs_nested_symlink_cycle_named_on_the_link() {
        let tmp = tempfile::tempdir().expect("expected value");
        let agents_dir = tmp.path().join("agents");
        let sub = agents_dir.join("sub");
        fs::create_dir_all(&sub).expect("expected value");
        let a = sub.join("a");
        std::os::unix::fs::symlink(sub.join("b"), &a).expect("expected value");
        std::os::unix::fs::symlink(&a, sub.join("b")).expect("expected value");

        let err = load_agents_no_ignore(&agents_dir).expect_err("expected error");
        let full = format!("{err:#}");
        // A mutually-referential pair fails inside `walkdir`'s own `metadata`
        // call, so its ancestor tracking never runs and the fallback would head
        // the diagnostic with the walk root instead of the link.
        assert!(
            full.starts_with(&format!(
                "{}: symlink target could not be resolved",
                a.display()
            )) || full.starts_with(&format!(
                "{}: symlink target could not be resolved",
                sub.join("b").display()
            )),
            "a cycle must be named on the link, not on the walk root: {full}"
        );
    }

    #[test]
    fn test_load_agent_specs_nested_enclosing_symlink_rejected_before_descent() {
        let tmp = tempfile::tempdir().expect("expected value");
        let agents_dir = tmp.path().join("spec").join("agents");
        let sub = agents_dir.join("sub");
        fs::create_dir_all(&sub).expect("expected value");
        let up = sub.join("up");
        std::os::unix::fs::symlink(tmp.path(), &up).expect("expected value");

        let err = load_agents_no_ignore(&agents_dir).expect_err("expected error");
        let full = format!("{err:#}");
        // The link's own path, undoubled: `walkdir` would report this only
        // after reading the target, and name the second encounter of the link
        // reached through itself.
        assert!(
            full.starts_with(&format!("{}: symlink loop", up.display())),
            "a nested enclosing link must be refused before the descent: {full}"
        );
    }

    #[test]
    fn test_load_skill_specs_nested_symlink_to_a_pool_loads() {
        let tmp = tempfile::tempdir().expect("expected value");
        let pool = tmp.path().join("pool");
        fs::create_dir_all(&pool).expect("expected value");
        fs::write(pool.join("helper.sh"), "echo hi\n").expect("expected value");

        let skills_dir = tmp.path().join("spec").join("skills");
        let skill_dir = skills_dir.join("s");
        fs::create_dir_all(&skill_dir).expect("expected value");
        let spec_content = "---\nid: s\ndescription: A test skill\nuser_invocable: true\nagent_invocable: false\n---\nSkill body.\n";
        fs::write(skill_dir.join("SKILL.md"), spec_content).expect("expected value");
        std::os::unix::fs::symlink(&pool, skill_dir.join("scripts")).expect("expected value");

        let specs = load_skills_no_ignore(&skills_dir).expect("a pool link is not a cycle");
        assert_eq!(specs.len(), 1);
        let Spec::Skill(ref s) = specs[0] else {
            panic!("expected Skill variant")
        };
        assert!(
            s.supporting_files
                .keys()
                .any(|p| p.ends_with("scripts/helper.sh")),
            "the pooled file should be emitted at the link's own path: {:?}",
            s.supporting_files.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_load_agent_specs_root_enclosing_its_own_tree_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let spec = tmp.path().join("spec");
        fs::create_dir_all(&spec).expect("expected value");
        let agents_dir = spec.join("agents");
        std::os::unix::fs::symlink(tmp.path(), &agents_dir).expect("expected value");

        let err = load_agents_no_ignore(&agents_dir).expect_err("expected error");
        let full = format!("{err:#}");
        // Naming the link itself is what distinguishes the pre-walk guard from
        // `walkdir`'s own detection, which reports the ancestor it was standing
        // on after traversing the target in full.
        assert!(
            full.starts_with(&format!("{}: symlink loop", agents_dir.display())),
            "a root enclosing its own tree must fail before the walk: {full}"
        );
    }

    #[test]
    fn test_load_rule_specs_root_enclosing_its_own_tree_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let spec = tmp.path().join("spec");
        fs::create_dir_all(&spec).expect("expected value");
        let rules_dir = spec.join("rules");
        std::os::unix::fs::symlink(tmp.path(), &rules_dir).expect("expected value");

        let err = load_rules_no_ignore(&rules_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.starts_with(&format!("{}: symlink loop", rules_dir.display())),
            "a root enclosing its own tree must fail before the walk: {full}"
        );
    }

    #[test]
    fn test_load_skill_specs_root_enclosing_its_own_tree_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let spec = tmp.path().join("spec");
        fs::create_dir_all(&spec).expect("expected value");
        let skills_dir = spec.join("skills");
        std::os::unix::fs::symlink(tmp.path(), &skills_dir).expect("expected value");

        let err = load_skills_no_ignore(&skills_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.starts_with(&format!("{}: symlink loop", skills_dir.display())),
            "a root enclosing its own tree must fail before the walk: {full}"
        );
    }

    #[test]
    fn test_load_specs_resolving_spec_root_symlink_loads() {
        let tmp = tempfile::tempdir().expect("expected value");
        let pool = tmp.path().join("pool-agents");
        fs::create_dir_all(&pool).expect("expected value");
        let spec_content = "---\nid: pooled\ndescription: A pooled agent\n---\nPooled body.\n";
        fs::write(pool.join("pooled.md"), spec_content).expect("expected value");
        let agents_dir = tmp.path().join("agents");
        std::os::unix::fs::symlink(&pool, &agents_dir).expect("expected value");

        let specs = load_agents_no_ignore(&agents_dir).expect("expected value");
        assert_eq!(specs.len(), 1);
        let Spec::Agent(ref a) = specs[0] else {
            panic!("expected Agent variant")
        };
        assert_eq!(a.frontmatter.id, "pooled");
    }

    #[test]
    fn test_load_hook_specs_dangling_scripts_symlink_without_toml_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        fs::create_dir_all(&hooks_dir).expect("expected value");
        std::os::unix::fs::symlink(tmp.path().join("gone"), hooks_dir.join("scripts"))
            .expect("expected value");

        let err = load_hooks_no_ignore(&hooks_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.contains("symlink target does not exist"),
            "error: {full}"
        );
    }

    #[test]
    fn test_load_skill_specs_ancestor_symlink_loop_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        fs::create_dir_all(&skills_dir).expect("expected value");
        // Resolves to a real directory, so `metadata()` reports no `ELOOP`.
        std::os::unix::fs::symlink(&skills_dir, skills_dir.join("self-link"))
            .expect("expected value");

        let err = load_skills_no_ignore(&skills_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(full.contains("symlink loop"), "error: {full}");
    }

    #[test]
    fn test_load_skill_specs_directory_symlink_loop_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let skills_dir = tmp.path().join("skills");
        fs::create_dir_all(&skills_dir).expect("expected value");

        std::os::unix::fs::symlink(skills_dir.join("b"), skills_dir.join("a"))
            .expect("expected value");
        std::os::unix::fs::symlink(skills_dir.join("a"), skills_dir.join("b"))
            .expect("expected value");

        let err = load_skills_no_ignore(&skills_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.contains("symlink target could not be resolved"),
            "error: {full}"
        );
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
        // Whole dir pruned — exactly one report entry.
        assert_eq!(report.ignored.len(), 1);
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
        // the whole-directory prune, just the .md-ignored path.
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
    fn test_load_hook_specs_out_of_tree_symlink_resolved() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        let scripts_dir = hooks_dir.join("scripts");
        fs::create_dir_all(&scripts_dir).expect("expected value");

        let outside = tmp.path().join("shared-init.sh");
        fs::write(&outside, "#!/bin/sh\necho pooled").expect("expected value");
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).expect("expected value");
        std::os::unix::fs::symlink(&outside, scripts_dir.join("init.sh")).expect("expected value");

        let toml = "
[hooks.init]
events = [\"session_start\"]
script = \"scripts/init.sh\"
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
            .get(&PathBuf::from("scripts/init.sh"))
            .expect("symlinked file should appear in supporting_files");
        assert_eq!(link_file.content, b"#!/bin/sh\necho pooled");
        assert_eq!(link_file.mode, 0o755);
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
    fn test_load_hook_specs_scripts_root_enclosing_its_own_tree_rejected() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("spec").join("hooks");
        fs::create_dir_all(&hooks_dir).expect("expected value");
        fs::write(
            hooks_dir.join("hooks.toml"),
            "\n[hooks.init]\nevents = [\"session_start\"]\nscript = \"scripts/init.sh\"\n",
        )
        .expect("expected value");
        let scripts_dir = hooks_dir.join("scripts");
        std::os::unix::fs::symlink(tmp.path(), &scripts_dir).expect("expected value");

        let err = load_hooks_no_ignore(&hooks_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.starts_with(&format!("{}: symlink loop", scripts_dir.display())),
            "a scripts root enclosing its own tree must fail before the walk: {full}"
        );
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

    #[test]
    fn test_ignore_exempts_a_dangling_spec_root_link() {
        let tmp = tempfile::tempdir().expect("expected value");
        let spec = tmp.path().join("spec");
        fs::create_dir_all(&spec).expect("expected value");
        std::os::unix::fs::symlink(tmp.path().join("gone"), spec.join("agents"))
            .expect("expected value");

        let ignore = IgnoreMatcher::compile(&["agents".to_string()]).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs = load_agent_specs(&spec.join("agents"), &ignore, &spec, &mut report)
            .expect("an ignored root is never stat'd");

        assert!(specs.is_empty());
        assert_eq!(report.ignored.len(), 1);
        assert_eq!(report.ignored[0].rel_path, PathBuf::from("agents"));
    }

    #[test]
    fn test_ignore_exempts_a_dangling_skill_dir_link() {
        let tmp = tempfile::tempdir().expect("expected value");
        let spec = tmp.path().join("spec");
        let skills_dir = spec.join("skills");
        fs::create_dir_all(&skills_dir).expect("expected value");
        std::os::unix::fs::symlink(tmp.path().join("gone"), skills_dir.join("demo"))
            .expect("expected value");

        // The `read_dir` scan under `skills/` used to stat before checking.
        let ignore = IgnoreMatcher::compile(&["skills/demo".to_string()]).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs = load_skill_specs(&skills_dir, &ignore, &spec, &mut report)
            .expect("an ignored skill directory link is never resolved");

        assert!(specs.is_empty());
        assert_eq!(report.ignored[0].rel_path, PathBuf::from("skills/demo"));
    }

    #[test]
    fn test_ignore_exempts_a_dangling_link_under_hook_scripts() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        write_hook_fixture(&hooks_dir, "init", "session_start", "init.sh");
        std::os::unix::fs::symlink(
            tmp.path().join("gone.sh"),
            hooks_dir.join("scripts").join("broken.sh"),
        )
        .expect("expected value");

        let ignore = IgnoreMatcher::compile(&["hooks/scripts/broken.sh".to_string()])
            .expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs = load_hook_specs(&hooks_dir, &ignore, tmp.path(), &mut report)
            .expect("a pattern naming a dangling link under scripts/ exempts it");

        assert_eq!(specs.len(), 1);
        assert!(report.unused_pattern_indices().is_empty());
    }

    #[test]
    fn test_ignored_hooks_toml_suppresses_the_hook_set() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        write_hook_fixture(&hooks_dir, "init", "session_start", "init.sh");

        let ignore =
            IgnoreMatcher::compile(&["hooks/hooks.toml".to_string()]).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs = load_hook_specs(&hooks_dir, &ignore, tmp.path(), &mut report)
            .expect("an ignored hooks.toml removes the hook set, scripts included");

        assert!(specs.is_empty());
        assert_eq!(
            report.ignored[0].rel_path,
            PathBuf::from("hooks/hooks.toml")
        );
    }

    #[test]
    fn test_ignored_scripts_dir_raises_no_orphan_error() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        fs::create_dir_all(hooks_dir.join("scripts")).expect("expected value");
        fs::write(hooks_dir.join("scripts").join("orphan.sh"), "#!/bin/sh\n")
            .expect("expected value");

        let ignore =
            IgnoreMatcher::compile(&["hooks/scripts/**".to_string()]).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs = load_hook_specs(&hooks_dir, &ignore, tmp.path(), &mut report)
            .expect("an ignored scripts/ tree is hooks turned off, not scripts forgotten");

        assert!(specs.is_empty());
    }

    #[test]
    fn test_ignored_scripts_dir_enclosing_link_fails_on_the_loop() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        fs::create_dir_all(&hooks_dir).expect("expected value");
        std::os::unix::fs::symlink(tmp.path(), hooks_dir.join("scripts")).expect("expected value");

        let err = load_hooks_no_ignore(&hooks_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(
            full.contains("symlink loop detected"),
            "the loop names the link and outranks the orphan message: {full}"
        );
    }

    #[test]
    fn test_hook_referencing_an_ignored_script_names_the_pattern() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        write_hook_fixture(&hooks_dir, "deploy", "session_start", "deploy.sh");

        let ignore = IgnoreMatcher::compile(&["hooks/scripts/deploy.sh".to_string()])
            .expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let err = load_hook_specs(&hooks_dir, &ignore, tmp.path(), &mut report)
            .expect_err("expected error");

        let full = format!("{err:#}");
        assert!(full.contains("deploy"), "error: {full}");
        assert!(full.contains("scripts/deploy.sh"), "error: {full}");
        assert!(
            full.contains("hooks/scripts/deploy.sh"),
            "the message names the pattern: {full}"
        );
        assert!(!full.contains("does not exist"), "error: {full}");
    }

    #[test]
    fn test_hook_script_excluded_by_an_ancestor_pattern_names_that_pattern() {
        // Neither spelling matches the script's own path — both prune the
        // directory above it — so both must still report the exclusion.
        for pattern in ["hooks/scripts", "hooks/scripts/**"] {
            let tmp = tempfile::tempdir().expect("expected value");
            let hooks_dir = tmp.path().join("hooks");
            write_hook_fixture(&hooks_dir, "deploy", "session_start", "deploy.sh");

            let ignore = IgnoreMatcher::compile(&[pattern.to_string()]).expect("expected value");
            let mut report = LoadReport::with_matcher(&ignore);
            let err = load_hook_specs(&hooks_dir, &ignore, tmp.path(), &mut report)
                .expect_err("expected error");

            let full = format!("{err:#}");
            assert!(full.contains(pattern), "pattern {pattern}: {full}");
            assert!(
                !full.contains("does not exist"),
                "pattern {pattern}: the script is on disk, it was excluded: {full}"
            );
        }
    }

    #[test]
    fn test_hook_referencing_a_missing_script_still_says_does_not_exist() {
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        write_hook_fixture(&hooks_dir, "deploy", "session_start", "deploy.sh");
        fs::remove_file(hooks_dir.join("scripts").join("deploy.sh")).expect("expected value");

        let err = load_hooks_no_ignore(&hooks_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(full.contains("does not exist"), "error: {full}");
    }

    #[test]
    fn test_hook_script_reference_tolerates_curdir_components() {
        for script in ["./scripts/deploy.sh", "././scripts/deploy.sh"] {
            let tmp = tempfile::tempdir().expect("expected value");
            let hooks_dir = tmp.path().join("hooks");
            fs::create_dir_all(hooks_dir.join("scripts")).expect("expected value");
            fs::write(hooks_dir.join("scripts").join("deploy.sh"), "#!/bin/sh\n")
                .expect("expected value");
            fs::write(
                hooks_dir.join("hooks.toml"),
                format!("[hooks.deploy]\nevents = [\"session_start\"]\nscript = \"{script}\"\n"),
            )
            .expect("expected value");

            load_hooks_no_ignore(&hooks_dir).expect("expected value");
        }
    }

    #[test]
    fn test_ignored_dangling_link_is_recorded_once() {
        let tmp = tempfile::tempdir().expect("expected value");
        let spec = tmp.path().join("spec");
        let agents_dir = spec.join("agents");
        fs::create_dir_all(&agents_dir).expect("expected value");
        std::os::unix::fs::symlink(tmp.path().join("gone.md"), agents_dir.join("broken.md"))
            .expect("expected value");

        let ignore =
            IgnoreMatcher::compile(&["agents/broken.md".to_string()]).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        load_agent_specs(&agents_dir, &ignore, &spec, &mut report).expect("expected value");

        assert_eq!(report.ignored.len(), 1);
        assert_eq!(report.pattern_hits, vec![1]);
    }

    #[test]
    fn test_skill_whose_only_md_is_an_ignored_broken_link_is_skipped() {
        let tmp = tempfile::tempdir().expect("expected value");
        let spec = tmp.path().join("spec");
        let skills_dir = spec.join("skills");
        let skill_dir = skills_dir.join("demo");
        fs::create_dir_all(&skill_dir).expect("expected value");
        std::os::unix::fs::symlink(tmp.path().join("gone.md"), skill_dir.join("SKILL.md"))
            .expect("expected value");

        let ignore =
            IgnoreMatcher::compile(&["skills/demo/SKILL.md".to_string()]).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs = load_skill_specs(&skills_dir, &ignore, &spec, &mut report)
            .expect("an ignored spec file is never resolved");

        assert!(specs.is_empty());
    }

    #[test]
    fn test_skill_with_no_md_at_all_still_fails() {
        let tmp = tempfile::tempdir().expect("expected value");
        let spec = tmp.path().join("spec");
        let skill_dir = spec.join("skills").join("demo");
        fs::create_dir_all(skill_dir.join("scripts")).expect("expected value");

        let ignore = IgnoreMatcher::empty();
        let mut report = LoadReport::default();
        let err = load_skill_specs(&spec.join("skills"), &ignore, &spec, &mut report)
            .expect_err("expected error");
        assert!(err.to_string().contains("no .md file"), "error: {err}");
    }

    #[test]
    fn test_load_report_record_dedupes_a_repeated_path() {
        let ignore = IgnoreMatcher::compile(&["agents/x.md".to_string()]).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        report.record(PathBuf::from("agents/x.md"), 0);
        report.record(PathBuf::from("agents/x.md"), 0);

        assert_eq!(report.ignored.len(), 1);
        assert_eq!(report.pattern_hits, vec![1]);
    }

    #[test]
    fn test_ignored_plain_file_under_skills_root_counts_as_a_pattern_hit() {
        let tmp = tempfile::tempdir().expect("expected value");
        let spec = tmp.path().join("spec");
        let skills_dir = spec.join("skills");
        let skill_dir = skills_dir.join("s");
        fs::create_dir_all(&skill_dir).expect("expected value");
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nid: s\ndescription: s\nuser_invocable: true\nagent_invocable: false\n---\nbody.\n",
        )
        .expect("expected value");
        fs::write(skills_dir.join("README.md"), "# notes").expect("expected value");

        let ignore =
            IgnoreMatcher::compile(&["skills/README.md".to_string()]).expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs =
            load_skill_specs(&skills_dir, &ignore, &spec, &mut report).expect("expected value");

        assert_eq!(specs.len(), 1);
        assert_eq!(
            report.ignored[0].rel_path,
            PathBuf::from("skills/README.md")
        );
        assert!(report.unused_pattern_indices().is_empty());
    }

    #[test]
    fn test_ignored_supporting_file_named_by_a_child_pattern_is_not_emitted() {
        // Regression: the scan under a skill directory and the walk over it both
        // run the child probe, so a `foo/**` pattern cannot exclude a path at one
        // and admit it at the other — which reported a file as ignored and
        // emitted it anyway.
        let tmp = tempfile::tempdir().expect("expected value");
        let spec = tmp.path().join("spec");
        let skills_dir = spec.join("skills");
        let skill_dir = skills_dir.join("demo");
        fs::create_dir_all(&skill_dir).expect("expected value");
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nid: demo\ndescription: d\nuser_invocable: true\nagent_invocable: false\n---\nbody.\n",
        )
        .expect("expected value");
        fs::write(skill_dir.join("helper.sh"), "#!/bin/sh\n").expect("expected value");

        let ignore = IgnoreMatcher::compile(&["skills/demo/helper.sh/**".to_string()])
            .expect("expected value");
        let mut report = LoadReport::with_matcher(&ignore);
        let specs =
            load_skill_specs(&skills_dir, &ignore, &spec, &mut report).expect("expected value");

        assert_eq!(specs.len(), 1);
        let Spec::Skill(ref sk) = specs[0] else {
            panic!("expected Skill variant")
        };
        assert!(
            sk.supporting_files.is_empty(),
            "a path the report calls ignored must not be emitted: {:?}",
            sk.supporting_files.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            report.ignored[0].rel_path,
            PathBuf::from("skills/demo/helper.sh")
        );
    }

    #[test]
    fn test_hook_script_reference_is_case_sensitive() {
        // The declared script is looked up in the loaded set rather than stat'd,
        // so a case mismatch fails even on a case-insensitive filesystem, where
        // the old `is_file()` stat accepted it and emitted a hook command whose
        // path matched no emitted file.
        let tmp = tempfile::tempdir().expect("expected value");
        let hooks_dir = tmp.path().join("hooks");
        fs::create_dir_all(hooks_dir.join("scripts")).expect("expected value");
        fs::write(hooks_dir.join("scripts").join("deploy.sh"), "#!/bin/sh\n")
            .expect("expected value");
        fs::write(
            hooks_dir.join("hooks.toml"),
            "[hooks.deploy]\nevents = [\"session_start\"]\nscript = \"scripts/Deploy.sh\"\n",
        )
        .expect("expected value");

        let err = load_hooks_no_ignore(&hooks_dir).expect_err("expected error");
        let full = format!("{err:#}");
        assert!(full.contains("does not exist"), "error: {full}");
    }
}
