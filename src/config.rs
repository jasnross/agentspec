use std::collections::HashMap;
use std::path::{Path, PathBuf};

use agentspec::compile::AdapterConfig;
use agentspec::presets::ProviderPresets;
use agentspec::provider::Provider;
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use strum::VariantArray;

/// Top-level config parsed from `agentspec.toml`.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentspecConfig {
    pub spec: SpecConfig,
    pub compile: CompileConfig,

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

    /// Resolve a config-relative path to an absolute path.
    pub fn resolve(&self, relative: &Path) -> PathBuf {
        self.root_dir.join(relative)
    }

    /// Resolve the sync target config for a provider, merging base → CLI overrides.
    ///
    /// Precedence (highest wins): CLI `SyncOverrides` → base `[sync.<provider>]`.
    pub fn resolve_sync_target(&self, provider: Provider, cli: &SyncFlags) -> SyncTargetConfig {
        let provider_str = provider.to_string();

        // Start with base config (or default if not configured)
        let mut resolved = self.sync.get(&provider_str).cloned().unwrap_or_default();

        // Apply CLI overrides last (highest precedence)
        if let Some(mode) = cli.mode {
            resolved.mode = mode;
        }
        if let Some(ref dest) = cli.dest {
            resolved.mode = SyncMode::Path;
            resolved.dir = Some(dest.clone());
        }
        if cli.force {
            resolved.overwrite = true;
        }

        if let Some(prefix) = cli.prefix.as_deref() {
            resolved.prefix = Some(prefix.to_string());
        }
        if let Some(content_prefix) = cli.content_prefix.as_deref() {
            resolved.content_prefix = Some(content_prefix.to_string());
        }

        if resolved.prefix.as_deref() == Some("") {
            resolved.prefix = None;
        }
        if resolved.content_prefix.as_deref() == Some("") {
            resolved.content_prefix = None;
        }

        resolved
    }

    /// Returns providers with explicit sync config.
    pub fn configured_sync_providers(&self) -> Vec<Provider> {
        Provider::VARIANTS
            .iter()
            .copied()
            .fold(Vec::new(), |mut acc, provider| {
                if self.has_sync_config(provider) {
                    acc.push(provider);
                }
                acc
            })
    }

    /// Returns `(provider, target)` pairs for providers with explicit `[sync.<provider>]` config.
    ///
    /// These are the raw TOML values without CLI overrides. Used by the compile command
    /// to apply `prefix` transforms to `generated/` output.
    pub fn sync_targets(&self) -> Vec<(Provider, SyncTargetConfig)> {
        self.configured_sync_providers()
            .into_iter()
            .filter_map(|p| self.sync.get(&p.to_string()).map(|t| (p, t.clone())))
            .collect()
    }

    /// Builds per-provider `AdapterConfig` from resolved sync targets.
    ///
    /// Providers absent from `targets` are absent from the map, causing adapters
    /// to produce canonical (unprefixed) output.
    ///
    /// Maps each target's `SyncMode` to a library-side `HookEmitMode` here so
    /// the library never imports `SyncMode` (preserving the binary/library
    /// boundary established in `CLAUDE.md`'s "Use config structs at module
    /// boundaries" guidance).
    pub fn adapter_configs(
        targets: &[(Provider, SyncTargetConfig)],
    ) -> HashMap<Provider, AdapterConfig> {
        targets
            .iter()
            .map(|(p, t)| {
                let hook_emit_mode = Some(match t.mode {
                    SyncMode::Path => agentspec::compile::HookEmitMode::Bundled,
                    SyncMode::User | SyncMode::Project => agentspec::compile::HookEmitMode::Merged,
                });
                (
                    *p,
                    AdapterConfig {
                        prefix: t.prefix.clone(),
                        content_prefix: t.content_prefix.clone(),
                        hook_emit_mode,
                    },
                )
            })
            .collect()
    }

    /// Returns whether a provider has explicit sync config.
    pub fn has_sync_config(&self, provider: Provider) -> bool {
        let provider_str = provider.to_string();
        self.sync.contains_key(&provider_str)
    }

    /// Returns whether CLI flags provide sufficient explicit intent for CLI-only sync.
    ///
    /// CLI-only sync always requires explicit provider selection via `--provider`.
    fn cli_sync_intent_sufficient(cli: &SyncFlags, has_target_selection: bool) -> bool {
        if !has_target_selection {
            return false;
        }

        if cli.dest.is_some() {
            return true;
        }

        matches!(cli.mode, Some(SyncMode::User | SyncMode::Project))
    }

    /// Resolves the effective sync target for a provider, returning an error if the
    /// invocation is not valid.
    ///
    /// Validation requires either explicit sync config in `agentspec.toml` for
    /// the provider, or an explicit `--provider` selection combined with sufficient
    /// CLI flags (`--mode user|project` or `--dest`).
    pub fn validated_sync_target(
        &self,
        provider: Provider,
        cli: &SyncFlags,
        has_explicit_provider_selection: bool,
    ) -> Result<SyncTargetConfig> {
        let has_explicit_config = self.has_sync_config(provider);
        let cli_only_allowed =
            Self::cli_sync_intent_sufficient(cli, has_explicit_provider_selection);

        if !has_explicit_config && !cli_only_allowed {
            bail!(
                "sync is not configured for {provider}. Configure [sync.{provider}] in agentspec.toml or specify additional arguments (--mode user|project or --dest <path>)"
            );
        }

        Ok(self.resolve_sync_target(provider, cli))
    }
}

