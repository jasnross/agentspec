use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::cli::CommonArgs;
use crate::types::{PresetsMap, Provider, SyncMode, SyncStrategy};

/// Top-level config parsed from `agentspec.toml`.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct AgentspecConfig {
    pub spec: SpecConfig,
    pub output: OutputConfig,
    pub targets: Vec<Provider>,

    /// Model presets: preset name → per-provider model config.
    ///
    /// Each provider value is either a string shorthand (e.g., `"opus"`) or an
    /// object with `model`, `variant`, and/or `reasoning_effort` fields.
    #[serde(default)]
    pub presets: HashMap<String, HashMap<String, serde_json::Value>>,

    /// Per-machine profile overrides, keyed by profile name (e.g., `"home"`, `"work"`).
    ///
    /// When `AGENTSPEC_PROFILE=home`, the `home` profile merges over `presets` at the
    /// provider level within each named preset.
    #[serde(default)]
    pub profiles: HashMap<String, HashMap<String, HashMap<String, serde_json::Value>>>,

    /// Per-provider sync target configuration (e.g., `[sync.claude]`).
    #[serde(default)]
    pub sync: HashMap<String, SyncTargetConfig>,

    /// Root directory where agentspec.toml was found (not serialized).
    #[serde(skip)]
    pub root_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct SpecConfig {
    pub agents_dir: PathBuf,
    pub skills_dir: PathBuf,
    pub rules_dir: PathBuf,
    pub fragments_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    pub dir: PathBuf,
}

impl Default for AgentspecConfig {
    fn default() -> Self {
        Self {
            spec: SpecConfig::default(),
            output: OutputConfig::default(),
            targets: Provider::ALL.to_vec(),
            presets: HashMap::new(),
            profiles: HashMap::new(),
            sync: HashMap::new(),
            root_dir: PathBuf::new(),
        }
    }
}

impl Default for SpecConfig {
    fn default() -> Self {
        Self {
            agents_dir: PathBuf::from("spec/agents"),
            skills_dir: PathBuf::from("spec/skills"),
            rules_dir: PathBuf::from("spec/rules"),
            fragments_dir: PathBuf::from("spec/fragments"),
        }
    }
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("generated"),
        }
    }
}

/// Per-provider sync target configuration.
///
/// Controls where and how generated files are distributed for a single provider.
/// When `mode` is `Path`, the per-kind fields (`agents`, `skills`, `rules`, `commands`)
/// supply explicit destination directories (tilde-expanded at use site).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SyncTargetConfig {
    /// Where to place synced files (user-level, project-local, or explicit path).
    pub mode: SyncMode,
    /// How to distribute: symlink into `generated/` or copy with manifest tracking.
    pub strategy: SyncStrategy,
    /// Whether to strip `name:` lines from `SKILL.md` files after copying
    /// (used for plugin namespace prefixing in work profile).
    pub strip_name: bool,
    /// Optional namespace prefix applied to synced skill/agent/command names.
    /// For Claude: filesystem dir uses `{prefix}-{name}`, `name:` frontmatter uses `{prefix}:{name}`.
    /// For `OpenCode`: synced into a `{prefix}/` subdirectory.
    /// For Cursor/Codex: filename becomes `{prefix}-{name}`.
    /// Rules are never prefixed.
    pub prefix: Option<String>,
    /// Permit overwriting user-owned files at the destination.
    /// When false (default), sync errors on collision. Overridden by `--force`.
    #[serde(default)]
    pub allow_overwrite: bool,
    /// Explicit destination for agent specs (used when `mode = "path"`).
    pub agents: Option<String>,
    /// Explicit destination for skill specs.
    pub skills: Option<String>,
    /// Explicit destination for rule specs.
    pub rules: Option<String>,
    /// Explicit destination for command specs.
    pub commands: Option<String>,
}

