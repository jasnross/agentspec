use std::path::Path;

use agentspec::plan::{FileWrite, WriteMode, WritePlan, file_kinds};
use agentspec::provider::Provider;
use anyhow::{Result, bail};

use crate::cli::RemoveArgs;
use crate::config::{AgentspecConfig, SyncFlags, SyncTargetConfig};
use crate::sync;

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
/// One `FileWrite { mode: WriteMode::Remove, .. }` is produced per
/// `(provider, kind)` dest dir. `files` is empty and `overwrite` is `false`
/// for every entry — the manifest at `destination/.agentspec-manifest.json` is
/// the source of truth at execution time. `post_write_hooks` is populated by
/// later phases (Claude/Cursor settings tidy in Phase 3, `OpenCode` instructions
/// tidy in Phase 4).
pub fn remove_plan(
    targets: &[(Provider, SyncTargetConfig)],
    home: &Path,
    cwd: &Path,
) -> Result<WritePlan> {
    let mut writes = Vec::new();

    for (provider, target) in targets {
        for kind in file_kinds(*provider) {
            let destination = sync::resolve_dest_dir(*provider, kind, target, home, cwd)?;
            writes.push(FileWrite {
                provider: *provider,
                kind: Some(kind),
                destination,
                files: Vec::new(),
                mode: WriteMode::Remove,
                overwrite: false,
            });
        }
    }

    Ok(WritePlan {
        writes,
        post_write_hooks: Vec::new(),
    })
}