impl Default for AgentspecConfig {
    fn default() -> Self {
        Self {
            spec: SpecConfig::default(),
            compile: CompileConfig::default(),
            presets: HashMap::new(),
            sync: HashMap::new(),
            root_dir: PathBuf::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpecConfig {
    /// Base directory containing spec subdirectories (`agents/`, `skills/`,
    /// `rules/`, `fragments/`).
    pub sources_dir: PathBuf,

    /// Glob patterns of files to exclude from the spec tree during loading.
    ///
    /// Patterns are matched against paths relative to `sources_dir`. Slashless
    /// patterns match only top-level entries — use `**/pattern` to match at
    /// any depth. Defaults to an empty list.
    pub ignore: Vec<String>,
}

impl Default for SpecConfig {
    fn default() -> Self {
        Self {
            sources_dir: PathBuf::from("spec"),
            ignore: Vec::new(),
        }
    }
}

impl SpecConfig {
    /// Compile the raw `ignore` patterns into an [`IgnoreMatcher`].
    ///
    /// Returns an error naming the first malformed pattern.
    pub fn compile_ignore_matcher(&self) -> Result<agentspec::specs::IgnoreMatcher> {
        agentspec::specs::IgnoreMatcher::compile(&self.ignore)
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CompileConfig {
    pub output_dir: PathBuf,
}

impl Default for CompileConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("generated"),
        }
    }
}

/// Per-provider sync target configuration.
///
/// Controls where and how generated files are distributed for a single provider.
/// When `mode` is `Path`, the per-kind fields (`agents`, `skills`, `rules`, `commands`)
/// supply explicit destination directories (tilde-expanded at use site).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct SyncTargetConfig {
    /// Where to place synced files (user-level, project-local, or explicit path).
    pub mode: SyncMode,
    /// Optional namespace prefix applied to synced file names.
    /// For Claude: filesystem dir uses `{prefix}-{name}`.
    /// For `OpenCode`: synced into a `{prefix}/` subdirectory.
    /// For Cursor: filename becomes `{prefix}-{name}`.
    pub prefix: Option<String>,
    /// Optional content-reference prefix. When set, `model_facing_name()` uses
    /// this literal string (including separator) instead of deriving from `prefix`.
    /// For example, `"tw:"` produces content references like `tw:skill-name`.
    pub content_prefix: Option<String>,
    /// Permit overwriting user-owned files at the destination.
    /// When false (default), sync errors on collision. Overridden by `--force`.
    pub overwrite: bool,
    /// Base directory for synced output when `mode = Path`.
    /// Subdirectories (`agents/`, `skills/`, `rules/`, `commands/`) are derived
    /// automatically from `FileKind::dir_name()`.
    pub dir: Option<String>,
}

/// Where synced files should be placed.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    /// Sync to user-level config dirs (`~/.claude/`, `~/.config/opencode/`, etc.)
    #[default]
    User,
    /// Sync to project-local config dirs (`.claude/`, `.cursor/`, etc.)
    Project,
    /// Sync to an explicit base directory specified by `SyncTargetConfig::dir`
    Path,
}