/// Partial sync target config used for profile overlay deserialization.
///
/// All fields are `Option` so we can distinguish "absent in TOML" from "set to default".
/// Only `Some` values override the base config during merge.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SyncTargetPartial {
    pub mode: Option<SyncMode>,
    pub strategy: Option<SyncStrategy>,
    pub strip_name: Option<bool>,
    pub prefix: Option<String>,
    pub allow_overwrite: Option<bool>,
    pub agents: Option<String>,
    pub skills: Option<String>,
    pub rules: Option<String>,
    pub commands: Option<String>,
}

/// CLI flag overrides for sync target resolution (highest precedence).
#[derive(Debug, Clone, Default)]
pub struct SyncOverrides {
    /// Override sync mode for all providers.
    pub mode: Option<SyncMode>,
    /// Override sync strategy for all providers.
    pub strategy: Option<SyncStrategy>,
    /// Override destination root (implies `mode = Path`).
    pub dest: Option<String>,
    /// Allow overwriting user-owned files at sync destinations.
    pub force: bool,
}

impl SyncTargetConfig {
    /// Validate sync settings for provider-specific constraints.
    pub fn validate_for_sync(&self, provider: Provider) -> Result<()> {
        if self.prefix.is_some() && self.strip_name {
            bail!("sync config for {provider}: `prefix` and `strip_name` are mutually exclusive");
        }

        if self.prefix.is_some()
            && provider == Provider::Claude
            && self.strategy == SyncStrategy::Symlink
        {
            bail!(
                "sync config for {provider}: `prefix` requires `strategy = \"copy\"` \
                 because Claude skill/agent names come from frontmatter"
            );
        }

        Ok(())
    }
}

impl AgentspecConfig {
    /// Discover `agentspec.toml` by walking up from `start_dir`.
    /// If not found, returns a default config rooted at `start_dir`.
    pub fn discover(start_dir: &Path) -> Result<Self> {
        let mut dir = start_dir.to_path_buf();
        loop {
            let candidate = dir.join("agentspec.toml");
            if candidate.is_file() {
                let content = std::fs::read_to_string(&candidate)
                    .with_context(|| format!("failed to read {}", candidate.display()))?;
                let mut config: AgentspecConfig = toml::from_str(&content)
                    .with_context(|| format!("failed to parse {}", candidate.display()))?;
                config.root_dir = dir;
                return Ok(config);
            }
            if !dir.pop() {
                break;
            }
        }

        // No config file found — use defaults rooted at start_dir
        Ok(AgentspecConfig {
            root_dir: start_dir.to_path_buf(),
            ..Default::default()
        })
    }

    /// Apply CLI flag overrides to this config.
    pub fn apply_overrides(&mut self, args: &CommonArgs) {
        if !args.target.is_empty() {
            self.targets.clone_from(&args.target);
        }
    }

    /// Resolve a config-relative path to an absolute path.
    pub fn resolve(&self, relative: &Path) -> PathBuf {
        self.root_dir.join(relative)
    }

    /// Resolve model presets, applying the named machine profile overlay if provided.
    ///
    /// When `active_profile` is `Some("home")`, the `[profiles.home.*]` entries
    /// are merged over the base `[presets.*]` entries at the provider level.
    pub fn resolve_presets(&self, active_profile: Option<&str>) -> PresetsMap {
        let mut resolved = self.presets.clone();

        if let Some(profile_name) = active_profile
            && let Some(overrides) = self.profiles.get(profile_name)
        {
            for (preset_key, provider_overrides) in overrides {
                let entry = resolved.entry(preset_key.clone()).or_default();
                for (provider, value) in provider_overrides {
                    entry.insert(provider.clone(), value.clone());
                }
            }
        }

        resolved
    }

