use std::path::Path;

use agentspec::adapters::RemoveCtx;
use agentspec::plan::{ConfigPatch, FileKind, RemovePlan, RemoveWrite};
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
/// One `RemoveWrite` is produced per `(provider, kind)` dest dir; one
/// `ConfigPatch` per provider for any post-write tidy (Claude/Cursor settings
/// strip, `OpenCode` instructions filter). The manifest at
/// `destination/.agentspec-manifest.json` is the source of truth at execution
/// time; no file content is carried because every tracked file is deleted.
///
/// Unlike `sync_plan`, `remove_plan` does NOT call `compile()` — it consults
/// the manifest at execution time and constructs `RemoveWrite` entries from
/// the recorded paths. For the post-write patches it calls
/// `Adapter::removal_patches(&ctx)`, which identifies owned entries via
/// on-disk `_agentspec_id` sentinels (no spec input needed).
pub fn remove_plan(
    targets: &[(Provider, SyncTargetConfig)],
    home: &Path,
    cwd: &Path,
) -> RemovePlan {
    debug_assert!(
        !targets.is_empty(),
        "remove_plan must not be called with empty targets; main.rs should print 'nothing to remove' instead"
    );
    let mut writes = Vec::new();
    let mut post_write_patches: Vec<Box<dyn ConfigPatch>> = Vec::new();

    for (provider, target) in targets {
        let adapter = provider.adapter();
        let target_dir_buf = target
            .dir
            .as_deref()
            .map(|d| agentspec::plan::expand_tilde(d, home));

        let ctx = RemoveCtx {
            mode: target.mode.to_destination_mode(),
            home,
            cwd,
            target_dir: target_dir_buf.as_deref(),
        };

        let removal = adapter.removal_patches(&ctx);
        let dest_root = removal.dest_root;
        post_write_patches.extend(removal.patches);

        // Each (provider, kind) still gets a `RemoveWrite` so `emit_remove`
        // can delete every manifest-tracked file at the per-kind dest dir.
        // The destination is the adapter's `dest_root` joined with the kind
        // dir name. Iterate every `FileKind` so providers that previously
        // emitted a kind they no longer support still get their stale
        // manifest cleaned up.
        for &kind in FileKind::all() {
            writes.push(RemoveWrite {
                provider: *provider,
                kind,
                destination: dest_root.join(kind.dir_name()),
            });
        }
    }

    RemovePlan {
        writes,
        post_write_patches,
    }
}
