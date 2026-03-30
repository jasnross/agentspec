use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use walkdir::WalkDir;

use agentspec::provider::Provider;
use crate::config::SyncMode;
use crate::config::SyncTargetConfig;

/// The kinds of outputs the sync command distributes, mirroring the `generated/<provider>/`
/// subdirectories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncKind {
    Agents,
    Commands,
    Rules,
    Skills,
}

impl SyncKind {
    /// Directory name used under `generated/<provider>/` and under tool config dirs.
    pub fn dir_name(self) -> &'static str {
        match self {
            SyncKind::Agents => "agents",
            SyncKind::Commands => "commands",
            SyncKind::Rules => "rules",
            SyncKind::Skills => "skills",
        }
    }
}

/// Returns the sync kinds generated for a given provider.
///
/// Codex only returns `[Skills]` — the Codex adapter emits individual `.md` rule files but
/// Codex expects a single `~/.codex/AGENTS.md`; fixing the adapter is tracked in `TODO.md`.
pub fn all_sync_kinds(provider: Provider) -> Vec<SyncKind> {
    match provider {
        Provider::Claude => vec![SyncKind::Agents, SyncKind::Rules, SyncKind::Skills],
        Provider::Cursor => vec![SyncKind::Rules, SyncKind::Skills],
        Provider::OpenCode => vec![
            SyncKind::Agents,
            SyncKind::Commands,
            SyncKind::Rules,
            SyncKind::Skills,
        ],
    }
}

/// Returns the user-level destination directory for a provider/kind pair.
///
/// Implements the hardcoded convention table from the plan's Provider Conventions section.
pub fn user_dest_dir(provider: Provider, kind: SyncKind, home: &Path) -> PathBuf {
    match provider {
        Provider::Claude => home.join(".claude").join(kind.dir_name()),
        Provider::Cursor => home.join(".cursor").join(kind.dir_name()),
        Provider::OpenCode => home.join(".config").join("opencode").join(kind.dir_name()),
    }
}

/// Returns the project-local destination directory for a provider/kind pair.
pub fn project_dest_dir(provider: Provider, kind: SyncKind, cwd: &Path) -> PathBuf {
    let tool_dir = match provider {
        Provider::Claude => ".claude",
        Provider::Cursor => ".cursor",
        Provider::OpenCode => ".opencode",
    };
    cwd.join(tool_dir).join(kind.dir_name())
}

/// Returns the source directory within the generated output for a provider/kind pair.
pub fn generated_source_dir(provider: Provider, kind: SyncKind, generated_root: &Path) -> PathBuf {
    generated_root
        .join(provider.to_string())
        .join(kind.dir_name())
}

