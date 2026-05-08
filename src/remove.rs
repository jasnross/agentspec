use std::path::Path;

use agentspec::plan::{RemovePlan, RemoveWrite};
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
///
/// Also enforces the `--dest` requires `--provider` invariant: passing
/// `--dest` without an explicit `--provider` is a usage error. This matches
/// `resolve_sync_targets`'s behavior so the two code paths reject the same
/// CLI shapes.
pub fn resolve_remove_targets(
    config: &AgentspecConfig,
    args: &RemoveArgs,
) -> Result<Vec<(Provider, SyncTargetConfig)>> {
    let sync_flags = SyncFlags::for_remove(args.dest.clone(), args.mode);

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

/// Builds a `RemovePlan` that reverses a prior sync.
///
/// One `RemoveWrite` is produced per `(provider, kind)` dest dir. The manifest at
/// `destination/.agentspec-manifest.json` is the source of truth at execution time;
/// no file content is carried because every tracked file is deleted.
/// `post_write_hooks` is populated by Claude/Cursor settings tidy and
/// `OpenCode` instructions tidy.
pub fn remove_plan(
    targets: &[(Provider, SyncTargetConfig)],
    home: &Path,
    cwd: &Path,
) -> Result<RemovePlan> {
    debug_assert!(
        !targets.is_empty(),
        "remove_plan must not be called with empty targets; main.rs should print 'nothing to remove' instead"
    );
    let mut writes = Vec::new();
    let mut post_write_hooks = Vec::new();

    for (provider, target) in targets {
        let adapter = provider.adapter();
        let emit_mode = target.mode.to_hook_emit_mode();
        // Hoist `config_dir` out of the inner loop — mirrors `sync_plan`'s shape.
        let config_dir = adapter.config_dir(
            target.mode.to_destination_mode(),
            target.dir.as_deref(),
            home,
            cwd,
        );

        // Each (provider, kind) gets a Remove batch; the adapter's
        // `remove_post_write_hook` is offered the chance to claim this kind.
        // Each factory returns `None` for kinds it doesn't care about
        // (Claude/Cursor key off `Hooks`; `OpenCode` keys off `Rules`).
        for &kind in adapter.file_kinds() {
            let destination = sync::resolve_dest_dir(*provider, kind, target, home, cwd)?;

            let hook = adapter.remove_post_write_hook(kind, &destination, &config_dir, emit_mode);
            if let Some(h) = hook {
                post_write_hooks.push(h);
            }

            writes.push(RemoveWrite {
                provider: *provider,
                kind,
                destination,
            });
        }
    }

    Ok(RemovePlan {
        writes,
        post_write_hooks,
    })
}
