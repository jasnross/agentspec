mod cli;
mod config;
mod emit;
mod sync;

use std::collections::HashMap;

use agentspec::compile::{self, AdapterConfig, CompileResult};
use agentspec::plan::compile_plan;
use agentspec::presets::ProviderPresetsMap;
use agentspec::provider::Provider;
use agentspec::specs::{SpecDirs, Specs, ValidatedSpecs};
use agentspec::templating::{TemplateContext, TemplatingResources, resolve_fragments};
use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use cli::{Cli, Command, CommonArgs};
use config::AgentspecConfig;
use emit::emit;
use strum::VariantArray;
use sync::{resolve_sync_targets, sync_plan};

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

    let args: &CommonArgs = match cli.command.args() {
        Some(args) => args,
        None => anyhow::bail!("internal error: command args unavailable"),
    };

    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let config = AgentspecConfig::discover(&cwd)?;

    let sources = config.resolve(&config.spec.sources_dir);
    let dirs = SpecDirs {
        agents: sources.join("agents"),
        skills: sources.join("skills"),
        rules: sources.join("rules"),
    };

    match &cli.command {
        Command::Validate(_) => {
            let validated = load_and_validate(&config, &dirs)?;
            let templating = load_templating(&config)?;
            // Check template syntax by resolving with unprefixed context.
            // `None` provider → `tool()` passes canonical names through unchanged.
            let env = templating.build_environment(None)?;
            let context = TemplateContext::from_specs(validated.specs());
            resolve_fragments(validated.into_specs(), &env, &context)?;
            eprintln!("validation complete");
        }
        Command::Sync(sync_args) => {
            let validated = load_and_validate(&config, &dirs)?;
            let templating = load_templating(&config)?;
            let targets = resolve_sync_targets(&config, sync_args)?;
            let sync_providers: Vec<Provider> = targets.iter().map(|(p, _)| *p).collect();

            let adapter_configs = AgentspecConfig::adapter_configs(&targets);

            if sync_args.dry_run {
                eprint!("[dry-run] ");
            }
            let (result, _) = run_compile(
                &validated,
                &templating,
                &config.presets,
                &sync_providers,
                &adapter_configs,
            )?;

            let home = home_dir()?;
            let plan = sync_plan(&result, &targets, &home, &cwd)?;
            emit(&plan, sync_args.dry_run, sync_args.verbose)?;
        }
        Command::Compile(_) => {
            let validated = load_and_validate(&config, &dirs)?;
            let templating = load_templating(&config)?;

            let adapter_configs = AgentspecConfig::adapter_configs(&config.sync_targets());

            let providers: Vec<Provider> = if args.provider.is_empty() {
                Provider::VARIANTS.to_vec()
            } else {
                args.provider.clone()
            };

            let (result, providers) = run_compile(
                &validated,
                &templating,
                &config.presets,
                &providers,
                &adapter_configs,
            )?;
            let output_dir = config.resolve(&config.compile.output_dir);
            let plan = compile_plan(&result, &output_dir, &providers);
            emit(&plan, false, false)?;
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

fn load_and_validate(config: &AgentspecConfig, dirs: &SpecDirs) -> Result<ValidatedSpecs> {
    Specs::load(dirs)?
        .normalize()
        .validate(&config.presets)
        .map_err(|errors| {
            for e in &errors {
                eprintln!("error: {e}");
            }
            anyhow::anyhow!("{} semantic validation error(s)", errors.len())
        })
}

fn load_templating(config: &AgentspecConfig) -> Result<TemplatingResources> {
    let sources = config.resolve(&config.spec.sources_dir);
    TemplatingResources::load(&sources.join("fragments"))
}

/// Runs the compile step and reports the compiled file count. Returns the result and the
/// provider list so the caller can decide what to do next (write or sync).
fn run_compile(
    validated: &ValidatedSpecs,
    templating: &TemplatingResources,
    presets: &ProviderPresetsMap,
    providers: &[Provider],
    adapter_configs: &HashMap<Provider, AdapterConfig>,
) -> Result<(CompileResult, Vec<Provider>)> {
    let providers = providers.to_vec();
    let result = compile::run(validated, templating, presets, &providers, adapter_configs)?;
    let n = providers.len();
    eprintln!(
        "compiled {} files for {n} {}",
        result.files.len(),
        if n == 1 { "provider" } else { "providers" }
    );
    Ok((result, providers))
}

/// Returns the current user's home directory.
fn home_dir() -> Result<std::path::PathBuf> {
    home::home_dir().ok_or_else(|| anyhow::anyhow!("could not determine home directory"))
}
