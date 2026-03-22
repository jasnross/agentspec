mod adapters;
mod cli;
mod compile;
mod config;
mod emit;
mod format;
mod fragments;
mod model;
mod parse;
mod schema;
mod tools;
mod types;
mod validate;

use anyhow::{Context, Result};
use clap::Parser;

use cli::{Cli, Command};
use compile::compile_specs;
use config::AgentspecConfig;
use emit::{check_generated_state, write_generated_files, write_manifest};
use fragments::{build_environment, load_fragments, resolve_fragments};
use parse::load_canonical_specs;
use schema::load_schemas;
use validate::{normalize_specs, validate_schema, validate_semantics};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let args = cli.command.args();

    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let mut config = AgentspecConfig::discover(&cwd)?;
    config.apply_overrides(args);

    // Parse embedded schemas once
    let schemas = load_schemas();

    // Phase 2: Load specs
    let specs = load_canonical_specs(&config)?;
    eprintln!("loaded {} specs", specs.len());

    // Phase 3: Resolve fragments
    let fragments_dir = config.resolve(&config.spec.fragments_dir);
    let fragment_map = load_fragments(&fragments_dir)?;
    let (env, fragment_warnings) = build_environment(&fragment_map)?;
    let specs = resolve_fragments(specs, &env)?;
    let registered_count = fragment_map.len() - fragment_warnings.len();
    for w in &fragment_warnings {
        eprintln!("warning: {}", w);
    }
    eprintln!(
        "resolved fragments ({} fragment templates loaded)",
        registered_count
    );

    // Validate frontmatter against canonical JSON schema
    let schema_errors = validate_schema(&specs, &schemas.canonical)?;
    if !schema_errors.is_empty() {
        for err in &schema_errors {
            eprintln!("error: {}", err);
        }
        anyhow::bail!("{} schema validation error(s)", schema_errors.len());
    }
    eprintln!("schema validation passed");

    // Normalize: apply defaults, dedup/sort tools, resolve targets
    let specs = normalize_specs(specs)?;
    eprintln!("normalized {} specs", specs.len());

    // Resolve model profiles from config, applying machine overlay if set
    let profiles = config.resolve_profiles(args.mapping_profile.as_deref());
    if profiles.is_empty() {
        eprintln!("no model profiles configured");
    } else {
        eprintln!("loaded {} model profile(s)", profiles.len());
    }

    // Semantic validation
    let semantic_errors = validate_semantics(&specs, &profiles);
    if !semantic_errors.is_empty() {
        for err in &semantic_errors {
            eprintln!("error: {}", err);
        }
        anyhow::bail!("{} semantic validation error(s)", semantic_errors.len());
    }
    eprintln!("semantic validation passed");

    let mut total_warnings = fragment_warnings.len();

    match &cli.command {
        Command::Validate(_) => {
            if args.strict && total_warnings > 0 {
                anyhow::bail!("{} warning(s) treated as errors (--strict)", total_warnings);
            }
            if total_warnings > 0 {
                eprintln!("validation complete ({} warning(s))", total_warnings);
            } else {
                eprintln!("validation complete");
            }
        }
        Command::Compile(_) | Command::Check(_) => {
            let targets = if args.target.is_empty() {
                config.targets.clone()
            } else {
                args.target.clone()
            };

            let result = compile_specs(&specs, &profiles, &targets);

            for w in &result.warnings {
                eprintln!("warning: {}", w);
            }

            total_warnings += result.warnings.len();
            if args.strict && total_warnings > 0 {
                anyhow::bail!("{} warning(s) treated as errors (--strict)", total_warnings);
            }

            eprintln!(
                "compiled {} files for {} provider(s) (hash: {})",
                result.files.len(),
                targets.len(),
                &result.source_hash[..12]
            );

            let output_dir = config.resolve(&config.output.dir);

            match &cli.command {
                Command::Compile(_) => {
                    write_generated_files(&result.files, &output_dir, &targets)?;
                    write_manifest(&result, &output_dir)?;
                    eprintln!(
                        "wrote {} files to {}",
                        result.files.len(),
                        output_dir.display()
                    );
                }
                Command::Check(_) => {
                    let check = check_generated_state(&result.files, &config.root_dir, &targets)?;
                    if check.is_clean() {
                        eprintln!("check passed: generated files are up to date");
                    } else {
                        for path in &check.missing {
                            eprintln!("missing: {}", path);
                        }
                        for path in &check.outdated {
                            eprintln!("outdated: {}", path);
                        }
                        for path in &check.unexpected {
                            eprintln!("unexpected: {}", path);
                        }
                        anyhow::bail!("check failed: {} problem(s) found", check.problem_count());
                    }
                }
                _ => unreachable!(),
            }
        }
    }

    Ok(())
}
