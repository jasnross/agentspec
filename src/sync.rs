pub(crate) mod manifest;

use std::path::{Path, PathBuf};

use agentspec::compile::{CompileResult, GeneratedFile};
use agentspec::plan::{
    ConfigPatch, FileKind, FileWrite, WriteMode, WritePlan, expand_tilde, file_kinds,
    project_dest_dir, user_dest_dir,
};
use agentspec::provider::Provider;
use anyhow::{Context, Result, bail};

use crate::cli::SyncArgs;
use crate::config::{AgentspecConfig, SyncMode, SyncOverrides, SyncTargetConfig};

/// Builds a `WritePlan` that writes compiled files directly to tool config destinations.
///
/// One `FileWrite` (with `WriteMode::ManifestTracked`) is produced per (provider, kind) pair.
/// `OpenCode` rules additionally generate a `ConfigPatch::OpenCodeInstructions` entry.
pub fn sync_plan(
    result: &CompileResult,
    targets: &[(Provider, SyncTargetConfig)],
    home: &Path,
    cwd: &Path,
) -> Result<WritePlan> {
    let mut writes = Vec::new();
    let mut patches = Vec::new();

    for (provider, target) in targets {
        for kind in file_kinds(*provider) {
            let dest = resolve_dest_dir(*provider, kind, target, home, cwd)?;
            let files = files_for_kind(result, *provider, kind);

            writes.push(FileWrite {
                provider: *provider,
                destination: dest.clone(),
                files,
                mode: WriteMode::ManifestTracked,
                allow_overwrite: target.allow_overwrite,
            });

            if *provider == Provider::OpenCode && kind == FileKind::Rules {
                patches.push(ConfigPatch::OpenCodeInstructions {
                    rules_dest_dir: dest,
                    config_dir: opencode_config_dir(target, home, cwd),
                });
            }
        }
    }

    Ok(WritePlan { writes, patches })
}

/// Validates and resolves sync targets from config and CLI args.
///
/// Returns the resolved `(provider, target)` pairs ready for plan construction.
/// Errors early if no providers are configured or if any provider's config is invalid.
pub fn resolve_sync_targets(
    config: &AgentspecConfig,
    args: &SyncArgs,
) -> Result<Vec<(Provider, SyncTargetConfig)>> {
    let sync_overrides = SyncOverrides {
        mode: args.mode,
        dest: args.dest.clone(),
        force: args.force,
    };

    let has_explicit_provider_selection = !args.common.provider.is_empty();

    if args.dest.is_some() && !has_explicit_provider_selection {
        bail!("--dest requires an explicit --provider; use --provider <provider> --dest <path>");
    }

    let providers = if has_explicit_provider_selection {
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
        let target = config.validated_sync_target(
            provider,
            &sync_overrides,
            has_explicit_provider_selection,
        )?;
        target.validate_for_sync(provider)?;
        resolved_targets.push((provider, target));
    }

    Ok(resolved_targets)
}

/// Resolves the destination directory for a provider/kind pair from a `SyncTargetConfig`.
///
/// - `User` → `user_dest_dir`
/// - `Project` → `project_dest_dir`
/// - `Path` → per-kind explicit field from `config`; error if the kind's field is `None`
fn resolve_dest_dir(
    provider: Provider,
    kind: FileKind,
    config: &SyncTargetConfig,
    home: &Path,
    cwd: &Path,
) -> Result<PathBuf> {
    match config.mode {
        SyncMode::User => Ok(user_dest_dir(provider, kind, home)),
        SyncMode::Project => Ok(project_dest_dir(provider, kind, cwd)),
        SyncMode::Path => {
            let raw = match kind {
                FileKind::Agents => config.agents.as_deref(),
                FileKind::Commands => config.commands.as_deref(),
                FileKind::Rules => config.rules.as_deref(),
                FileKind::Skills => config.skills.as_deref(),
            };
            let path_str = raw.with_context(|| {
                format!(
                    "sync mode is 'path' but no explicit path configured for \
                     provider '{provider}' kind '{}'",
                    kind.dir_name()
                )
            })?;
            Ok(expand_tilde(path_str, home))
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

/// Resolves the `OpenCode` config directory for a sync target.
///
/// In `Path` mode, derives the config dir from the rules path's parent (convention:
/// `opencode.json` lives one level above `rules/`). Falls back to the user-level
/// config dir if the rules path is absent or has no parent.
pub(crate) fn opencode_config_dir(target: &SyncTargetConfig, home: &Path, cwd: &Path) -> PathBuf {
    match target.mode {
        SyncMode::User => home.join(".config").join("opencode"),
        SyncMode::Project => cwd.join(".opencode"),
        SyncMode::Path => target
            .rules
            .as_deref()
            .and_then(|r| expand_tilde(r, home).parent().map(Path::to_path_buf))
            .unwrap_or_else(|| home.join(".config").join("opencode")),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use agentspec::compile::{CompileResult, GeneratedFile};
    use agentspec::plan::{ConfigPatch, FileKind, WriteMode};
    use agentspec::provider::Provider;

    use super::{files_for_kind, resolve_dest_dir, sync_plan};
    use crate::config::{SyncMode, SyncTargetConfig};

    fn make_result(files: Vec<GeneratedFile>) -> CompileResult {
        CompileResult { files }
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
            agents: Some("/out/claude/agents".to_string()),
            skills: Some("/out/claude/skills".to_string()),
            rules: Some("/out/claude/rules".to_string()),
            ..SyncTargetConfig::default()
        };

        let opencode_target = SyncTargetConfig {
            mode: SyncMode::Path,
            agents: Some("/out/opencode/agents".to_string()),
            commands: Some("/out/opencode/commands".to_string()),
            rules: Some("/out/opencode/rules".to_string()),
            skills: Some("/out/opencode/skills".to_string()),
            ..SyncTargetConfig::default()
        };

        let targets = vec![
            (Provider::Claude, claude_target),
            (Provider::OpenCode, opencode_target),
        ];

        let home = Path::new("/tmp");
        let plan = sync_plan(&result, &targets, home, home).expect("sync_plan should succeed");

        // Claude: 3 kinds (agents, rules, skills); OpenCode: 4 kinds (agents, commands, rules, skills)
        assert_eq!(plan.writes.len(), 7);

        // Every write uses ManifestTracked mode.
        for w in &plan.writes {
            assert!(
                matches!(w.mode, WriteMode::ManifestTracked),
                "expected ManifestTracked for {} {:?}",
                w.provider,
                w.destination
            );
        }

        // Exactly one ConfigPatch: OpenCode instructions for the rules dest.
        assert_eq!(plan.patches.len(), 1);
        assert!(matches!(
            plan.patches[0],
            ConfigPatch::OpenCodeInstructions { .. }
        ));

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
    fn test_resolve_dest_path_explicit_skills() {
        let config = SyncTargetConfig {
            mode: SyncMode::Path,
            skills: Some("~/foo/skills".to_string()),
            ..Default::default()
        };
        let result = resolve_dest_dir(Provider::Cursor, FileKind::Skills, &config, &home(), &cwd())
            .expect("expected value");
        assert_eq!(result, std::path::PathBuf::from("/home/user/foo/skills"));
    }

    #[test]
    fn test_resolve_dest_path_missing_agents_errors() {
        let config = SyncTargetConfig {
            mode: SyncMode::Path,
            ..Default::default()
        };
        let result = resolve_dest_dir(Provider::Claude, FileKind::Agents, &config, &home(), &cwd());
        assert!(result.is_err());
    }
}