/// CLI flag overrides for sync target resolution.
#[derive(Clone, Debug, Default)]
pub struct SyncFlags {
    /// Allow overwriting user-owned files at sync destinations.
    pub force: bool,
    /// Override destination root (implies `mode = Path`).
    pub dest: Option<String>,
    /// Override sync mode.
    pub mode: Option<SyncMode>,
    /// Override `prefix` setting.
    pub prefix: Option<String>,
    /// Override `content-prefix` setting.
    pub content_prefix: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use agentspec::presets::{ClaudePreset, CursorPreset, OpenCodePreset};

    use super::*;

    #[test]
    fn test_default_config() {
        let config = AgentspecConfig::default();
        assert_eq!(config.spec.sources_dir, PathBuf::from("spec"));
        assert_eq!(config.compile.output_dir, PathBuf::from("generated"));
        assert!(config.presets.is_empty());
        assert!(config.sync.is_empty());
    }

    #[test]
    fn test_discover_with_toml() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[spec]
sources_dir = "my/specs"

[compile]
output_dir = "out"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");
        assert_eq!(config.spec.sources_dir, PathBuf::from("my/specs"));
        assert_eq!(config.compile.output_dir, PathBuf::from("out"));
        assert_eq!(config.root_dir, tmp.path());
    }

    #[test]
    fn test_discover_without_toml() {
        let tmp = tempfile::tempdir().expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");
        assert_eq!(config.spec.sources_dir, PathBuf::from("spec"));
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
        let cli = SyncFlags::default();
        let result = config.resolve_sync_target(Provider::Claude, &cli);
        assert_eq!(result.mode, SyncMode::User);
        assert!(result.prefix.is_none());
        assert!(!result.overwrite);
        assert!(result.dir.is_none());
    }

    #[test]
    fn test_resolve_sync_target_applies_base_config() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[sync.claude]
prefix = "tw"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");
        let cli = SyncFlags::default();

        let result = config.resolve_sync_target(Provider::Claude, &cli);
        assert_eq!(result.prefix.as_deref(), Some("tw"));
        assert!(!result.overwrite);
        assert_eq!(result.mode, SyncMode::User); // default
    }

    #[test]
    fn test_resolve_sync_target_cli_overrides_win() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[sync.claude]
mode = "user"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");

        let cli = SyncFlags {
            mode: Some(SyncMode::Project),
            dest: None,
            force: false,
            ..Default::default()
        };

        let result = config.resolve_sync_target(Provider::Claude, &cli);
        assert_eq!(result.mode, SyncMode::Project);
    }

    #[test]
    fn test_resolve_sync_target_cli_dest_implies_path_mode() {
        let config = AgentspecConfig::default();
        let cli = SyncFlags {
            mode: None,
            dest: Some("/tmp/sync-test".to_string()),
            force: false,
            ..Default::default()
        };

        let result = config.resolve_sync_target(Provider::Claude, &cli);
        assert_eq!(result.mode, SyncMode::Path);
        assert_eq!(result.dir.as_deref(), Some("/tmp/sync-test"));
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

        let result = config.resolve_sync_target(Provider::Claude, &SyncFlags::default());
        assert!(result.prefix.is_none());
    }

    #[test]
    fn test_resolve_sync_target_cli_force_sets_overwrite() {
        let config = AgentspecConfig::default();
        let cli = SyncFlags {
            force: true,
            ..SyncFlags::default()
        };

        let result = config.resolve_sync_target(Provider::Claude, &cli);
        assert!(result.overwrite);
    }

    #[test]
    fn test_resolve_sync_target_cli_prefix_overrides_base() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[sync.claude]
