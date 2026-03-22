use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::cli::CommonArgs;
use crate::types::{PresetsMap, Provider};

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
            self.targets = args.target.clone();
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
}

#[cfg(test)]
#[allow(clippy::expect_used)]
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
}
