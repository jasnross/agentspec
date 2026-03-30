mod manifest;
pub mod provider;
mod strategy;

use std::path::{Path, PathBuf};

use agentspec::provider::Provider;
use anyhow::{Context, Result, bail};
use manifest::Manifest;
use provider::{
    SyncKind, all_sync_kinds, generated_source_dir, patch_opencode_instructions, resolve_dest_dir,
};
use strategy::{NamePrefixMode, SyncEntry, apply_strip_name, sync_copied_dir, sync_symlinked_dir};

use crate::cli::SyncArgs;
use crate::config::{AgentspecConfig, SyncMode, SyncOverrides, SyncStrategy, SyncTargetConfig};

/// Runs the sync command: distributes generated files to each tool's config directory.
///
/// If `--no-compile` was not given, the caller is responsible for having already run the
/// compile pipeline and for providing the generated output at `config.output.dir`.
pub fn run_sync(config: &AgentspecConfig, args: &SyncArgs) -> Result<()> {
    let home = home_dir()?;
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let output_dir = config.resolve(&config.output.dir);

    let ctx = SyncContext {
        output_dir: &output_dir,
        home: &home,
        cwd: &cwd,
        dry_run: args.dry_run,
    };

    let sync_overrides = SyncOverrides {
        mode: args.mode,
        strategy: args.strategy,
        dest: args.dest.clone(),
        force: args.force,
    };

    let has_explicit_provider_selection = !args.common.provider.is_empty();
    let providers = if has_explicit_provider_selection {
        args.common.provider.clone()
    } else {
        config.configured_sync_providers()
    };

    if providers.is_empty() {
        // FIXME: move user messaging to cli
        bail!(
            "no sync providers are configured; add [sync.<provider>] in agentspec.toml, or run CLI-only sync with an explicit target (for example: --target claude --mode user|project or --target claude --dest <path>)"
        );
    }

    let mut resolved_targets: Vec<(Provider, SyncTargetConfig)> = Vec::new();
    for provider in providers {
        // FIXME: Require passing a SyncIntent instead of resolving it here
        let intent =
            config.resolve_sync_intent(provider, &sync_overrides, has_explicit_provider_selection);

        if !intent.has_explicit_config && !intent.cli_only_allowed {
            // FIXME: move user messaging to cli
            bail!(
                "sync config for {provider} is not configured; add [sync.{provider}] in agentspec.toml, or pass explicit CLI-only sync arguments with --target {provider} and --mode user|project, or --target {provider} --dest <path>"
            );
        }

        intent.target.validate_for_sync(provider)?;
        resolved_targets.push((provider, intent.target));
    }

    let mut all_entries: Vec<SyncEntry> = Vec::new();

    for (provider, target) in &resolved_targets {
        eprintln!(
            "syncing {provider} (mode={:?}, strategy={:?})",
            target.mode, target.strategy
        );

        sync_provider(*provider, target, &ctx, &mut all_entries)?;
    }

    print_summary(&all_entries);
    Ok(())
}

/// Context passed through the sync loop to reduce argument counts.
struct SyncContext<'a> {
    output_dir: &'a Path,
    home: &'a Path,
    cwd: &'a Path,
    dry_run: bool,
}

/// Syncs all kinds for a single provider and appends entries to `all_entries`.
fn sync_provider(
    provider: Provider,
    target: &SyncTargetConfig,
    ctx: &SyncContext<'_>,
    all_entries: &mut Vec<SyncEntry>,
) -> Result<()> {
    let mut skills_dest: Option<PathBuf> = None;

    for kind in all_sync_kinds(provider) {
        let (entries, dest_dir) = sync_kind(provider, kind, target, ctx)?;

        // Track skills dest dir for strip_name post-processing
        if kind == SyncKind::Skills && !entries.is_empty() {
            skills_dest = Some(dest_dir.clone());
        }

        // After syncing OpenCode rules, patch opencode.json instructions
        if provider == Provider::OpenCode && kind == SyncKind::Rules {
            let config_dir = opencode_config_dir(target, ctx.home, ctx.cwd);
            patch_opencode_instructions(&dest_dir, &config_dir, ctx.dry_run)?;
        }

        all_entries.extend(entries);
    }

    // Apply strip_name post-processing on skills when using copy strategy
    if target.strip_name
        && target.strategy == SyncStrategy::Copy
        && let Some(ref dir) = skills_dest
    {
        apply_strip_name(dir, ctx.dry_run)?;
    }

    Ok(())
}

