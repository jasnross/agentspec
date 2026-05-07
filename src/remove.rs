use std::path::Path;

use agentspec::plan::WritePlan;
use agentspec::provider::Provider;
use anyhow::{Result, bail};

use crate::cli::RemoveArgs;
use crate::config::{AgentspecConfig, SyncFlags, SyncTargetConfig};

/// Validates and resolves remove targets from config and CLI args.
///
/// Mirrors `sync::resolve_sync_targets` but diverges on the empty-providers
/// case: where sync errors out, remove returns `Ok(vec![])` so the caller can
/// print a "nothing to remove" notice and exit cleanly. Running
/// `agentspec remove` on a fresh checkout with no `[sync.*]` configured is a
/// legitimate "nothing to clean up" state, not a usage error.
pub fn resolve_remove_targets(
    config: &AgentspecConfig,
    args: &RemoveArgs,
) -> Result<Vec<(Provider, SyncTargetConfig)>> {
    let sync_flags = SyncFlags {
        force: false,
        dest: args.dest.clone(),
        mode: args.mode,
        prefix: None,
        content_prefix: None,
    };

    let has_provider_arg = !args.common.provider.is_empty();

    if args.dest.is_some() && !has_provider_arg {
        bail!("--dest requires an explicit --provider; use --provider <provider> --dest <path>");
    }

    let providers = if has_provider_arg {
        args.common.provider.clone()
    } else {
        config.configured_sync_providers()
    };

    if providers.is_empty() {
        return Ok(Vec::new());
    }

    let mut resolved: Vec<(Provider, SyncTargetConfig)> = Vec::new();
    for provider in providers {
        let target = config.validated_sync_target(provider, &sync_flags, has_provider_arg)?;
        resolved.push((provider, target));
    }

    Ok(resolved)
}

/// Builds a `WritePlan` that reverses a prior sync.
///
/// Phase 1 stub: returns an empty plan. Phase 2 populates `writes` with one
/// `FileWrite::Remove` per `(provider, kind)` dest dir; Phase 3+4 populate
/// `post_write_hooks` with the per-provider CST/JSON tidy patches.
#[allow(dead_code)] // wired in by Phase 2's main.rs dispatch arm
#[allow(clippy::unnecessary_wraps)] // Phases 2+ introduce fallible work; keep the signature stable.
pub fn remove_plan(
    _targets: &[(Provider, SyncTargetConfig)],
    _home: &Path,
    _cwd: &Path,
) -> Result<WritePlan> {
    Ok(WritePlan {
        writes: Vec::new(),
        post_write_hooks: Vec::new(),
    })
}
