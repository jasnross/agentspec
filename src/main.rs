mod adapters;
mod cli;
mod compile;
mod config;
mod emit;
mod fragments;
mod parse;
mod presets;
mod spec;
mod sync;
mod validate;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use clap_complete::generate;
use cli::{Cli, Command, CommonArgs};
use compile::CompileResult;
use compile::compile_specs;
use config::AgentspecConfig;
use emit::{check_generated_state, write_generated_files};
use fragments::{build_environment, load_fragments};
use presets::ProviderPresetsMap;
use validate::validate_semantics;

use crate::config::Provider;
use crate::fragments::resolve_fragments;
use crate::parse::load_specs;
use crate::spec::NormalizedSpec;
use crate::validate::normalize_specs;

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

    // Load specs
    let specs = load_specs(&config)?;
    eprintln!("loaded {} specs", specs.len());

    // Resolve fragments
    let fragments_dir = config.resolve(&config.spec.fragments_dir);
    let fragment_map = load_fragments(&fragments_dir)?;
    let env = build_environment(&fragment_map)?;
    let specs = resolve_fragments(specs, &env)?;
    eprintln!(
        "resolved fragments ({} fragment templates loaded)",
        fragment_map.len()
    );

    let specs = normalize_specs(specs)?;
    eprintln!("normalized {} specs", specs.len());

    let presets = config.presets.clone();
    eprintln!("loaded {} preset(s)", presets.len());

    let semantic_errors = validate_semantics(&specs, &presets);
    if !semantic_errors.is_empty() {
        for err in &semantic_errors {
            eprintln!("error: {err}");
        }
        anyhow::bail!("{} semantic validation error(s)", semantic_errors.len());
    }
    eprintln!("semantic validation passed");

    match &cli.command {
        Command::Validate(_) => {
            eprintln!("validation complete");
        }
        Command::Sync(sync_args) => {
            if !sync_args.no_compile {
                let sync_compile_providers = if sync_args.common.provider.is_empty() {
                    config.configured_sync_providers()
                } else {
                    sync_args.common.provider.clone()
                };

                if !sync_compile_providers.is_empty() {
                    let (result, providers) =
                        run_compile(&specs, &presets, &sync_compile_providers, &config.providers)?;
                    let output_dir = config.resolve(&config.output.dir);
                    write_generated_files(&result.files, &output_dir, &providers)?;
                }
            }
            sync::run_sync(&config, sync_args)?;
        }
        Command::Compile(_) | Command::Check(_) => {
            let (result, providers) =
                run_compile(&specs, &presets, &args.provider, &config.providers)?;
            let output_dir = config.resolve(&config.output.dir);
            match &cli.command {
                Command::Compile(_) => {
                    write_generated_files(&result.files, &output_dir, &providers)?;
                    eprintln!(
                        "wrote {} files to {}",
                        result.files.len(),
                        output_dir.display()
                    );
                }
                Command::Check(_) => {
                    let check = check_generated_state(&result.files, &config.root_dir, &providers)?;
                    if check.is_clean() {
                        eprintln!("check passed: generated files are up to date");
                    } else {
                        for path in &check.missing {
                            eprintln!("missing: {path}");
                        }
                        for path in &check.outdated {
                            eprintln!("outdated: {path}");
                        }
                        for path in &check.unexpected {
                            eprintln!("unexpected: {path}");
                        }
                        anyhow::bail!("check failed: {} problem(s) found", check.problem_count());
                    }
                }
                Command::Validate(_) | Command::Completions { .. } | Command::Sync(_) => {
                    unreachable!()
                }
            }
        }
        Command::Completions { .. } => unreachable!("handled above"),
    }

    Ok(())
}

/// Runs the compile pipeline and reports the compiled file count. Returns the result and the
/// resolved target list so the caller can decide what to do next (write, check, or sync).
fn run_compile(
    specs: &[NormalizedSpec],
    presets: &ProviderPresetsMap,
    override_providers: &[Provider],
    config_providers: &[Provider],
) -> Result<(CompileResult, Vec<Provider>)> {
    let providers: Vec<Provider> = if override_providers.is_empty() {
        config_providers.to_vec()
    } else {
        override_providers.to_vec()
    };
    let result = compile_specs(specs, presets, &providers)?;
    eprintln!(
        "compiled {} files for {} provider(s)",
        result.files.len(),
        providers.len()
    );
    Ok((result, providers))
}
