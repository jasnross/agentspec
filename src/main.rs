mod cli;
mod config;
mod emit;
mod hook;
mod remove;
mod sync;

use std::collections::HashMap;

use agentspec::compile::{
    self, AdapterConfig, CompileDiagnostics, CompileResult, ProviderCompileTarget,
};
use agentspec::plan::{compile_plan, expand_tilde};
use agentspec::presets::ProviderPresetsMap;
use agentspec::provider::Provider;
use agentspec::specs::{IgnoreMatcher, LoadReport, SpecDirs, Specs, ValidatedSpecs};
use agentspec::templating::{TemplateContext, Templating, resolve_fragments};
use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use cli::{Cli, Command};
use config::{AgentspecConfig, SyncTargetConfig};
use emit::{emit_compile, emit_remove, emit_sync};
use strum::VariantArray;
use sync::{resolve_sync_targets, sync_plan};

// `main` is naturally long — one match arm per subcommand. Extracting each
// arm into its own helper would buy little and obscure the dispatch shape.
#[allow(clippy::too_many_lines)]
fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle completions before any I/O — no config or spec loading needed.
    if let Command::Completions { shell } = cli.command {
        clap_complete::generate(
            shell,
            &mut Cli::command(),
            "agentspec",
            &mut std::io::stdout(),
        );
        return Ok(());
    }

    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let config = AgentspecConfig::discover(&cwd)?;

    let sources = config.resolve(&config.spec.sources_dir);
    let ignore = config.spec.compile_ignore_matcher()?;
    let dirs = SpecDirs {
        agents: sources.join("agents"),
        skills: sources.join("skills"),
        rules: sources.join("rules"),
        hooks: sources.join("hooks"),
        ignore,
        ignore_anchor: sources,
    };

    match &cli.command {
        Command::Validate(_) => {
            let (validated, report) = load_and_validate(&config, &dirs)?;
            // `validate` always shows the full listing — this is the command
            // users run to inspect their `[spec].ignore` effect.
            surface_load_report(&dirs.ignore, &report, ReportDisplay::Full);

            // Config validation runs after spec validation: spec errors are
            // more fundamental — if specs fail to load or validate, config
            // validation is secondary and the user should fix spec issues first.
            let config_errors = config.validate_sync_config();
            if !config_errors.is_empty() {
                for e in &config_errors {
                    eprintln!("error: {e}");
                }
                anyhow::bail!("{} config validation error(s)", config_errors.len());
            }

            let templating = load_templating(&config)?;
            let context = TemplateContext::from_specs(validated.specs());
            resolve_fragments(validated.into_specs(), &templating, None, &context)?;
            eprintln!("validation complete");
        }
        Command::Sync(sync_args) => {
            let (validated, report) = load_and_validate(&config, &dirs)?;
            let display = if sync_args.dry_run || sync_args.common.verbose {
                ReportDisplay::Full
            } else {
                ReportDisplay::WarningsOnly
            };
            surface_load_report(&dirs.ignore, &report, display);
            let templating = load_templating(&config)?;
            let targets = resolve_sync_targets(&config, sync_args)?;
            let sync_providers: Vec<Provider> = targets.iter().map(|(p, _)| *p).collect();

            let adapter_configs = AgentspecConfig::adapter_configs(&targets);
            let compile_targets = compile_targets_from(&targets);

            if sync_args.dry_run {
                eprint!("[dry-run] ");
            }
            let home = home_dir()?;
            let (mut result, diagnostics) = run_compile(
                &validated,
                &templating,
                &config.presets,
                &sync_providers,
                &adapter_configs,
                &compile_targets,
                &home,
                &cwd,
            )?;
            surface_compile_diagnostics(&diagnostics, display);

            let plan = sync_plan(&mut result, &targets, &home)?;
            emit_sync(&plan, sync_args.dry_run, sync_args.common.verbose)?;
        }
        Command::Remove(remove_args) => {
            let targets = remove::resolve_remove_targets(&config, remove_args)?;
            if targets.is_empty() {
                eprintln!("nothing to remove");
            } else {
                let home = home_dir()?;
                let plan = remove::remove_plan(&targets, &home, &cwd);
                emit_remove(&plan, remove_args.dry_run)?;
            }
        }
        Command::Compile(compile_args) => {
            let (validated, report) = load_and_validate(&config, &dirs)?;
            let display = if compile_args.verbose {
                ReportDisplay::Full
            } else {
                ReportDisplay::WarningsOnly
            };
            surface_load_report(&dirs.ignore, &report, display);
            let templating = load_templating(&config)?;

            let sync_targets = config.sync_targets();
            let adapter_configs = AgentspecConfig::adapter_configs(&sync_targets);
            // The `compile` command path has no sync destination — adapters
            // produce canonical, provider-config-dir-agnostic output anchored
            // at `SyncDestinationMode::Compile`. Pass an empty map so each
            // adapter falls back to `ProviderCompileTarget::default`. Plugin
            // manifests are gated on `ctx.mode == Plugin` inside each adapter,
            // so any `plugin-*` TOML fields set under `[sync.<provider>]`
            // never leak into `generated/<provider>/` output.
            let compile_targets: HashMap<Provider, ProviderCompileTarget> = HashMap::new();

            let providers: Vec<Provider> = if compile_args.provider.is_empty() {
                Provider::VARIANTS.to_vec()
            } else {
                compile_args.provider.clone()
            };

            let home = home_dir()?;
            let (result, diagnostics) = run_compile(
                &validated,
                &templating,
                &config.presets,
                &providers,
                &adapter_configs,
                &compile_targets,
                &home,
                &cwd,
            )?;
            surface_compile_diagnostics(&diagnostics, display);
            let output_dir = config.resolve(&config.compile.output_dir);
            let plan = compile_plan(&result, &output_dir, &providers);
            emit_compile(&plan, false)?;
            eprintln!(
                "wrote {} files to {}",
                result.files.len(),
                output_dir.display()
            );
        }
        Command::Hook(hook_cmd) => match &hook_cmd.command {
            cli::HookSubcommand::Test(test_args) => {
                let (validated, _report) = load_and_validate(&config, &dirs)?;
                hook::run_hook_test(test_args, &dirs, &validated)?;
            }
        },
        Command::Completions { .. } => unreachable!("handled above"),
    }

    Ok(())
}