/// Syncs one `(provider, kind)` pair according to the resolved target config.
/// Returns `(entries, dest_dir)` so callers can reuse the resolved destination
/// without re-resolving it.
fn sync_kind(
    provider: Provider,
    kind: SyncKind,
    target: &SyncTargetConfig,
    ctx: &SyncContext<'_>,
) -> Result<(Vec<SyncEntry>, PathBuf)> {
    let source_dir = generated_source_dir(provider, kind, ctx.output_dir);
    let mut dest_dir = resolve_dest_dir(provider, kind, target, ctx.home, ctx.cwd)?;
    let mut file_prefix: Option<String> = None;

    if let Some(prefix) = target.prefix.as_deref() {
        if provider == Provider::OpenCode && kind == SyncKind::Commands {
            dest_dir = dest_dir.join(prefix);
        } else if kind != SyncKind::Rules {
            file_prefix = Some(format!("{prefix}-"));
        }
    }

    if !source_dir.is_dir() {
        eprintln!("  skip {provider}/{}: no generated output", kind.dir_name());
        return Ok((Vec::new(), dest_dir));
    }

    // FIXME: move output to cli instead
    eprintln!(
        "  {} {} → {}",
        if ctx.dry_run { "[dry-run]" } else { "sync" },
        source_dir.display(),
        dest_dir.display()
    );

    let entries = match target.strategy {
        SyncStrategy::Symlink => sync_symlinked_dir(
            &source_dir,
            &dest_dir,
            file_prefix.as_deref(),
            target.allow_overwrite,
            ctx.dry_run,
        )?,
        SyncStrategy::Copy => {
            let mut manifest = Manifest::load(&dest_dir)?;
            let name_prefix = if provider == Provider::Claude {
                match kind {
                    SyncKind::Agents => target
                        .prefix
                        .as_deref()
                        .map(|prefix| (prefix, NamePrefixMode::Agents)),
                    SyncKind::Skills => target
                        .prefix
                        .as_deref()
                        .map(|prefix| (prefix, NamePrefixMode::Skills)),
                    // FIXME: Claude does support rules
                    SyncKind::Commands | SyncKind::Rules => None,
                }
            } else {
                None
            };
            let result = sync_copied_dir(
                &source_dir,
                &dest_dir,
                &mut manifest,
                file_prefix.as_deref(),
                name_prefix,
                target.allow_overwrite,
                ctx.dry_run,
            )?;
            if !ctx.dry_run {
                manifest.save(&dest_dir)?;
            }
            result
        }
    };

    Ok((entries, dest_dir))
}

/// Resolves the home directory from the environment.
fn home_dir() -> Result<PathBuf> {
    std::env::var("HOME")
        .context("HOME environment variable not set")
        .map(PathBuf::from)
}

/// Resolves the `OpenCode` config directory for the given sync target config.
///
/// In `Path` mode the config dir is derived from the rules path's parent directory,
/// which follows the convention that `opencode.json` lives one level above the `rules/`
/// subdirectory (e.g. `~/custom/opencode/rules` → `~/custom/opencode`). Users who
/// place rules at a path that doesn't follow this convention should set
/// `mode = "user"` or `mode = "project"` instead.
fn opencode_config_dir(target: &SyncTargetConfig, home: &Path, cwd: &Path) -> PathBuf {
    match target.mode {
        SyncMode::User => home.join(".config").join("opencode"),
        SyncMode::Project => cwd.join(".opencode"),
        SyncMode::Path => target
            .rules
            .as_deref()
            .and_then(|r| {
                provider::expand_tilde(r, home)
                    .parent()
                    .map(Path::to_path_buf)
            })
            .unwrap_or_else(|| home.join(".config").join("opencode")),
    }
}

/// FIXME: Move to CLI instead
/// Prints a summary of sync actions across all providers.
fn print_summary(all_entries: &[SyncEntry]) {
    let created = all_entries
        .iter()
        .filter(|e| e.action == strategy::SyncAction::Created)
        .count();
    let updated = all_entries
        .iter()
        .filter(|e| e.action == strategy::SyncAction::Updated)
        .count();
    let removed = all_entries
        .iter()
        .filter(|e| e.action == strategy::SyncAction::Removed)
        .count();
    let backed_up = all_entries
        .iter()
        .filter(|e| e.action == strategy::SyncAction::BackedUp)
        .count();
    let unchanged = all_entries
        .iter()
        .filter(|e| e.action == strategy::SyncAction::Unchanged)
        .count();
    eprintln!(
        "sync complete: {created} created, {updated} updated, {removed} removed, \
         {backed_up} backed up, {unchanged} unchanged"
    );
}
