pub(crate) mod manifest;

use std::path::{Path, PathBuf};

use agentspec::compile::{CompileResult, GeneratedFile};
use agentspec::plan::{FileKind, ManifestTrackedWrite, SyncPlan};
use agentspec::provider::Provider;
use anyhow::{Context, Result, bail};

use crate::cli::SyncArgs;
use crate::config::{AgentspecConfig, SyncFlags, SyncTargetConfig};

/// Builds a `SyncPlan` that writes compiled files directly to tool config destinations.
///
/// One `ManifestTrackedWrite` is produced per (provider, kind) pair. Post-write
/// patches were constructed by `Adapter::compile` and are drained per-provider
/// from `result.patches`.
///
/// Takes `&mut CompileResult` because the per-provider patch vectors are
/// consumed (moved) from the result. This is a one-shot transfer — patches
/// cannot be re-read after `sync_plan` runs, which matches the existing
/// pipeline shape (each plan is built from `CompileResult` exactly once
/// before emit).
pub fn sync_plan(
    result: &mut CompileResult,
    targets: &[(Provider, SyncTargetConfig)],
) -> Result<SyncPlan> {
    // Invariant: `compile_specs` populates `dest_roots` for every provider it
    // dispatched to. A missing entry means the binary built `targets` from a
    // different provider list than it passed to `compile::run`, which is a
    // wiring bug — surface it at the call site rather than letting it fall
    // through to a user-facing error.
    debug_assert!(
        targets
            .iter()
            .all(|(p, _)| result.dest_roots.contains_key(p)),
        "sync_plan: every target provider must have a dest_root recorded by compile_specs"
    );

    let mut writes = Vec::new();
    let mut post_write_patches = Vec::new();

    for (provider, target) in targets {
        // `dest_root` was computed by the adapter's `config_dir` during
        // `compile_specs` and is the parent directory of every per-kind dest
        // dir. It already incorporates any custom `dir` override.
        let dest_root = result
            .dest_root_for(*provider)
            .with_context(|| format!("compile_specs did not record a dest_root for {provider}"))?
            .to_path_buf();

        // Iterate every `FileKind` (not just the ones this provider emits)
        // so stale-cleanup runs even for kinds a provider used to emit but
        // doesn't anymore. Empty batches with no prior manifest are a no-op
        // inside `write_manifest_tracked`, so the extra iterations are free
        // for providers that never used a given kind (e.g., OpenCode +
        // Hooks).
        for &kind in FileKind::all() {
            // `None` here means this provider doesn't support `kind` (today:
            // only PluginManifest for `OpenCode`). Skip the `ManifestTrackedWrite`
            // entirely — there's nothing to write and nothing to track.
            let Some(dest) = resolve_dest_dir(*provider, kind, &dest_root) else {
                continue;
            };
            let files = files_for_kind(result, *provider, kind);

            writes.push(ManifestTrackedWrite {
                provider: *provider,
                kind,
                destination: dest,
                files,
                overwrite: target.overwrite,
            });
        }

        // Drain pre-built post-write patches for this provider. Adapters
        // construct these inside `compile`; the orchestrator merely shuttles
        // them through `CompileResult` to here.
        if let Some(patches) = result.patches.remove(provider) {
            post_write_patches.extend(patches);
        }
    }

    Ok(SyncPlan {
        writes,
        post_write_patches,
    })
}

/// Validates and resolves sync targets from config and CLI args.
///
/// Returns the resolved `(provider, target)` pairs ready for plan construction.
/// Errors early if no providers are configured or if any provider's config is invalid.
pub fn resolve_sync_targets(
    config: &AgentspecConfig,
    args: &SyncArgs,
) -> Result<Vec<(Provider, SyncTargetConfig)>> {
    let sync_flags = SyncFlags {
        force: args.force,
        dest: args.dest.clone(),
        mode: args.mode,
        prefix: args.prefix.clone(),
        content_prefix: args.content_prefix.clone(),
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
        bail!(
            "no sync providers are configured; add [sync.<provider>] in agentspec.toml, or run CLI-only sync with an explicit provider (for example: --provider claude --mode user|project)"
        );
    }

    let mut resolved_targets: Vec<(Provider, SyncTargetConfig)> = Vec::new();
    for provider in providers {
        let target = config.validated_sync_target(provider, &sync_flags, has_provider_arg)?;
        resolved_targets.push((provider, target));
    }

    Ok(resolved_targets)
}

