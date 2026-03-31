pub(crate) mod manifest;
pub mod provider;
pub(crate) mod strategy;

use std::path::{Path, PathBuf};

use agentspec::compile::{CompileResult, GeneratedFile};
use agentspec::plan::{
    ConfigPatch, FileKind, FileWrite, NamePrefixMode, WriteMode, WritePlan, expand_tilde,
    file_kinds,
};
use agentspec::provider::Provider;
use anyhow::{Result, bail};

use crate::cli::SyncArgs;
use crate::config::{AgentspecConfig, SyncMode, SyncOverrides, SyncTargetConfig};
use provider::resolve_dest_dir;

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
        strategy: args.strategy,
        dest: args.dest.clone(),
        force: args.force,
    };

    let has_explicit_provider_selection = !args.common.provider.is_empty();
    let providers = if has_explicit_provider_selection {
        args.common.provider.clone()
    } else {
        config.configured_sync_providers()
    };

    if providers.is_empty() {
        bail!(
            "no sync providers are configured; add [sync.<provider>] in agentspec.toml, or run CLI-only sync with an explicit target (for example: --target claude --mode user|project or --target claude --dest <path>)"
        );
    }

    let mut resolved_targets: Vec<(Provider, SyncTargetConfig)> = Vec::new();
    for provider in providers {
        let intent =
            config.resolve_sync_intent(provider, &sync_overrides, has_explicit_provider_selection);

        if !intent.has_explicit_config && !intent.cli_only_allowed {
            bail!(
                "sync config for {provider} is not configured; add [sync.{provider}] in agentspec.toml, or pass explicit CLI-only sync arguments with --target {provider} and --mode user|project, or --target {provider} --dest <path>"
            );
        }

        intent.target.validate_for_sync(provider)?;
        resolved_targets.push((provider, intent.target));
    }

    Ok(resolved_targets)
}

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
        eprintln!(
            "syncing {provider} (mode={:?}, strategy={:?})",
            target.mode, target.strategy
        );

        for kind in file_kinds(*provider) {
            let mut dest = resolve_dest_dir(*provider, kind, target, home, cwd)?;
            let mut file_prefix: Option<String> = None;

            if let Some(prefix) = target.prefix.as_deref() {
                if *provider == Provider::OpenCode && kind == FileKind::Commands {
                    dest = dest.join(prefix);
                } else if kind != FileKind::Rules {
                    file_prefix = Some(format!("{prefix}-"));
                }
            }

            let files = files_for_kind(result, *provider, kind);
            let name_prefix = resolve_name_prefix(target, *provider, kind);

            writes.push(FileWrite {
                provider: *provider,
                destination: dest.clone(),
                files,
                mode: WriteMode::ManifestTracked,
                allow_overwrite: target.allow_overwrite,
                file_prefix,
                name_prefix,
                strip_name: target.strip_name,
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

/// Resolves the `name_prefix` for a `FileWrite` from target config, provider, and kind.
///
/// Only Claude agents and skills receive frontmatter name prefixes; all other
/// combinations return `None`.
fn resolve_name_prefix(
    target: &SyncTargetConfig,
    provider: Provider,
    kind: FileKind,
) -> Option<(String, NamePrefixMode)> {
    if provider != Provider::Claude {
        return None;
    }
    target.prefix.as_deref().and_then(|prefix| match kind {
        FileKind::Agents => Some((prefix.to_string(), NamePrefixMode::Agents)),
        FileKind::Skills => Some((prefix.to_string(), NamePrefixMode::Skills)),
        FileKind::Commands | FileKind::Rules => None,
    })
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
