pub(crate) mod manifest;

use std::path::{Path, PathBuf};

use agentspec::compile::{CompileResult, GeneratedFile};
use agentspec::plan::{FileKind, ManifestTrackedWrite, SyncPlan, expand_tilde};
use agentspec::provider::Provider;
use anyhow::{Context, Result, bail};

use crate::cli::SyncArgs;
use crate::config::{AgentspecConfig, SyncFlags, SyncMode, SyncTargetConfig};

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
    home: &Path,
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
        let adapter = provider.adapter();
        // `dest_root` was computed by the adapter during `compile_specs` and
        // is the parent directory of every per-kind dest dir. Use it as the
        // anchor when `mode == Path` so the explicit `--dest`/`dir` value
        // wins over adapter defaults; for `User`/`Project` modes it matches
        // what the adapter would compute again here.
        let dest_root = result
            .dest_root_for(*provider)
            .with_context(|| format!("compile_specs did not record a dest_root for {provider}"))?
            .to_path_buf();

        for &kind in adapter.file_kinds() {
            let dest = resolve_dest_dir(kind, target, &dest_root, home);
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
            "no sync providers are configured; add [sync.<provider>] in agentspec.toml, or run CLI-only sync with an explicit provider (for example: --provider claude --mode user|project or --provider claude --dest <path>)"
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
/// during `compile_specs` (e.g. `~/.claude` for User mode, `<cwd>/.claude` for
/// Project mode, `<dir>` for Path mode). For Path mode we honour the explicit
/// `dir` from `SyncTargetConfig` (tilde-expanded) so the `--dest` override
/// wins; for User/Project modes the adapter's `dest_root` is canonical.
fn resolve_dest_dir(
    kind: FileKind,
    config: &SyncTargetConfig,
    dest_root: &Path,
    home: &Path,
) -> PathBuf {
    let base = match config.mode {
        SyncMode::Path => config
            .dir
            .as_deref()
            .map_or_else(|| dest_root.to_path_buf(), |d| expand_tilde(d, home)),
        SyncMode::User | SyncMode::Project => dest_root.to_path_buf(),
    };
    base.join(kind.dir_name())
}

/// Extracts files from `result` that belong to the given provider and kind.
///
/// Relies on the invariant that every adapter produces paths whose first component
/// matches a `FileKind::dir_name()` (e.g. `"agents/foo.md"`, `"rules/bar.md"`).
fn files_for_kind(
    result: &CompileResult,
    provider: Provider,
    kind: FileKind,
) -> Vec<GeneratedFile> {
    #[cfg(debug_assertions)]
    for f in result.files_for(provider) {
        let first = f
            .path
            .components()
            .next()
            .and_then(|c| c.as_os_str().to_str());
        debug_assert!(
            FileKind::all().iter().any(|k| first == Some(k.dir_name())),
            "GeneratedFile path {} does not start with a known FileKind dir",
            f.path.display()
        );
    }

    result
        .files_for(provider)
        .filter(|f| f.path.starts_with(kind.dir_name()))
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
            GeneratedFile::text(Provider::Claude, "agents/foo.md", "agent".to_string()),
            GeneratedFile::text(Provider::Claude, "skills/bar/SKILL.md", "skill".to_string()),
            GeneratedFile::text(Provider::Claude, "rules/baz.md", "rule".to_string()),
            GeneratedFile::text(
                Provider::Cursor,
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
            GeneratedFile::text(Provider::Claude, "agents/agent.md", "a".to_string()),
            GeneratedFile::text(Provider::Claude, "skills/s/SKILL.md", "s".to_string()),
            GeneratedFile::text(Provider::Claude, "rules/rule.md", "r".to_string()),
            GeneratedFile::text(Provider::OpenCode, "agents/agent.md", "a".to_string()),
            GeneratedFile::text(Provider::OpenCode, "commands/cmd.md", "c".to_string()),
            GeneratedFile::text(Provider::OpenCode, "rules/rule/AGENTS.md", "r".to_string()),
            GeneratedFile::text(Provider::OpenCode, "skills/s/SKILL.md", "s".to_string()),
        ]);
        seed_dest_roots(
            &mut result,
            &[
                (Provider::Claude, "/out/claude"),
                (Provider::OpenCode, "/out/opencode"),
            ],
        );

        let claude_target = SyncTargetConfig {
            mode: SyncMode::Path,
            dir: Some("/out/claude".to_string()),
            ..SyncTargetConfig::default()
        };

        let opencode_target = SyncTargetConfig {
            mode: SyncMode::Path,
            dir: Some("/out/opencode".to_string()),
            ..SyncTargetConfig::default()
        };

        let targets = vec![
            (Provider::Claude, claude_target),
            (Provider::OpenCode, opencode_target),
        ];

        let home = Path::new("/tmp");
        let plan = sync_plan(&mut result, &targets, home).expect("sync_plan should succeed");

        // Claude: 4 kinds (agents, rules, skills, hooks); OpenCode: 4 kinds (agents, commands, rules, skills).
        assert_eq!(plan.writes.len(), 8);

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

    fn home() -> PathBuf {
        PathBuf::from("/home/user")
    }

    #[test]
    fn test_resolve_dest_user_appends_kind_dir_to_root() {
        // User mode: the per-kind dest is `<dest_root>/<kind>`. The adapter
        // already encoded the home-rooted `~/.claude` portion in `dest_root`
        // during `compile_specs`; `resolve_dest_dir` just appends the kind.
        let config = SyncTargetConfig::default(); // mode = User
        let result = resolve_dest_dir(
            FileKind::Agents,
            &config,
            Path::new("/home/user/.claude"),
            &home(),
        );
        assert_eq!(result, PathBuf::from("/home/user/.claude/agents"));
    }

    #[test]
    fn test_resolve_dest_project_appends_kind_dir_to_root() {
        let config = SyncTargetConfig {
            mode: SyncMode::Project,
            ..Default::default()
        };
        let result = resolve_dest_dir(
            FileKind::Skills,
            &config,
            Path::new("/work/project/.cursor"),
            &home(),
        );
        assert_eq!(result, PathBuf::from("/work/project/.cursor/skills"));
    }

    #[test]
    fn test_resolve_dest_path_explicit_dir_overrides_dest_root() {
        let config = SyncTargetConfig {
            mode: SyncMode::Path,
            dir: Some("~/foo".to_string()),
            ..Default::default()
        };
        let result = resolve_dest_dir(
            FileKind::Skills,
            &config,
            Path::new("/should-be-ignored"),
            &home(),
        );
        assert_eq!(result, PathBuf::from("/home/user/foo/skills"));
    }

    #[test]
    fn test_resolve_dest_path_no_explicit_dir_uses_dest_root() {
        // Path mode without an explicit `dir`: fall back to `dest_root` (which
        // the adapter already populated). The previous behavior errored here
        // because path resolution lived in the binary; under the new shape
        // adapters always supply a `dest_root`, so this path is no longer an
        // error.
        let config = SyncTargetConfig {
            mode: SyncMode::Path,
            ..Default::default()
        };
        let result = resolve_dest_dir(
            FileKind::Agents,
            &config,
            Path::new("/explicit/dest/root"),
            &home(),
        );
        assert_eq!(result, PathBuf::from("/explicit/dest/root/agents"));
    }
}
