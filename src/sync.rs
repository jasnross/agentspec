pub(crate) mod manifest;

use std::path::{Path, PathBuf};

use agentspec::compile::{CompileResult, EmittedHookEntry, GeneratedFile};
use agentspec::plan::{FileKind, ManifestTrackedWrite, SyncPlan, expand_tilde};
use agentspec::provider::Provider;
use anyhow::{Context, Result, bail};

use crate::cli::SyncArgs;
use crate::config::{AgentspecConfig, SyncFlags, SyncMode, SyncTargetConfig};

/// Builds a `SyncPlan` that writes compiled files directly to tool config destinations.
///
/// One `ManifestTrackedWrite` is produced per (provider, kind) pair. Each adapter may
/// optionally provide a post-write hook for specific kinds.
pub fn sync_plan(
    result: &CompileResult,
    targets: &[(Provider, SyncTargetConfig)],
    home: &Path,
    cwd: &Path,
) -> Result<SyncPlan> {
    let mut writes = Vec::new();
    let mut post_write_hooks = Vec::new();

    for (provider, target) in targets {
        let adapter = provider.adapter();
        let emit_mode = target.mode.to_hook_emit_mode();
        let owned_entries: &[EmittedHookEntry] =
            result.hooks.get(provider).map_or(&[], Vec::as_slice);
        // `config_dir` does not depend on `kind`, so compute it once per
        // (provider, target) and reuse across the inner loop.
        let config_dir = adapter.config_dir(
            target.mode.to_destination_mode(),
            target.dir.as_deref(),
            home,
            cwd,
        );

        for &kind in adapter.file_kinds() {
            let dest = resolve_dest_dir(*provider, kind, target, home, cwd)?;
            let files = files_for_kind(result, *provider, kind);

            writes.push(ManifestTrackedWrite {
                provider: *provider,
                kind,
                destination: dest.clone(),
                files,
                overwrite: target.overwrite,
            });

            // Every adapter gets a chance to provide a post-write hook.
            // `target.overwrite` reflects `--force`; the Claude/Cursor merge
            // patchers consume it to decide whether to replace a user-authored
            // non-object `hooks` value with `{}`. OpenCode's instructions
            // patcher ignores it.
            let hook = adapter.post_write_hook(
                kind,
                &dest,
                &config_dir,
                emit_mode,
                owned_entries,
                target.overwrite,
            );
            if let Some(h) = hook {
                post_write_hooks.push(h);
            }
        }
    }

    Ok(SyncPlan {
        writes,
        post_write_hooks,
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

/// Resolves the destination directory for a provider/kind pair from a `SyncTargetConfig`.
///
/// - `User` → `ProviderAdapter::user_dest_dir`
/// - `Project` → `ProviderAdapter::project_dest_dir`
/// - `Path` → per-kind explicit field from `config`; error if the kind's field is `None`
pub(crate) fn resolve_dest_dir(
    provider: Provider,
    kind: FileKind,
    config: &SyncTargetConfig,
    home: &Path,
    cwd: &Path,
) -> Result<PathBuf> {
    let adapter = provider.adapter();
    match config.mode {
        SyncMode::User => Ok(adapter.user_dest_dir(home, kind)),
        SyncMode::Project => Ok(adapter.project_dest_dir(cwd, kind)),
        SyncMode::Path => {
            let base = config.dir.as_deref().with_context(|| {
                format!("sync mode is 'path' but no `dir` configured for provider '{provider}'")
            })?;
            Ok(expand_tilde(base, home).join(kind.dir_name()))
        }
    }
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
    use std::path::Path;

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

    #[test]
    fn test_sync_plan_produces_correct_writes_and_patches() {
        let result = make_result(vec![
            GeneratedFile::text(Provider::Claude, "agents/agent.md", "a".to_string()),
            GeneratedFile::text(Provider::Claude, "skills/s/SKILL.md", "s".to_string()),
            GeneratedFile::text(Provider::Claude, "rules/rule.md", "r".to_string()),
            GeneratedFile::text(Provider::OpenCode, "agents/agent.md", "a".to_string()),
            GeneratedFile::text(Provider::OpenCode, "commands/cmd.md", "c".to_string()),
            GeneratedFile::text(Provider::OpenCode, "rules/rule/AGENTS.md", "r".to_string()),
            GeneratedFile::text(Provider::OpenCode, "skills/s/SKILL.md", "s".to_string()),
        ]);

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
        let plan = sync_plan(&result, &targets, home, home).expect("sync_plan should succeed");

        // Claude: 4 kinds (agents, rules, skills, hooks); OpenCode: 4 kinds (agents, commands, rules, skills).
        assert_eq!(plan.writes.len(), 8);

        // Exactly one post-write hook: OpenCode instructions patching.
        assert_eq!(plan.post_write_hooks.len(), 1);

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

    fn home() -> std::path::PathBuf {
        std::path::PathBuf::from("/home/user")
    }

    fn cwd() -> std::path::PathBuf {
        std::path::PathBuf::from("/work/project")
    }

    #[test]
    fn test_resolve_dest_user_claude_agents() {
        let config = SyncTargetConfig::default(); // mode = User
        let result = resolve_dest_dir(Provider::Claude, FileKind::Agents, &config, &home(), &cwd())
            .expect("expected value");
        assert_eq!(
            result,
            std::path::PathBuf::from("/home/user/.claude/agents")
        );
    }

    #[test]
    fn test_resolve_dest_project_cursor_skills() {
        let config = SyncTargetConfig {
            mode: SyncMode::Project,
            ..Default::default()
        };
        let result = resolve_dest_dir(Provider::Cursor, FileKind::Skills, &config, &home(), &cwd())
            .expect("expected value");
        assert_eq!(
            result,
            std::path::PathBuf::from("/work/project/.cursor/skills")
        );
    }

    #[test]
    fn test_resolve_dest_path_explicit_dir() {
        let config = SyncTargetConfig {
            mode: SyncMode::Path,
            dir: Some("~/foo".to_string()),
            ..Default::default()
        };
        let result = resolve_dest_dir(Provider::Cursor, FileKind::Skills, &config, &home(), &cwd())
            .expect("expected value");
        assert_eq!(result, std::path::PathBuf::from("/home/user/foo/skills"));
    }

    #[test]
    fn test_resolve_dest_path_missing_dir_errors() {
        let config = SyncTargetConfig {
            mode: SyncMode::Path,
            ..Default::default()
        };
        let result = resolve_dest_dir(Provider::Claude, FileKind::Agents, &config, &home(), &cwd());
        assert!(result.is_err());
    }
}
