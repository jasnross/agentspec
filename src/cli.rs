use clap::{Parser, Subcommand};

use crate::types::Provider;

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
}

#[derive(Parser, Debug)]
pub struct CommonArgs {
    /// Comma-separated list of target providers (e.g., claude,cursor,codex,opencode)
    #[arg(long, value_delimiter = ',')]
    pub target: Vec<Provider>,

    /// Profile overlay to apply (e.g., "home", "work"); also reads `AGENTSPEC_PROFILE` env var.
    /// Set AGENTSPEC_PROFILE in your shell profile to make a selection permanent.
    #[arg(long, env = "AGENTSPEC_PROFILE")]
    pub profile: Option<String>,

    /// Treat warnings as errors (exit code 1)
    #[arg(long)]
    pub strict: bool,
}

impl Command {
    pub fn args(&self) -> &CommonArgs {
        match self {
            Command::Validate(args) | Command::Compile(args) | Command::Check(args) => args,
        }
    }
}