    /// Resolve the sync target config for a provider, merging base → profile → CLI overrides.
    ///
    /// Precedence (highest wins): CLI `SyncOverrides` → profile overlay → base `[sync.<provider>]`.
    /// Unknown profiles are silently ignored (consistent with `resolve_presets`).
    pub fn resolve_sync_target(
        &self,
        provider: Provider,
        active_profile: Option<&str>,
        cli: &SyncOverrides,
    ) -> SyncTargetConfig {
        let provider_str = provider.to_string();

        // Start with base config (or default if not configured)
        let mut resolved = self.sync.get(&provider_str).cloned().unwrap_or_default();

        // Apply profile overlay if present (only explicitly-set fields override)
        if let Some(profile_name) = active_profile
            && let Some(profile) = self.profiles.get(profile_name)
            && let Some(sync_overrides) = profile.get("sync")
            && let Some(provider_value) = sync_overrides.get(&provider_str)
        {
            match serde_json::from_value::<SyncTargetPartial>(provider_value.clone()) {
                Ok(partial) => {
                    if let Some(mode) = partial.mode {
                        resolved.mode = mode;
                    }
                    if let Some(strategy) = partial.strategy {
                        resolved.strategy = strategy;
                    }
                    if let Some(strip_name) = partial.strip_name {
                        resolved.strip_name = strip_name;
                    }
                    if partial.prefix.is_some() {
                        resolved.prefix = partial
                            .prefix
                            .and_then(|value| if value.is_empty() { None } else { Some(value) });
                    }
                    if let Some(allow_overwrite) = partial.allow_overwrite {
                        resolved.allow_overwrite = allow_overwrite;
                    }
                    if partial.agents.is_some() {
                        resolved.agents = partial.agents;
                    }
                    if partial.skills.is_some() {
                        resolved.skills = partial.skills;
                    }
                    if partial.rules.is_some() {
                        resolved.rules = partial.rules;
                    }
                    if partial.commands.is_some() {
                        resolved.commands = partial.commands;
                    }
                }
                Err(e) => {
                    eprintln!(
                        "warning: invalid sync config in profile '{profile_name}' for provider '{provider_str}': {e}"
                    );
                }
            }
        }

        // Apply CLI overrides last (highest precedence)
        if let Some(mode) = cli.mode {
            resolved.mode = mode;
        }
        if let Some(strategy) = cli.strategy {
            resolved.strategy = strategy;
        }
        if let Some(ref dest) = cli.dest {
            resolved.mode = SyncMode::Path;
            resolved.agents = Some(format!("{dest}/agents"));
            resolved.skills = Some(format!("{dest}/skills"));
            resolved.rules = Some(format!("{dest}/rules"));
            resolved.commands = Some(format!("{dest}/commands"));
        }
        if cli.force {
            resolved.allow_overwrite = true;
        }

        if resolved.prefix.as_deref() == Some("") {
            resolved.prefix = None;
        }

        resolved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_default_config() {
        let config = AgentspecConfig::default();
        assert_eq!(config.spec.agents_dir, PathBuf::from("spec/agents"));
        assert_eq!(config.spec.skills_dir, PathBuf::from("spec/skills"));
        assert_eq!(config.spec.fragments_dir, PathBuf::from("spec/fragments"));
        assert_eq!(config.output.dir, PathBuf::from("generated"));
        assert_eq!(config.targets.len(), 4);
        assert!(config.presets.is_empty());
        assert!(config.profiles.is_empty());
        assert!(config.sync.is_empty());
    }

    #[test]
    fn test_discover_with_toml() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[spec]
agents_dir = "my/agents"

[output]
dir = "out"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");
        assert_eq!(config.spec.agents_dir, PathBuf::from("my/agents"));
        assert_eq!(config.output.dir, PathBuf::from("out"));
        // skills_dir should use default
        assert_eq!(config.spec.skills_dir, PathBuf::from("spec/skills"));
        assert_eq!(config.root_dir, tmp.path());
    }

    #[test]
    fn test_discover_without_toml() {
        let tmp = tempfile::tempdir().expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");
        assert_eq!(config.spec.agents_dir, PathBuf::from("spec/agents"));
        assert_eq!(config.root_dir, tmp.path());
    }

    #[test]
    fn test_discover_with_presets() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[presets.deep_review]
claude = "opus"

[presets.balanced]
claude = "sonnet"
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");
        assert_eq!(config.presets.len(), 2);
        assert!(config.presets.contains_key("deep_review"));
        assert!(config.presets.contains_key("balanced"));

