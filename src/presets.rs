use std::collections::HashMap;

use serde::Deserialize;

/// Resolved model presets: preset name → per-provider model config.
pub type ProviderPresetsMap = HashMap<String, ProviderPresets>;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProviderPresets {
    pub claude: Option<ClaudePreset>,
    pub codex: Option<CodexPreset>,
    pub cursor: Option<CursorPreset>,
    pub opencode: Option<OpenCodePreset>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ClaudePreset {
    pub model: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct OpenCodePreset {
    pub model: Option<String>,
    pub variant: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CodexPreset {
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CursorPreset {
    pub model: Option<String>,
}
