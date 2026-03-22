use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::cli::CommonArgs;
use crate::types::{ProfilesMap, Provider};

/// Top-level config parsed from `agentspec.toml`.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct AgentspecConfig {
    pub spec: SpecConfig,
    pub output: OutputConfig,
    pub targets: Vec<Provider>,

    /// Model profiles: profile name → per-provider model config.
    ///
    /// Each provider value is either a string shorthand (e.g., `"opus"`) or an
    /// object with `model`, `variant`, and/or `reasoning_effort` fields.
    #[serde(default)]
    pub profiles: HashMap<String, HashMap<String, serde_json::Value>>,

    /// Per-machine profile overrides, keyed by overlay name (e.g., `"home"`, `"work"`).
    ///
    /// When `AGENTSPEC_PROFILE=home`, the `home` overlay merges over `profiles` at the
    /// provider level within each named profile.
    #[serde(default)]
    pub profile_overrides: HashMap<String, HashMap<String, HashMap<String, serde_json::Value>>>,

    /// Root directory where agentspec.toml was found (not serialized).
    #[serde(skip)]
    pub root_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct SpecConfig {
    pub agents_dir: PathBuf,
    pub skills_dir: PathBuf,
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
            profiles: HashMap::new(),
            profile_overrides: HashMap::new(),
            root_dir: PathBuf::new(),
        }
    }
}

impl Default for SpecConfig {
    fn default() -> Self {
        Self {
            agents_dir: PathBuf::from("spec/agents"),
            skills_dir: PathBuf::from("spec/skills"),
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

    /// Resolve model profiles, applying the named machine overlay if provided.
    ///
    /// When `active_profile` is `Some("home")`, the `[profile_overrides.home.*]` entries
    /// are merged over the base `[profiles.*]` entries at the provider level.
    pub fn resolve_profiles(&self, active_profile: Option<&str>) -> ProfilesMap {
        let mut resolved = self.profiles.clone();

        if let Some(overlay_name) = active_profile {
            if let Some(overrides) = self.profile_overrides.get(overlay_name) {
                for (profile_key, provider_overrides) in overrides {
                    let entry = resolved.entry(profile_key.clone()).or_default();
                    for (provider, value) in provider_overrides {
                        entry.insert(provider.clone(), value.clone());
                    }
                }
            }
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
        assert!(config.profiles.is_empty());
        assert!(config.profile_overrides.is_empty());
    }

    #[test]
    fn test_discover_with_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let toml_content = r#"
[spec]
agents_dir = "my/agents"

[output]
dir = "out"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).unwrap();
        let config = AgentspecConfig::discover(tmp.path()).unwrap();
        assert_eq!(config.spec.agents_dir, PathBuf::from("my/agents"));
        assert_eq!(config.output.dir, PathBuf::from("out"));
        // skills_dir should use default
        assert_eq!(config.spec.skills_dir, PathBuf::from("spec/skills"));
        assert_eq!(config.root_dir, tmp.path());
    }

    #[test]
    fn test_discover_without_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let config = AgentspecConfig::discover(tmp.path()).unwrap();
        assert_eq!(config.spec.agents_dir, PathBuf::from("spec/agents"));
        assert_eq!(config.root_dir, tmp.path());
    }

    #[test]
    fn test_discover_with_profiles() {
        let tmp = tempfile::tempdir().unwrap();
        let toml_content = r#"
[profiles.deep_review]
claude = "opus"

[profiles.balanced]
claude = "sonnet"
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).unwrap();
        let config = AgentspecConfig::discover(tmp.path()).unwrap();
        assert_eq!(config.profiles.len(), 2);
        assert!(config.profiles.contains_key("deep_review"));
        assert!(config.profiles.contains_key("balanced"));

        let deep = &config.profiles["deep_review"];
        assert_eq!(deep["claude"], serde_json::json!("opus"));

        let balanced = &config.profiles["balanced"];
        assert_eq!(
            balanced["opencode"],
            serde_json::json!({"model": "anthropic/claude-sonnet-4-5", "variant": "high"})
        );
    }

    #[test]
    fn test_resolve_profiles_no_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        let toml_content = r#"
[profiles.balanced]
claude = "sonnet"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).unwrap();
        let config = AgentspecConfig::discover(tmp.path()).unwrap();

        let resolved = config.resolve_profiles(None);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved["balanced"]["claude"], serde_json::json!("sonnet"));
    }

    #[test]
    fn test_resolve_profiles_with_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        let toml_content = r#"
[profiles.balanced]
claude = "sonnet"
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }

[profile_overrides.home.balanced]
opencode = { model = "openai/gpt-5.3-codex", variant = "medium" }
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).unwrap();
        let config = AgentspecConfig::discover(tmp.path()).unwrap();

        // Without overlay: uses base
        let base = config.resolve_profiles(None);
        assert_eq!(
            base["balanced"]["opencode"],
            serde_json::json!({"model": "anthropic/claude-sonnet-4-5", "variant": "high"})
        );

        // With home overlay: opencode is overridden
        let home = config.resolve_profiles(Some("home"));
        assert_eq!(
            home["balanced"]["opencode"],
            serde_json::json!({"model": "openai/gpt-5.3-codex", "variant": "medium"})
        );
        // claude is unchanged
        assert_eq!(home["balanced"]["claude"], serde_json::json!("sonnet"));
    }

    #[test]
    fn test_resolve_profiles_unknown_overlay_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let toml_content = r#"
[profiles.balanced]
claude = "sonnet"
"#;
        fs::write(tmp.path().join("agentspec.toml"), toml_content).unwrap();
        let config = AgentspecConfig::discover(tmp.path()).unwrap();

        let resolved = config.resolve_profiles(Some("nonexistent"));
        assert_eq!(resolved["balanced"]["claude"], serde_json::json!("sonnet"));
    }
}
