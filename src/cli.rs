use agentspec::provider::Provider;
use clap::{Parser, Subcommand};
use clap_complete::Shell;

use crate::config::{SyncMode, SyncStrategy};

#[derive(Debug, Parser)]
#[command(
    name = "agentspec",
    about = "Compile provider-neutral specs into AI coding agent configurations"
)]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Validate spec files against schemas and run semantic checks
    Validate(CommonArgs),

    /// Compile spec files into provider-specific configurations
    Compile(CommonArgs),

    /// Compile and distribute generated files to each tool's config directory
    Sync(SyncArgs),

    /// Print shell completion script to stdout
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
}

#[derive(Debug, Default, Parser)]
pub struct CommonArgs {
    /// Providers to target (repeatable: `--provider claude --provider cursor`, or comma-separated: `--provider claude,cursor`)
    #[arg(long, value_delimiter = ',')]
    pub provider: Vec<Provider>,
}

/// Arguments for the `sync` subcommand.
#[derive(Debug, Parser)]
pub struct SyncArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Show what would be synced without making changes
    #[arg(long)]
    pub dry_run: bool,

    /// Allow overwriting user-owned files at sync destinations (disables collision errors).
    #[arg(long)]
    pub force: bool,

    /// Override sync strategy for all providers
    #[arg(long, value_enum)]
    pub strategy: Option<SyncStrategy>,

    /// Override destination root for all providers (implies --mode=path)
    #[arg(long)]
    pub dest: Option<String>,

    /// Override sync mode for all providers
    #[arg(long, value_enum)]
    pub mode: Option<SyncMode>,
}

impl Command {
    pub fn args(&self) -> Option<&CommonArgs> {
        match self {
            Command::Validate(args) | Command::Compile(args) => Some(args),
            Command::Sync(args) => Some(&args.common),
            Command::Completions { .. } => None,
        }
    }
}