/// Display mode for `surface_load_report`.
#[derive(Clone, Copy, Debug)]
enum ReportDisplay {
    /// Unused-pattern warnings only (the default for `compile` / `sync`
    /// without `--verbose`).
    WarningsOnly,
    /// Unused-pattern warnings plus the full ignored-path listing.
    Full,
}

/// Print ignore-pattern diagnostics to stderr.
///
/// Unused-pattern warnings are always printed. The ignored-path listing is
/// printed only for `Full`.
fn surface_load_report(matcher: &IgnoreMatcher, report: &LoadReport, display: ReportDisplay) {
    // Always: unused-pattern warnings.
    for idx in report.unused_pattern_indices() {
        if let Some(pat) = matcher.pattern(idx) {
            eprintln!("warning: ignore pattern '{pat}' matched no files");
        }
    }

    if matches!(display, ReportDisplay::WarningsOnly) || report.ignored.is_empty() {
        return;
    }

    for line in format_ignored_listing(matcher, report) {
        eprintln!("{line}");
    }
}

/// Produce the ignored-path listing as a sequence of lines, ready for stderr.
///
/// Pure function — takes a matcher + report and returns printable strings.
/// Split from `surface_load_report` to make formatting unit-testable.
fn format_ignored_listing(matcher: &IgnoreMatcher, report: &LoadReport) -> Vec<String> {
    if report.ignored.is_empty() {
        return Vec::new();
    }

    let pruned_count = report.ignored.iter().filter(|p| p.pruned).count();
    let total = report.ignored.len();

    let mut lines = Vec::with_capacity(1 + total);
    let paths_word = if total == 1 { "path" } else { "paths" };
    let summary = if pruned_count == 0 {
        format!("ignoring {total} {paths_word}:")
    } else {
        let subtrees_word = if pruned_count == 1 {
            "subtree"
        } else {
            "subtrees"
        };
        format!("ignoring {total} {paths_word} ({pruned_count} pruned {subtrees_word}):")
    };
    lines.push(summary);

    let max_rel = report
        .ignored
        .iter()
        .map(|p| p.rel_path.display().to_string().len())
        .max()
        .unwrap_or(0);
    for entry in &report.ignored {
        let pattern = matcher.pattern(entry.pattern_index).unwrap_or("<unknown>");
        let suffix = if entry.pruned { ", pruned" } else { "" };
        lines.push(format!(
            "  {:<max_rel$}  (pattern: {pattern}{suffix})",
            entry.rel_path.display(),
        ));
    }
    lines
}

