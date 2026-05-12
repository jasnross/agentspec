use agentspec::provider::Provider;
use clap::{Parser, Subcommand};
use clap_complete::Shell;

use crate::config::SyncMode;

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

    /// Remove agentspec-managed files and config entries from each tool's config directory
    Remove(RemoveArgs),

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

    /// Show detailed diagnostics. For `compile` and `sync`, emits the full
    /// [spec].ignore listing; for `sync`, also shows unchanged sync
    /// destinations. `validate` always shows the full listing regardless of
    /// this flag.
    #[arg(long)]
    pub verbose: bool,
}

/// Arguments for the `sync` subcommand.
#[derive(Debug, Parser)]
pub struct SyncArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Shows what would be synced without making changes
    #[arg(long)]
    pub dry_run: bool,

    /// Allow overwriting user-owned files at sync destinations (disables collision errors)
    #[arg(long)]
    pub force: bool,

    /// Output destination (implies --mode=plugin)
    #[arg(long)]
    pub dest: Option<String>,

    /// Specify sync mode
    #[arg(long, value_enum)]
    pub mode: Option<SyncMode>,

    /// Add a prefix to synced file names (can help avoid naming collisions with other commands)
    #[arg(long)]
    pub prefix: Option<String>,

    /// Override the content-reference prefix (e.g., "tw:" for plugin namespaces)
    #[arg(long)]
    pub content_prefix: Option<String>,
}

/// Arguments for the `remove` subcommand.
#[derive(Debug, Parser)]
pub struct RemoveArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Show what would be removed without making changes
    #[arg(long)]
    pub dry_run: bool,

    /// Destination to remove from (implies --mode=plugin)
    #[arg(long)]
    pub dest: Option<String>,

    /// Specify which sync mode to reverse
    #[arg(long, value_enum)]
    pub mode: Option<SyncMode>,
}
