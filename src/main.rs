mod cli;
mod config;
mod emit;
mod sync;

use agentspec::compile::{self, CompileResult};
use agentspec::plan::compile_plan;
use agentspec::presets::ProviderPresetsMap;
use agentspec::provider::Provider;
use agentspec::specs::{SpecDirs, Specs};
use agentspec::templating::{self, ResolvedSpecs, TemplatingConfig};
use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use clap_complete::generate;
use cli::{Cli, Command, CommonArgs};
use config::AgentspecConfig;
use emit::emit;
use sync::{resolve_sync_targets, sync_plan};

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle completions before any I/O — no config or spec loading needed.
    if let Command::Completions { shell } = cli.command {
        generate(
            shell,
            &mut Cli::command(),
            "agentspec",
            &mut std::io::stdout(),
        );
        return Ok(());
    }

    let args: &CommonArgs = match cli.command.args() {
        Some(args) => args,
        None => anyhow::bail!("internal error: command args unavailable"),
    };

    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let mut config = AgentspecConfig::discover(&cwd)?;
    config.apply_overrides(args);

    let dirs = SpecDirs {
        agents: config.resolve(&config.spec.agents_dir),
        skills: config.resolve(&config.spec.skills_dir),
        rules: config.resolve(&config.spec.rules_dir),
    };

    let validated = Specs::load(&dirs)?
        .normalize()
        .validate(&config.presets)
        .map_err(|errors| {
            for e in &errors {
                eprintln!("error: {e}");
            }
            anyhow::anyhow!("{} semantic validation error(s)", errors.len())
        })?;

    let templating_config = TemplatingConfig {
        fragments_dir: config.resolve(&config.spec.fragments_dir),
    };
    let resolved = templating::resolve(validated, &templating_config)?;

    match &cli.command {
        Command::Validate(_) => {
            eprintln!("validation complete");
        }
        Command::Sync(sync_args) => {
            let targets = resolve_sync_targets(&config, sync_args)?;
            let sync_providers: Vec<Provider> = targets.iter().map(|(p, _)| *p).collect();

            let (result, _) = run_compile(
                resolved,
                &config.presets,
                &sync_providers,
                &config.providers,
            )?;

            let home = home_dir()?;
            let plan = sync_plan(&result, &targets, &home, &cwd)?;
            emit(&plan, sync_args.dry_run)?;
        }
        Command::Compile(_) => {
            let (result, providers) =
                run_compile(resolved, &config.presets, &args.provider, &config.providers)?;
            let output_dir = config.resolve(&config.output.dir);
            let plan = compile_plan(&result, &output_dir, &providers);
            emit(&plan, false)?;
            eprintln!(
                "wrote {} files to {}",
                result.files.len(),
                output_dir.display()
            );
        }
        Command::Completions { .. } => unreachable!("handled above"),
    }

    Ok(())
}

/// Runs the compile step and reports the compiled file count. Returns the result and the
/// resolved target list so the caller can decide what to do next (write or sync).
fn run_compile(
    resolved: ResolvedSpecs,
    presets: &ProviderPresetsMap,
    override_providers: &[Provider],
    config_providers: &[Provider],
) -> Result<(CompileResult, Vec<Provider>)> {
    let providers: Vec<Provider> = if override_providers.is_empty() {
        config_providers.to_vec()
    } else {
        override_providers.to_vec()
    };
    let result = compile::run(resolved, presets, &providers)?;
    eprintln!(
        "compiled {} files for {} provider(s)",
        result.files.len(),
        providers.len()
    );
    Ok((result, providers))
}

/// Returns the current user's home directory.
fn home_dir() -> Result<std::path::PathBuf> {
    home::home_dir().ok_or_else(|| anyhow::anyhow!("could not determine home directory"))
}
