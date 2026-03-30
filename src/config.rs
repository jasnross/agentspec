use std::collections::HashMap;
use std::path::{Path, PathBuf};

use agentspec::presets::ProviderPresets;
use agentspec::provider::Provider;
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use strum::VariantArray;

use crate::cli::CommonArgs;

/// Where synced files should be placed.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    /// Sync to user-level config dirs (`~/.claude/`, `~/.config/opencode/`, etc.)
    #[default]
    User,
    /// Sync to project-local config dirs (`.claude/`, `.cursor/`, etc.)
    Project,
    /// Sync to explicit paths specified per-kind in `SyncTargetConfig`
    Path,
}

/// How files are distributed from `generated/` to the destination.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum SyncStrategy {
    /// Create symlinks from destination into `generated/`
    #[default]
    Symlink,
    /// Copy files and track ownership via `.agentspec-manifest.json`
    Copy,
}

/// Top-level config parsed from `agentspec.toml`.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentspecConfig {
    pub spec: SpecConfig,
    pub output: OutputConfig,
    pub providers: Vec<Provider>,

    /// Model presets: preset name → per-provider model config.
    ///
    /// Each provider value is an object with provider-specific fields
    /// (`model`, `variant`, `reasoning_effort`, etc.).
    #[serde(default)]
    pub presets: HashMap<String, ProviderPresets>,

    /// Per-provider sync target configuration (e.g., `[sync.claude]`).
    #[serde(default)]
    pub sync: HashMap<String, SyncTargetConfig>,

    /// Root directory where agentspec.toml was found (not serialized).
    #[serde(skip)]
    pub root_dir: PathBuf,
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
                let mut config: AgentspecConfig =
                    serde_path_to_error::deserialize(toml::de::Deserializer::new(&content))
                        .map_err(|error| {
                            let path = error.path().to_string();
                            let location = if path.is_empty() { "<root>" } else { &path };
                            anyhow!(
                                "failed to parse {} at `{location}`: {}",
                                candidate.display(),
                                error.into_inner()
                            )
                        })?;
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
        if !args.provider.is_empty() {
            self.providers.clone_from(&args.provider);
        }
    }

    /// Resolve a config-relative path to an absolute path.
    pub fn resolve(&self, relative: &Path) -> PathBuf {
        self.root_dir.join(relative)
    }

    /// Resolve the sync target config for a provider, merging base → CLI overrides.
    ///
    /// Precedence (highest wins): CLI `SyncOverrides` → base `[sync.<provider>]`.
    /// FIXME: Does this belong in the sync module?
    pub fn resolve_sync_target(&self, provider: Provider, cli: &SyncOverrides) -> SyncTargetConfig {
        let provider_str = provider.to_string();

        // Start with base config (or default if not configured)
        let mut resolved = self.sync.get(&provider_str).cloned().unwrap_or_default();

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

    /// Returns providers with explicit sync config.
    pub fn configured_sync_providers(&self) -> Vec<Provider> {
        Provider::VARIANTS
            .iter()
            .copied()
            .fold(Vec::new(), |mut acc, provider| {
                if self.has_explicit_sync_config(provider) {
                    acc.push(provider);
                }
                acc
            })
    }

    /// Returns whether a provider has explicit sync config.
    pub fn has_explicit_sync_config(&self, provider: Provider) -> bool {
        let provider_str = provider.to_string();
        self.sync.contains_key(&provider_str)
    }

    /// Returns whether CLI flags provide sufficient explicit intent for CLI-only sync.
    ///
    /// CLI-only sync always requires explicit provider selection via `--provider`.
    pub fn cli_sync_intent_sufficient(
        cli: &SyncOverrides,
        has_explicit_target_selection: bool,
    ) -> bool {
        if !has_explicit_target_selection {
            return false;
        }

        if cli.dest.is_some() {
            return true;
        }

        matches!(cli.mode, Some(SyncMode::User | SyncMode::Project))
    }

    /// Resolves effective sync intent metadata for a provider.
    /// FIXME: Does this belong in the sync module?
    pub fn resolve_sync_intent(
        &self,
        provider: Provider,
        cli: &SyncOverrides,
        has_explicit_provider_selection: bool, // FIXME: Can we remove boolean by passing something more structured somehow?
    ) -> SyncIntent {
        let has_explicit_config = self.has_explicit_sync_config(provider);
        let cli_only_allowed =
            Self::cli_sync_intent_sufficient(cli, has_explicit_provider_selection);
        let target = self.resolve_sync_target(provider, cli);

        SyncIntent {
            target,
            has_explicit_config,
            cli_only_allowed,
        }
    }
}

