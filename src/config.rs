use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::cli::CommonArgs;
use crate::types::Provider;

/// Top-level config parsed from `agentspec.toml`.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct AgentspecConfig {
    pub spec: SpecConfig,
    pub mappings: MappingsConfig,
    pub output: OutputConfig,
    pub targets: Vec<Provider>,

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
pub struct MappingsConfig {
    pub models: PathBuf,
    pub tools: PathBuf,
    pub features: PathBuf,
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
            mappings: MappingsConfig::default(),
            output: OutputConfig::default(),
            targets: Provider::ALL.to_vec(),
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

impl Default for MappingsConfig {
    fn default() -> Self {
        Self {
            models: PathBuf::from("mappings/models.yaml"),
            tools: PathBuf::from("mappings/tools.yaml"),
            features: PathBuf::from("mappings/features.yaml"),
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
}