/// Load specs and run semantic validation, returning the validated specs
/// alongside the [`LoadReport`] so the caller can surface ignore-pattern
/// diagnostics via [`surface_load_report`].
fn load_and_validate(
    config: &AgentspecConfig,
    dirs: &SpecDirs,
) -> Result<(ValidatedSpecs, LoadReport)> {
    let (specs, report) = Specs::load(dirs)?;
    let validated = specs.validate(&config.presets).map_err(|errors| {
        for e in &errors {
            eprintln!("error: {e}");
        }
        anyhow::anyhow!("{} semantic validation error(s)", errors.len())
    })?;
    Ok((validated, report))
}

fn load_templating(config: &AgentspecConfig) -> Result<Templating> {
    let sources = config.resolve(&config.spec.sources_dir);
    let extra_dirs = resolve_extra_fragment_dirs(config)?;
    Templating::load(&sources.join("fragments"), &extra_dirs)
}

fn resolve_extra_fragment_dirs(config: &AgentspecConfig) -> Result<Vec<std::path::PathBuf>> {
    if config.spec.extra_fragment_dirs.is_empty() {
        return Ok(Vec::new());
    }
    let home = home_dir()?;
    Ok(config
        .spec
        .extra_fragment_dirs
        .iter()
        .map(|p| {
            let expanded = expand_tilde(&p.to_string_lossy(), &home);
            if expanded.is_relative() {
                config.resolve(&expanded)
            } else {
                expanded
            }
        })
        .collect())
}

/// Runs the compile step and reports the compiled file count. The compile
/// stage owns its own diagnostics ([`CompileDiagnostics`]) and returns them
/// directly — this thin wrapper exists only to print the file-count line.
#[allow(clippy::too_many_arguments)] // params mirror compile::run; threading them as a struct
// would just rename, not reduce, the call-site noise.
fn run_compile(
    validated: &ValidatedSpecs,
    templating: &Templating,
    presets: &ProviderPresetsMap,
    providers: &[Provider],
    adapter_configs: &HashMap<Provider, AdapterConfig>,
    compile_targets: &HashMap<Provider, ProviderCompileTarget>,
    home: &std::path::Path,
    cwd: &std::path::Path,
) -> Result<(CompileResult, CompileDiagnostics)> {
    let (result, diagnostics) = compile::run(
        validated,
        templating,
        presets,
        providers,
        adapter_configs,
        compile_targets,
        home,
        cwd,
    )?;
    let n = providers.len();
    eprintln!(
        "compiled {} files for {n} {}",
        result.files.len(),
        if n == 1 { "provider" } else { "providers" }
    );
    Ok((result, diagnostics))
}

/// Build per-provider [`ProviderCompileTarget`] entries from the binary's
/// `SyncTargetConfig` list. Mirrors `AgentspecConfig::adapter_configs`'s
/// shape: providers absent from `targets` are absent from the map (the
/// orchestrator falls back to [`ProviderCompileTarget::default`]).
fn compile_targets_from(
    targets: &[(Provider, SyncTargetConfig)],
) -> HashMap<Provider, ProviderCompileTarget> {
    targets
        .iter()
        .map(|(p, t)| {
            (
                *p,
                ProviderCompileTarget {
                    mode: t.mode.to_destination_mode(),
                    target_dir: t.dir.as_deref().map(std::path::PathBuf::from),
                    overwrite: t.overwrite,
                },
            )
        })
        .collect()
}