prefix = "base"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");

        let cli = SyncFlags {
            prefix: Some("cli-override".to_string()),
            ..SyncFlags::default()
        };

        let result = config.resolve_sync_target(Provider::Claude, &cli);
        assert_eq!(result.prefix.as_deref(), Some("cli-override"));
    }

    #[test]
    fn test_resolve_sync_target_cli_prefix_sets_when_no_base() {
        let config = AgentspecConfig::default();
        let cli = SyncFlags {
            prefix: Some("from-cli".to_string()),
            ..SyncFlags::default()
        };

        let result = config.resolve_sync_target(Provider::Claude, &cli);
        assert_eq!(result.prefix.as_deref(), Some("from-cli"));
    }

    #[test]
    fn test_resolve_sync_target_cli_empty_prefix_normalized_to_none() {
        let config = AgentspecConfig::default();
        let cli = SyncFlags {
            prefix: Some(String::new()),
            ..SyncFlags::default()
        };

        let result = config.resolve_sync_target(Provider::Claude, &cli);
        assert!(result.prefix.is_none());
    }

    // -----------------------------------------------------------------------
    // explicit sync intent tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_has_explicit_sync_config_detects_base() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[sync.claude]
mode = "user"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");

        assert!(config.has_sync_config(Provider::Claude));
        assert!(!config.has_sync_config(Provider::Cursor));
    }

    #[test]
    fn test_cli_sync_intent_requires_explicit_target_selection() {
        let cli = SyncFlags {
            mode: Some(SyncMode::User),
            ..SyncFlags::default()
        };

        assert!(!AgentspecConfig::cli_sync_intent_sufficient(&cli, false));
    }

    #[test]
    fn test_cli_sync_intent_dest_with_target_is_sufficient() {
        let cli = SyncFlags {
            dest: Some("/tmp/out".to_string()),
            ..SyncFlags::default()
        };

        assert!(AgentspecConfig::cli_sync_intent_sufficient(&cli, true));
    }

    #[test]
    fn test_cli_sync_intent_mode_user_with_target_is_sufficient() {
        let cli = SyncFlags {
            mode: Some(SyncMode::User),
            ..SyncFlags::default()
        };

        assert!(AgentspecConfig::cli_sync_intent_sufficient(&cli, true));
    }

    #[test]
    fn test_cli_sync_intent_mode_project_with_target_is_sufficient() {
        let cli = SyncFlags {
            mode: Some(SyncMode::Project),
            ..SyncFlags::default()
        };

        assert!(AgentspecConfig::cli_sync_intent_sufficient(&cli, true));
    }

    #[test]
    fn test_cli_sync_intent_mode_path_without_dest_is_insufficient() {
        let cli = SyncFlags {
            mode: Some(SyncMode::Path),
            ..SyncFlags::default()
        };

        assert!(!AgentspecConfig::cli_sync_intent_sufficient(&cli, true));
    }

    #[test]
    fn test_validated_sync_target_succeeds_with_cli_only() {
        let tmp = tempfile::tempdir().expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");
        let cli = SyncFlags {
            mode: Some(SyncMode::Project),
            ..SyncFlags::default()
        };

        // No agentspec.toml config for cursor, but explicit --provider + --mode is sufficient.
        let target = config
            .validated_sync_target(Provider::Cursor, &cli, true)
            .expect("cli-only sync should be allowed with explicit provider and mode");
        assert_eq!(target.mode, SyncMode::Project);
    }

    #[test]
    fn test_validated_sync_target_errors_without_config_or_cli() {
        let tmp = tempfile::tempdir().expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");
        let cli = SyncFlags::default();

        // No agentspec.toml config and no useful CLI flags — should fail.
        let result = config.validated_sync_target(Provider::Cursor, &cli, false);
        assert!(
            result.is_err(),
            "should error when provider has no config and CLI is insufficient"
        );
    }

    // -----------------------------------------------------------------------
    // content_prefix tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_sync_target_content_prefix_from_config() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[sync.claude]