        let deep = &config.presets["deep_review"];
        assert_eq!(deep["claude"], serde_json::json!("opus"));

        let balanced = &config.presets["balanced"];
        assert_eq!(
            balanced["opencode"],
            serde_json::json!({"model": "anthropic/claude-sonnet-4-5", "variant": "high"})
        );
    }

    #[test]
    fn test_resolve_presets_no_profile() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[presets.balanced]
claude = "sonnet"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");

        let resolved = config.resolve_presets(None);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved["balanced"]["claude"], serde_json::json!("sonnet"));
    }

    #[test]
    fn test_resolve_presets_with_profile() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[presets.balanced]
claude = "sonnet"
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }

[profiles.home.balanced]
opencode = { model = "openai/gpt-5.3-codex", variant = "medium" }
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");

        // Without profile: uses base presets
        let base = config.resolve_presets(None);
        assert_eq!(
            base["balanced"]["opencode"],
            serde_json::json!({"model": "anthropic/claude-sonnet-4-5", "variant": "high"})
        );

        // With home profile: opencode is overridden
        let home = config.resolve_presets(Some("home"));
        assert_eq!(
            home["balanced"]["opencode"],
            serde_json::json!({"model": "openai/gpt-5.3-codex", "variant": "medium"})
        );
        // claude is unchanged
        assert_eq!(home["balanced"]["claude"], serde_json::json!("sonnet"));
    }

    #[test]
    fn test_resolve_presets_unknown_profile_is_noop() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[presets.balanced]
claude = "sonnet"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");

        let resolved = config.resolve_presets(Some("nonexistent"));
        assert_eq!(resolved["balanced"]["claude"], serde_json::json!("sonnet"));
    }

    // -----------------------------------------------------------------------
    // resolve_sync_target tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_sync_target_default_when_no_sync_configured() {
        let config = AgentspecConfig::default();
        let cli = SyncOverrides::default();
        let result = config.resolve_sync_target(Provider::Claude, None, &cli);
        assert_eq!(result.mode, SyncMode::User);
        assert_eq!(result.strategy, SyncStrategy::Symlink);
        assert!(!result.strip_name);
        assert!(result.prefix.is_none());
        assert!(!result.allow_overwrite);
        assert!(result.agents.is_none());
    }

    #[test]
    fn test_resolve_sync_target_applies_base_config() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[sync.claude]
strategy = "copy"
strip_name = true
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");
        let cli = SyncOverrides::default();

        let result = config.resolve_sync_target(Provider::Claude, None, &cli);
        assert_eq!(result.strategy, SyncStrategy::Copy);
        assert!(result.strip_name);
        assert!(result.prefix.is_none());
        assert!(!result.allow_overwrite);
        assert_eq!(result.mode, SyncMode::User); // default
    }

    #[test]
    fn test_resolve_sync_target_profile_overrides_base() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[sync.claude]
strategy = "symlink"

[profiles.work.sync.claude]
strategy = "copy"
strip_name = true
skills = "~/Workspace/thoughts/plugin/claude/skills"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");
        let cli = SyncOverrides::default();

        // Without profile: base config
        let base = config.resolve_sync_target(Provider::Claude, None, &cli);
        assert_eq!(base.strategy, SyncStrategy::Symlink);
        assert!(!base.strip_name);

        // With profile: overridden
        let work = config.resolve_sync_target(Provider::Claude, Some("work"), &cli);
        assert_eq!(work.strategy, SyncStrategy::Copy);
        assert!(work.strip_name);
        assert_eq!(
            work.skills.as_deref(),
            Some("~/Workspace/thoughts/plugin/claude/skills")
        );
    }

    #[test]
    fn test_resolve_sync_target_unknown_profile_is_noop() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[sync.claude]
