use clap::{Parser, Subcommand};
use clap_complete::Shell;

use crate::types::{Provider, SyncMode, SyncStrategy};

#[derive(Parser, Debug)]
#[command(
    name = "agentspec",
    about = "Compile provider-neutral specs into AI coding agent configurations"
)]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Validate spec files against schemas and run semantic checks
    Validate(CommonArgs),

    /// Compile spec files into provider-specific configurations
    Compile(CommonArgs),

    /// Check that generated files match what compile would produce
    Check(CommonArgs),

    /// Compile and distribute generated files to each tool's config directory
    Sync(SyncArgs),

    /// Print shell completion script to stdout
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
}

#[derive(Parser, Debug, Default)]
pub struct CommonArgs {
    /// Comma-separated list of target providers (e.g., claude,cursor,codex,opencode)
    #[arg(long, value_delimiter = ',')]
    pub target: Vec<Provider>,

    /// Profile overlay to apply (e.g., "home", "work"); also reads `AGENTSPEC_PROFILE` env var.
    /// Set `AGENTSPEC_PROFILE` in your shell profile to make a selection permanent.
    #[arg(long, env = "AGENTSPEC_PROFILE")]
    pub profile: Option<String>,

    /// Treat warnings as errors (exit code 1)
    #[arg(long)]
    pub strict: bool,
}

/// Arguments for the `sync` subcommand.
#[derive(Parser, Debug)]
pub struct SyncArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Skip compilation; sync from existing generated output
    #[arg(long)]
    pub no_compile: bool,

    /// Show what would be synced without making changes
    #[arg(long)]
    pub dry_run: bool,

    /// Override sync strategy for all providers
    #[arg(long, value_enum)]
    pub strategy: Option<SyncStrategy>,

    /// Override destination root for all providers (implies --mode=path)
    #[arg(long)]
    pub dest: Option<String>,

    /// Override target mode for all providers
    #[arg(long, value_enum)]
    pub mode: Option<SyncMode>,
}

impl Command {
    pub fn args(&self) -> Option<&CommonArgs> {
        match self {
            Command::Validate(args) | Command::Compile(args) | Command::Check(args) => Some(args),
            Command::Sync(args) => Some(&args.common),
            Command::Completions { .. } => None,
        }
    }
}