content-prefix = "tw:"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");
        let result = config.resolve_sync_target(Provider::Claude, &SyncFlags::default());
        assert_eq!(result.content_prefix.as_deref(), Some("tw:"));
    }

    #[test]
    fn test_resolve_sync_target_cli_content_prefix_overrides_config() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[sync.claude]
content-prefix = "original:"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");

        let cli = SyncFlags {
            content_prefix: Some("cli:".to_string()),
            ..SyncFlags::default()
        };

        let result = config.resolve_sync_target(Provider::Claude, &cli);
        assert_eq!(result.content_prefix.as_deref(), Some("cli:"));
    }

    #[test]
    fn test_resolve_sync_target_empty_content_prefix_normalized_to_none() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[sync.claude]
content-prefix = ""
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");

        let result = config.resolve_sync_target(Provider::Claude, &SyncFlags::default());
        assert!(result.content_prefix.is_none());
    }

    #[test]
    fn test_resolve_sync_target_content_prefix_defaults_to_none() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[sync.claude]
prefix = "tw"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");

        let result = config.resolve_sync_target(Provider::Claude, &SyncFlags::default());
        assert!(result.content_prefix.is_none());
        assert_eq!(result.prefix.as_deref(), Some("tw"));
    }

    // -----------------------------------------------------------------------
    // [spec].ignore tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_spec_ignore_round_trips_from_toml() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[spec]
sources_dir = "spec"
ignore = ["**/*.bats", "**/.DS_Store"]
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");
        assert_eq!(
            config.spec.ignore,
            vec!["**/*.bats".to_string(), "**/.DS_Store".to_string()],
        );
    }

    #[test]
    fn test_spec_ignore_defaults_to_empty() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r#"
[spec]
sources_dir = "spec"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let config = AgentspecConfig::discover(tmp.path()).expect("expected value");
        assert!(config.spec.ignore.is_empty());
    }

    #[test]
    fn test_spec_ignore_rejects_non_string_element() {
        let tmp = tempfile::tempdir().expect("expected value");
        let toml_content = r"
[spec]
ignore = [42]
";
        fs::write(tmp.path().join("agentspec.toml"), toml_content).expect("expected value");
        let err = AgentspecConfig::discover(tmp.path()).expect_err("expected parse error");
        let full = format!("{err:#}");
        assert!(full.contains("failed to parse"), "error: {full}");
        assert!(full.contains("spec.ignore"), "error: {full}");
    }

    #[test]
    fn test_spec_compile_ignore_matcher_propagates_pattern_error() {
        let spec = SpecConfig {
            sources_dir: PathBuf::from("spec"),
            ignore: vec!["[".to_string()],
        };
        let err = spec
            .compile_ignore_matcher()
            .expect_err("expected pattern parse error");
        let full = format!("{err:#}");
        assert!(full.contains("invalid ignore pattern"), "error: {full}");
        assert!(full.contains("'['"), "error: {full}");
    }

    #[test]
    fn test_spec_compile_ignore_matcher_empty_by_default() {
        let spec = SpecConfig::default();
        let matcher = spec
            .compile_ignore_matcher()
            .expect("empty patterns should compile");
        assert!(matcher.is_empty());
    }
}