/// Print compile diagnostics to stderr.
///
/// Default mode prints one line per provider with skipped hooks (`opencode:
/// skipped N hook(s)`). Full mode additionally lists each skipped spec id —
/// matching the format established by `surface_load_report` for `[spec].ignore`
/// listings under `--verbose`.
fn surface_compile_diagnostics(diagnostics: &CompileDiagnostics, display: ReportDisplay) {
    use std::collections::BTreeMap;

    // Cross-provider portability warnings — printed regardless of display
    // mode since each represents a real behavioral asymmetry the user
    // should know about. The set is small (at most a handful per run) and
    // each line is `agentspec warning:`-prefixed for easy filtering.
    for warning in &diagnostics.warnings {
        eprintln!("agentspec warning: {}", warning.message());
    }

    if diagnostics.skipped_hooks.is_empty() {
        return;
    }

    let mut by_provider: BTreeMap<Provider, Vec<&str>> = BTreeMap::new();
    for skip in &diagnostics.skipped_hooks {
        by_provider
            .entry(skip.provider)
            .or_default()
            .push(skip.hook_id.as_str());
    }

    for (provider, ids) in &by_provider {
        let n = ids.len();
        let hook_word = if n == 1 { "hook" } else { "hooks" };
        eprintln!("{provider}: skipped {n} {hook_word}");
        if matches!(display, ReportDisplay::Full) {
            for id in ids {
                eprintln!("{provider}: skipped hook {id}");
            }
        }
    }
}

/// Returns the current user's home directory.
fn home_dir() -> Result<std::path::PathBuf> {
    home::home_dir().ok_or_else(|| anyhow::anyhow!("could not determine home directory"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use agentspec::specs::LoadReport;

    use super::*;

    #[test]
    fn test_format_ignored_listing_empty_returns_no_lines() {
        let matcher = IgnoreMatcher::empty();
        let report = LoadReport::default();
        assert!(format_ignored_listing(&matcher, &report).is_empty());
    }

    #[test]
    fn test_format_ignored_listing_renders_file_and_pruned_entries() {
        let matcher =
            IgnoreMatcher::compile(&["**/*.bats".to_string(), "skills/deploy/**".to_string()])
                .expect("expected value");
        let mut report = LoadReport::with_matcher(&matcher);
        report.record(PathBuf::from("skills/s/test.bats"), 0, false);
        report.record(PathBuf::from("skills/deploy"), 1, true);

        let lines = format_ignored_listing(&matcher, &report);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "ignoring 2 paths (1 pruned subtree):");
        assert!(lines[1].contains("skills/s/test.bats"));
        assert!(lines[1].contains("(pattern: **/*.bats)"));
        assert!(lines[2].contains("skills/deploy"));
        assert!(lines[2].contains("(pattern: skills/deploy/**, pruned)"));
    }

    #[test]
    fn test_format_ignored_listing_drops_pruned_parenthetical_when_zero() {
        // When no subtrees are pruned, the summary should be "ignoring 1 path:"
        // — not "ignoring 1 path (0 pruned subtrees):".
        let matcher = IgnoreMatcher::compile(&["**/*.bats".to_string()]).expect("expected value");
        let mut report = LoadReport::with_matcher(&matcher);
        report.record(PathBuf::from("skills/s/test.bats"), 0, false);

        let lines = format_ignored_listing(&matcher, &report);
        assert_eq!(lines[0], "ignoring 1 path:");
    }

    #[test]
    fn test_format_ignored_listing_plural_pruned() {
        let matcher =
            IgnoreMatcher::compile(&["skills/a/**".to_string(), "skills/b/**".to_string()])
                .expect("expected value");
        let mut report = LoadReport::with_matcher(&matcher);
        report.record(PathBuf::from("skills/a"), 0, true);
        report.record(PathBuf::from("skills/b"), 1, true);

        let lines = format_ignored_listing(&matcher, &report);
        assert_eq!(lines[0], "ignoring 2 paths (2 pruned subtrees):");
    }
}