/// Resolves the per-kind destination directory under a provider's sync root.
///
/// `dest_root` is the per-provider sync destination root the adapter computed
/// during `compile_specs`. The adapter's `config_dir` already handles all
/// mode-specific logic (User → `$HOME/<dotdir>`, Project → `<cwd or
/// dir>/<dotdir>`, Plugin → `<dir>`), so this function simply joins the
/// per-kind subdirectory name.
///
/// Returns `None` when the provider doesn't support `kind` — today, only
/// for [`FileKind::PluginManifest`] on providers whose
/// `Adapter::plugin_manifest_dir()` returns `None` (i.e., `OpenCode`). The
/// caller skips the write in that case.
fn resolve_dest_dir(provider: Provider, kind: FileKind, dest_root: &Path) -> Option<PathBuf> {
    provider
        .adapter()
        .dir_for_kind(kind)
        .map(|dir| dest_root.join(dir))
}

/// Extracts files from `result` that belong to the given provider and kind.
///
/// Partitions on the explicit `kind` field each adapter sets when constructing
/// `GeneratedFile` instances — no path-component derivation needed.
fn files_for_kind(
    result: &CompileResult,
    provider: Provider,
    kind: FileKind,
) -> Vec<GeneratedFile> {
    result
        .files_for(provider)
        .filter(|f| f.kind == kind)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use agentspec::compile::{CompileResult, GeneratedFile};
    use agentspec::plan::FileKind;
    use agentspec::provider::Provider;

    use super::{files_for_kind, resolve_dest_dir, sync_plan};
    use crate::config::{SyncMode, SyncTargetConfig};

    fn make_result(files: Vec<GeneratedFile>) -> CompileResult {
        CompileResult {
            files,
            ..CompileResult::default()
        }
    }

    #[test]
    fn test_files_for_kind_partitions_by_kind_and_provider() {
        let result = make_result(vec![
            GeneratedFile::text(
                Provider::Claude,
                FileKind::Agents,
                "agents/foo.md",
                "agent".to_string(),
            ),
            GeneratedFile::text(
                Provider::Claude,
                FileKind::Skills,
                "skills/bar/SKILL.md",
                "skill".to_string(),
            ),
            GeneratedFile::text(
                Provider::Claude,
                FileKind::Rules,
                "rules/baz.md",
                "rule".to_string(),
            ),
            GeneratedFile::text(
                Provider::Cursor,
                FileKind::Agents,
                "agents/foo.md",
                "cursor-agent".to_string(),
            ),
        ]);

        // Returns only Claude agents, not the Cursor one.
        let agents = files_for_kind(&result, Provider::Claude, FileKind::Agents);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].path.to_str(), Some("agents/foo.md"));
        assert_eq!(agents[0].provider, Provider::Claude);

        // Correct kind partitioning within a single provider.
        let skills = files_for_kind(&result, Provider::Claude, FileKind::Skills);
        assert_eq!(skills.len(), 1);

        let rules = files_for_kind(&result, Provider::Claude, FileKind::Rules);
        assert_eq!(rules.len(), 1);

        // Cursor sees its own agent.
        let cursor_agents = files_for_kind(&result, Provider::Cursor, FileKind::Agents);
        assert_eq!(cursor_agents.len(), 1);
        assert_eq!(cursor_agents[0].provider, Provider::Cursor);

        // No cross-kind contamination.
        let commands = files_for_kind(&result, Provider::Claude, FileKind::Commands);
        assert!(commands.is_empty());
    }

    /// Inserts a placeholder `dest_root` for each provider so `sync_plan` —
    /// which now reads `dest_root` from `CompileResult` rather than calling
    /// adapter methods directly — has the same input shape it would receive
    /// from a real `compile_specs` run.
    fn seed_dest_roots(result: &mut CompileResult, providers: &[(Provider, &str)]) {
        for (p, root) in providers {
            result.dest_roots.insert(*p, PathBuf::from(root));
        }
    }

    #[test]
    fn test_sync_plan_produces_correct_writes_and_patches() {
        let mut result = make_result(vec![
            GeneratedFile::text(
                Provider::Claude,
                FileKind::Agents,
                "agents/agent.md",
                "a".to_string(),
            ),
            GeneratedFile::text(
                Provider::Claude,
                FileKind::Skills,
                "skills/s/SKILL.md",
                "s".to_string(),
            ),
            GeneratedFile::text(
                Provider::Claude,
                FileKind::Rules,
                "rules/rule.md",
                "r".to_string(),
            ),
            GeneratedFile::text(
                Provider::OpenCode,
                FileKind::Agents,
                "agents/agent.md",
                "a".to_string(),
            ),
            GeneratedFile::text(
                Provider::OpenCode,
                FileKind::Commands,
                "commands/cmd.md",
                "c".to_string(),
            ),
            GeneratedFile::text(
                Provider::OpenCode,
                FileKind::Rules,
                "rules/rule/AGENTS.md",
                "r".to_string(),
            ),
            GeneratedFile::text(
                Provider::OpenCode,
                FileKind::Skills,
                "skills/s/SKILL.md",
                "s".to_string(),
            ),
        ]);
        seed_dest_roots(
            &mut result,
            &[
                (Provider::Claude, "/out/claude"),
                (Provider::OpenCode, "/out/opencode"),
            ],
        );

        let claude_target = SyncTargetConfig {
            mode: Some(SyncMode::Plugin),
            dir: Some("/out/claude".to_string()),
            ..SyncTargetConfig::default()
        };

        let opencode_target = SyncTargetConfig {
            mode: Some(SyncMode::Plugin),
            dir: Some("/out/opencode".to_string()),
            ..SyncTargetConfig::default()
        };

        let targets = vec![
            (Provider::Claude, claude_target),
            (Provider::OpenCode, opencode_target),
        ];

        let plan = sync_plan(&mut result, &targets).expect("sync_plan should succeed");

        // Every provider in the targets list gets one `ManifestTrackedWrite`
        // per `FileKind` variant the provider supports. Providers without
        // a plugin concept (today: `OpenCode`) skip `FileKind::PluginManifest`,
        // so the count is `FileKind::all().len() * 2 - 1` (Claude supports
        // all six kinds; OpenCode skips PluginManifest). Non-emitting kinds
        // have empty `files` and become no-ops at emit time.
        assert_eq!(plan.writes.len(), FileKind::all().len() * 2 - 1);

        // No post-write patches in this test: the unit-test `CompileResult` is
        // constructed by hand without calling `compile_specs`, so no
        // adapter-built patches are present. Real runs populate them via
        // `Adapter::compile`.
        assert!(plan.post_write_patches.is_empty());

        // Spot-check: the Claude agents write targets the correct destination
        // and carries the correct file.
        let claude_agents = plan
            .writes
            .iter()
            .find(|w| w.provider == Provider::Claude && w.destination.ends_with("claude/agents"))
            .expect("claude agents write should exist");
        assert_eq!(claude_agents.files.len(), 1);
        assert!(claude_agents.files[0].path.starts_with("agents/"));
    }

    #[test]
    fn test_resolve_dest_dir_joins_dest_root_with_kind_dir() {
        let result = resolve_dest_dir(
            Provider::Claude,
            FileKind::Agents,
            Path::new("/home/user/.claude"),
        )
        .expect("kind supported by provider");
        assert_eq!(result, PathBuf::from("/home/user/.claude/agents"));
    }

    #[test]
    fn test_resolve_dest_dir_works_for_any_dest_root() {
        let result = resolve_dest_dir(
            Provider::Cursor,
            FileKind::Skills,
            Path::new("/work/project/.cursor"),
        )
        .expect("kind supported by provider");
        assert_eq!(result, PathBuf::from("/work/project/.cursor/skills"));
    }

    #[test]
    fn test_resolve_dest_dir_plugin_manifest_dispatches_through_adapter() {
        let claude = resolve_dest_dir(
            Provider::Claude,
            FileKind::PluginManifest,
            Path::new("/out"),
        )
        .expect("claude supports plugin manifest");
        assert_eq!(claude, PathBuf::from("/out/.claude-plugin"));

        let cursor = resolve_dest_dir(
            Provider::Cursor,
            FileKind::PluginManifest,
            Path::new("/out"),
        )
        .expect("cursor supports plugin manifest");
        assert_eq!(cursor, PathBuf::from("/out/.cursor-plugin"));

        let opencode = resolve_dest_dir(
            Provider::OpenCode,
            FileKind::PluginManifest,
            Path::new("/out"),
        );
        assert!(
            opencode.is_none(),
            "OpenCode has no plugin concept; expected None, got {opencode:?}"
        );
    }
}