impl Default for AgentspecConfig {
    fn default() -> Self {
        Self {
            spec: SpecConfig::default(),
            output: OutputConfig::default(),
            providers: Provider::VARIANTS.to_vec(),
            presets: HashMap::new(),
            sync: HashMap::new(),
            root_dir: PathBuf::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpecConfig {
    pub agents_dir: PathBuf,
    pub skills_dir: PathBuf,
    pub rules_dir: PathBuf,
    pub fragments_dir: PathBuf,
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

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutputConfig {
    pub dir: PathBuf,
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
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SyncTargetConfig {
    /// Where to place synced files (user-level, project-local, or explicit path).
    pub mode: SyncMode,
    /// How to distribute: symlink or copy with manifest tracking.
    pub strategy: SyncStrategy,
    /// Whether to strip `name:` lines from `SKILL.md` files after copying
    pub strip_name: bool,
    /// Optional namespace prefix applied to synced skill/agent/command names.
    /// For Claude: filesystem dir uses `{prefix}-{name}`, `name:` frontmatter uses `{prefix}:{name}`.
    /// For `OpenCode`: synced into a `{prefix}/` subdirectory.
    /// For Cursor/Codex: filename becomes `{prefix}-{name}`.
    /// Rules are never prefixed.
    /// FIXME: prefix rules too
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

/// CLI flag overrides for sync target resolution (highest precedence).
/// FIXME: Consider if other sync flags should be allowed here
#[derive(Clone, Debug, Default)]
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

/// Effective sync intent for a provider after config/CLI evaluation.
#[derive(Clone, Debug)]
pub struct SyncIntent {
    /// Final sync target after applying base -> CLI precedence.
    pub target: SyncTargetConfig,
    /// Whether this provider has explicit sync config in base.
    /// FIXME: Do we really need this boolean or can we do something more structured?
    pub has_explicit_config: bool,
    /// Whether CLI-only sync is allowed for this provider in this invocation.
    /// FIXME: Do we really need this boolean or can we do something more structured?
    pub cli_only_allowed: bool,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use agentspec::presets::{ClaudePreset, CursorPreset, OpenCodePreset};

    use super::*;

    #[test]
    fn test_default_config() {
        let config = AgentspecConfig::default();
        assert_eq!(config.spec.agents_dir, PathBuf::from("spec/agents"));
        assert_eq!(config.spec.skills_dir, PathBuf::from("spec/skills"));
        assert_eq!(config.spec.fragments_dir, PathBuf::from("spec/fragments"));
        assert_eq!(config.output.dir, PathBuf::from("generated"));
        assert_eq!(config.providers.len(), 3);
        assert!(config.presets.is_empty());
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
    fn test_discover_invalid_sync_field_errors() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[sync.cursor]
invalid_field = "oops"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let err = AgentspecConfig::discover(tmp.path()).expect_err("expected parse error");
        let full = format!("{err:#}");
        assert!(full.contains("failed to parse"), "error: {full}");
        assert!(full.contains("unknown field"), "error: {full}");
    }

    #[test]
    fn test_discover_parse_error_includes_field_path() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r"
[presets.bad]
claude = 42
";
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let err = AgentspecConfig::discover(tmp.path()).expect_err("expected parse error");
        let full = format!("{err:#}");
        assert!(full.contains("failed to parse"), "error: {full}");
        assert!(full.contains("presets.bad.claude"), "error: {full}");
    }

    #[test]
    fn test_discover_rejects_preset_shorthand_string() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[presets.bad]
claude = "opus"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let err = AgentspecConfig::discover(tmp.path()).expect_err("expected parse error");
        let full = format!("{err:#}");
        assert!(full.contains("failed to parse"), "error: {full}");
        assert!(full.contains("presets.bad.claude"), "error: {full}");
    }

    #[test]
    fn test_discover_with_presets() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[presets.deep_review]
claude = { model = "opus" }

[presets.balanced]
claude = { model = "sonnet" }
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
cursor = { model = "fast" }
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");
        assert_eq!(config.presets.len(), 2);
        assert!(config.presets.contains_key("deep_review"));
        assert!(config.presets.contains_key("balanced"));

        let deep = &config.presets["deep_review"];
        assert_eq!(
            deep.claude,
            Some(ClaudePreset {
                model: Some("opus".to_string())
            })
        );

        let balanced = &config.presets["balanced"];
        assert_eq!(
            balanced.opencode,
            Some(OpenCodePreset {
                model: Some("anthropic/claude-sonnet-4-5".to_string()),
                variant: Some("high".to_string())
            })
        );
        assert_eq!(
            balanced.cursor,
            Some(CursorPreset {
                model: Some("fast".to_string())
            })
        );
    }

    // -----------------------------------------------------------------------
    // resolve_sync_target tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_sync_target_default_when_no_sync_configured() {
        let config = AgentspecConfig::default();
        let cli = SyncOverrides::default();
        let result = config.resolve_sync_target(Provider::Claude, &cli);
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

        let result = config.resolve_sync_target(Provider::Claude, &cli);
        assert_eq!(result.strategy, SyncStrategy::Copy);
        assert!(result.strip_name);
        assert!(result.prefix.is_none());
        assert!(!result.allow_overwrite);
        assert_eq!(result.mode, SyncMode::User); // default
    }

    #[test]
    fn test_resolve_sync_target_cli_overrides_win() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[sync.claude]
strategy = "symlink"
mode = "user"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");

        let cli = SyncOverrides {
            mode: Some(SyncMode::Project),
            strategy: Some(SyncStrategy::Symlink),
            dest: None,
            force: false,
        };

        // CLI overrides base
        let result = config.resolve_sync_target(Provider::Claude, &cli);
        assert_eq!(result.mode, SyncMode::Project);
        assert_eq!(result.strategy, SyncStrategy::Symlink);
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

        let result = config.resolve_sync_target(Provider::Claude, &cli);
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

        let result = config.resolve_sync_target(Provider::Claude, &SyncOverrides::default());
        assert!(result.prefix.is_none());
    }

    #[test]
    fn test_resolve_sync_target_cli_force_sets_allow_overwrite() {
        let config = AgentspecConfig::default();
        let cli = SyncOverrides {
            force: true,
            ..SyncOverrides::default()
        };

        let result = config.resolve_sync_target(Provider::Claude, &cli);
        assert!(result.allow_overwrite);
    }

    // -----------------------------------------------------------------------
    // explicit sync intent tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_has_explicit_sync_config_detects_base() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[sync.claude]
strategy = "copy"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");

        assert!(config.has_explicit_sync_config(Provider::Claude));
        assert!(!config.has_explicit_sync_config(Provider::Cursor));
    }