/// Expands a leading `~/` to the home directory. Returns the path unchanged otherwise.
pub fn expand_tilde(path: &str, home: &Path) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// Resolves the destination directory for a provider/kind pair from a `SyncTargetConfig`.
///
/// - `User` → `user_dest_dir`
/// - `Project` → `project_dest_dir`
/// - `Path` → per-kind explicit field from `config`; error if the kind's field is `None`
pub fn resolve_dest_dir(
    provider: Provider,
    kind: SyncKind,
    config: &SyncTargetConfig,
    home: &Path,
    cwd: &Path,
) -> Result<PathBuf> {
    match config.mode {
        SyncMode::User => Ok(user_dest_dir(provider, kind, home)),
        SyncMode::Project => Ok(project_dest_dir(provider, kind, cwd)),
        SyncMode::Path => {
            let raw = match kind {
                SyncKind::Agents => config.agents.as_deref(),
                SyncKind::Commands => config.commands.as_deref(),
                SyncKind::Rules => config.rules.as_deref(),
                SyncKind::Skills => config.skills.as_deref(),
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

/// Patches the `instructions` array in `opencode_config_dir/opencode.json`.
///
/// Ownership contract: agentspec owns any entry whose path falls under `rules_dest_dir`.
/// On each sync those entries are replaced wholesale; all other entries are preserved.
///
/// If `opencode.json` does not exist, it is created with just the `instructions` key.
///
/// When `dry_run` is true, prints the planned diff but does not write the file.
pub fn patch_opencode_instructions(
    rules_dest_dir: &Path,
    opencode_config_dir: &Path,
    dry_run: bool,
) -> Result<()> {
    let config_path = opencode_config_dir.join("opencode.json");

    // Read existing config (or start with empty object)
    let mut config: serde_json::Value = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("failed to read {}", config_path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("failed to parse {}", config_path.display()))?
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };

    // Get the existing instructions array (default to [])
    let existing: Vec<String> = config
        .get("instructions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    // Split into user-owned and agentspec-owned
    let user_entries: Vec<String> = existing
        .into_iter()
        .filter(|p| !Path::new(p).starts_with(rules_dest_dir))
        .collect();

    // Enumerate current rule files in rules_dest_dir
    let mut new_rule_paths: Vec<String> = if rules_dest_dir.is_dir() {
        WalkDir::new(rules_dest_dir)
            .min_depth(1)
            .follow_links(true) // dest entries may be directory-level symlinks (symlink strategy)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file() && e.file_name() == "AGENTS.md")
            .map(|e| e.path().to_string_lossy().into_owned())
            .collect()
    } else {
        Vec::new()
    };
    new_rule_paths.sort();

    let mut updated_instructions = user_entries;
    updated_instructions.extend(new_rule_paths);

    // Skip writing entirely when the file doesn't exist yet and there's nothing to record.
    // This avoids creating a spurious `opencode.json` when no rules have ever been synced.
    if !config_path.exists() && updated_instructions.is_empty() {
        return Ok(());
    }

    if dry_run {
        eprintln!(
            "would write {} instructions to {}",
            updated_instructions.len(),
            config_path.display()
        );
        return Ok(());
    }

    // Update the instructions key
    let instructions_value: Vec<serde_json::Value> = updated_instructions
        .into_iter()
        .map(serde_json::Value::String)
        .collect();
    if let Some(obj) = config.as_object_mut() {
        obj.insert(
            "instructions".to_string(),
            serde_json::Value::Array(instructions_value),
        );
    }

    let content =
        serde_json::to_string_pretty(&config).context("failed to serialize opencode.json")?;
    std::fs::write(&config_path, format!("{content}\n"))
        .with_context(|| format!("failed to write {}", config_path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/user")
    }

    fn cwd() -> PathBuf {
        PathBuf::from("/work/project")
    }

    #[test]
    fn test_resolve_dest_user_claude_agents() {
        let config = SyncTargetConfig::default(); // mode = User
        let result = resolve_dest_dir(Provider::Claude, SyncKind::Agents, &config, &home(), &cwd())
            .expect("expected value");
        assert_eq!(result, PathBuf::from("/home/user/.claude/agents"));
    }

    #[test]
    fn test_resolve_dest_project_cursor_skills() {
        let config = SyncTargetConfig {
            mode: SyncMode::Project,
            ..Default::default()
        };
        let result = resolve_dest_dir(Provider::Cursor, SyncKind::Skills, &config, &home(), &cwd())
            .expect("expected value");
        assert_eq!(result, PathBuf::from("/work/project/.cursor/skills"));
    }

    #[test]
    fn test_resolve_dest_path_explicit_skills() {
        let config = SyncTargetConfig {
            mode: SyncMode::Path,
            skills: Some("~/foo/skills".to_string()),
            ..Default::default()
        };
        let result = resolve_dest_dir(Provider::Cursor, SyncKind::Skills, &config, &home(), &cwd())
            .expect("expected value");
        assert_eq!(result, PathBuf::from("/home/user/foo/skills"));
    }

    #[test]
    fn test_resolve_dest_path_missing_agents_errors() {
        let config = SyncTargetConfig {
            mode: SyncMode::Path,
            ..Default::default()
        };
        let result = resolve_dest_dir(Provider::Claude, SyncKind::Agents, &config, &home(), &cwd());
        assert!(result.is_err());
    }

    #[test]
    fn test_all_sync_kinds_opencode_all_four() {
        let kinds = all_sync_kinds(Provider::OpenCode);
        assert!(kinds.contains(&SyncKind::Agents));
        assert!(kinds.contains(&SyncKind::Commands));
        assert!(kinds.contains(&SyncKind::Rules));
        assert!(kinds.contains(&SyncKind::Skills));
    }

    #[test]
    fn test_expand_tilde_replaces_home() {
        let result = expand_tilde("~/foo/bar", &home());
        assert_eq!(result, PathBuf::from("/home/user/foo/bar"));
    }

    #[test]
    fn test_expand_tilde_absolute_unchanged() {
        let result = expand_tilde("/absolute/path", &home());
        assert_eq!(result, PathBuf::from("/absolute/path"));
    }

    // patch_opencode_instructions tests

    #[test]
    fn test_patch_no_prior_config_creates_file() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");
        std::fs::create_dir_all(rules_dir.join("my-rule")).expect("expected value");
        std::fs::write(rules_dir.join("my-rule/AGENTS.md"), "rule").expect("expected value");

        patch_opencode_instructions(&rules_dir, tmp.path(), false).expect("expected value");

        let config_path = tmp.path().join("opencode.json");
        assert!(config_path.exists());
        let content = std::fs::read_to_string(&config_path).expect("expected value");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("expected value");
        let instructions = parsed["instructions"].as_array().expect("expected array");
        assert_eq!(instructions.len(), 1);
        assert!(
            instructions[0]
                .as_str()
                .expect("expected str")
                .contains("my-rule")
        );
    }

    #[test]
    fn test_patch_preserves_user_entries() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");
        std::fs::create_dir_all(rules_dir.join("my-rule")).expect("expected value");
        std::fs::write(rules_dir.join("my-rule/AGENTS.md"), "rule").expect("expected value");

        // Pre-populate with a user entry
        let config_path = tmp.path().join("opencode.json");
        std::fs::write(
            &config_path,
            r#"{"instructions": ["/user/custom/AGENTS.md"]}"#,
        )
        .expect("expected value");

        patch_opencode_instructions(&rules_dir, tmp.path(), false).expect("expected value");

        let content = std::fs::read_to_string(&config_path).expect("expected value");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("expected value");
        let instructions = parsed["instructions"].as_array().expect("expected array");
        let paths: Vec<&str> = instructions
            .iter()
            .map(|v| v.as_str().expect("expected str"))
            .collect();
        assert!(
            paths.contains(&"/user/custom/AGENTS.md"),
            "user entry preserved"
        );
        assert!(
            paths.iter().any(|p| p.contains("my-rule")),
            "agentspec entry added"
        );
    }

    #[test]
    fn test_patch_replaces_stale_agentspec_entries() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");
        std::fs::create_dir_all(rules_dir.join("new-rule")).expect("expected value");
        std::fs::write(rules_dir.join("new-rule/AGENTS.md"), "rule").expect("expected value");

        // Pre-populate with a stale agentspec entry AND a user entry
        let config_path = tmp.path().join("opencode.json");
        let stale_path = rules_dir.join("old-rule/AGENTS.md");
        let existing = serde_json::json!({
            "instructions": [
                stale_path.to_string_lossy(),
                "/user/AGENTS.md"
            ]
        });
        std::fs::write(
            &config_path,
            serde_json::to_string(&existing).expect("expected value"),
        )
        .expect("expected value");

        patch_opencode_instructions(&rules_dir, tmp.path(), false).expect("expected value");

        let content = std::fs::read_to_string(&config_path).expect("expected value");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("expected value");
        let instructions = parsed["instructions"].as_array().expect("expected array");
        let paths: Vec<&str> = instructions
            .iter()
            .map(|v| v.as_str().expect("expected str"))
            .collect();
        // Stale entry removed
        assert!(
            !paths.iter().any(|p| p.contains("old-rule")),
            "stale entry removed"
        );
        // New entry present
        assert!(
            paths.iter().any(|p| p.contains("new-rule")),
            "new entry present"
        );
        // User entry preserved
        assert!(paths.contains(&"/user/AGENTS.md"), "user entry preserved");
    }

    #[test]
    fn test_patch_empty_rules_dir_removes_agentspec_entries() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");
        std::fs::create_dir_all(&rules_dir).expect("expected value");

        let config_path = tmp.path().join("opencode.json");
        let stale_path = rules_dir.join("old-rule/AGENTS.md");
        let existing = serde_json::json!({
            "instructions": [stale_path.to_string_lossy(), "/user/AGENTS.md"]
        });
        std::fs::write(
            &config_path,
            serde_json::to_string(&existing).expect("expected value"),
        )
        .expect("expected value");

        patch_opencode_instructions(&rules_dir, tmp.path(), false).expect("expected value");

        let content = std::fs::read_to_string(&config_path).expect("expected value");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("expected value");
        let instructions = parsed["instructions"].as_array().expect("expected array");
        assert_eq!(instructions.len(), 1); // only user entry remains
        assert_eq!(
            instructions[0].as_str().expect("expected str"),
            "/user/AGENTS.md"
        );
    }

    #[test]
    fn test_patch_dry_run_no_file_written() {
        let tmp = tempfile::tempdir().expect("expected value");
        let rules_dir = tmp.path().join("rules");

        patch_opencode_instructions(&rules_dir, tmp.path(), true).expect("expected value");

        assert!(
            !tmp.path().join("opencode.json").exists(),
            "dry_run must not create file"
        );
    }
}