strategy = "copy"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");
        let cli = SyncOverrides::default();

        let result = config.resolve_sync_target(Provider::Claude, Some("nonexistent"), &cli);
        assert_eq!(result.strategy, SyncStrategy::Copy); // base preserved
    }

    #[test]
    fn test_resolve_sync_target_cli_overrides_win() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[sync.claude]
strategy = "symlink"
mode = "user"

[profiles.work.sync.claude]
strategy = "copy"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");

        let cli = SyncOverrides {
            mode: Some(SyncMode::Project),
            strategy: Some(SyncStrategy::Symlink),
            dest: None,
            force: false,
        };

        // CLI overrides both base and profile
        let result = config.resolve_sync_target(Provider::Claude, Some("work"), &cli);
        assert_eq!(result.mode, SyncMode::Project);
        assert_eq!(result.strategy, SyncStrategy::Symlink);
    }

    #[test]
    fn test_resolve_sync_target_partial_profile_preserves_base() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[sync.claude]
mode = "path"
strategy = "copy"
strip_name = true
agents = "~/custom/agents"

[profiles.work.sync.claude]
strip_name = false
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");
        let cli = SyncOverrides::default();

        let result = config.resolve_sync_target(Provider::Claude, Some("work"), &cli);
        // Profile only set strip_name — mode, strategy, agents must be preserved from base
        assert_eq!(result.mode, SyncMode::Path);
        assert_eq!(result.strategy, SyncStrategy::Copy);
        assert!(!result.strip_name); // overridden to false
        assert_eq!(result.agents.as_deref(), Some("~/custom/agents"));
    }

    #[test]
    fn test_resolve_sync_target_cli_dest_implies_path_mode() {
        let config = AgentspecConfig::default();
        let cli = SyncOverrides {
            mode: None,
            strategy: None,
            dest: Some("/tmp/sync-test".to_string()),
            force: false,
        };

        let result = config.resolve_sync_target(Provider::Claude, None, &cli);
        assert_eq!(result.mode, SyncMode::Path);
        assert_eq!(result.agents.as_deref(), Some("/tmp/sync-test/agents"));
        assert_eq!(result.skills.as_deref(), Some("/tmp/sync-test/skills"));
        assert_eq!(result.rules.as_deref(), Some("/tmp/sync-test/rules"));
        assert_eq!(result.commands.as_deref(), Some("/tmp/sync-test/commands"));
    }

    #[test]
    fn test_resolve_sync_target_empty_prefix_normalized_to_none() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[sync.claude]
prefix = ""
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");

        let result = config.resolve_sync_target(Provider::Claude, None, &SyncOverrides::default());
        assert!(result.prefix.is_none());
    }

    #[test]
    fn test_resolve_sync_target_cli_force_sets_allow_overwrite() {
        let config = AgentspecConfig::default();
        let cli = SyncOverrides {
            force: true,
            ..SyncOverrides::default()
        };

        let result = config.resolve_sync_target(Provider::Claude, None, &cli);
        assert!(result.allow_overwrite);
    }

    #[test]
    fn test_validate_prefix_strip_name_conflict() {
        let target = SyncTargetConfig {
            prefix: Some("tw".to_string()),
            strip_name: true,
            ..SyncTargetConfig::default()
        };

        let result = target.validate_for_sync(Provider::Claude);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_prefix_requires_copy_for_claude() {
        let target = SyncTargetConfig {
            prefix: Some("tw".to_string()),
            strategy: SyncStrategy::Symlink,
            ..SyncTargetConfig::default()
        };

        let result = target.validate_for_sync(Provider::Claude);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_prefix_symlink_ok_for_opencode() {
        let target = SyncTargetConfig {
            prefix: Some("tw".to_string()),
            strategy: SyncStrategy::Symlink,
            ..SyncTargetConfig::default()
        };

        let result = target.validate_for_sync(Provider::OpenCode);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_prefix_none_no_error() {
        let target = SyncTargetConfig::default();

        for provider in Provider::ALL {
            assert!(target.validate_for_sync(provider).is_ok());
        }
    }
}