    #[test]
    fn test_cli_sync_intent_requires_explicit_target_selection() {
        let cli = SyncOverrides {
            mode: Some(SyncMode::User),
            ..SyncOverrides::default()
        };

        assert!(!AgentspecConfig::cli_sync_intent_sufficient(&cli, false));
    }

    #[test]
    fn test_cli_sync_intent_dest_with_target_is_sufficient() {
        let cli = SyncOverrides {
            dest: Some("/tmp/out".to_string()),
            ..SyncOverrides::default()
        };

        assert!(AgentspecConfig::cli_sync_intent_sufficient(&cli, true));
    }

    #[test]
    fn test_cli_sync_intent_mode_user_with_target_is_sufficient() {
        let cli = SyncOverrides {
            mode: Some(SyncMode::User),
            ..SyncOverrides::default()
        };

        assert!(AgentspecConfig::cli_sync_intent_sufficient(&cli, true));
    }

    #[test]
    fn test_cli_sync_intent_mode_project_with_target_is_sufficient() {
        let cli = SyncOverrides {
            mode: Some(SyncMode::Project),
            ..SyncOverrides::default()
        };

        assert!(AgentspecConfig::cli_sync_intent_sufficient(&cli, true));
    }

    #[test]
    fn test_cli_sync_intent_mode_path_without_dest_is_insufficient() {
        let cli = SyncOverrides {
            mode: Some(SyncMode::Path),
            ..SyncOverrides::default()
        };

        assert!(!AgentspecConfig::cli_sync_intent_sufficient(&cli, true));
    }

    #[test]
    fn test_cli_sync_intent_strategy_only_is_insufficient() {
        let cli = SyncOverrides {
            strategy: Some(SyncStrategy::Copy),
            ..SyncOverrides::default()
        };

        assert!(!AgentspecConfig::cli_sync_intent_sufficient(&cli, true));
    }

    #[test]
    fn test_resolve_sync_intent_reports_config_and_cli_metadata() {
        let tmp = tempfile::tempdir().expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");
        let cli = SyncOverrides {
            mode: Some(SyncMode::Project),
            ..SyncOverrides::default()
        };

        let intent = config.resolve_sync_intent(Provider::Cursor, &cli, true);
        assert!(!intent.has_explicit_config); // no agentspec.toml → no explicit sync config
        assert!(intent.cli_only_allowed);
        assert_eq!(intent.target.mode, SyncMode::Project);
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

        for &provider in Provider::VARIANTS {
            assert!(target.validate_for_sync(provider).is_ok());
        }
    }
}
