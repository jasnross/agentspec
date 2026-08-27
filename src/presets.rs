use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Resolved model presets: preset name → per-provider model config.
pub type ProviderPresetsMap = HashMap<String, ProviderPresets>;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderPresets {
    pub claude: Option<ClaudePreset>,
    pub cursor: Option<CursorPreset>,
    pub opencode: Option<OpenCodePreset>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ClaudePreset {
    pub model: Option<String>,
    /// Claude's `effort` is independent of `model`: both
    /// `experiments/claude-agent-effort/` and `experiments/claude-skill-effort/`
    /// measured it valid at `outbound-request` depth with no `model` key at all.
    /// Unlike Cursor's options, which Cursor encodes as a suffix on the model id
    /// and so cannot exist without one, this needs no cross-field check.
    pub effort: Option<ClaudeEffort>,
}

/// Claude's reasoning-effort vocabulary — a documented, closed set, so a typo
/// fails at config parse rather than reaching frontmatter. Derives `Serialize`
/// as well as `Deserialize` because both Claude frontmatter structs carry the
/// value straight through.
///
/// The asymmetry with `CursorPreset::effort`, which is a plain `String`, is
/// deliberate: Cursor documents its legal values as varying by model and
/// discoverable only at runtime, so there is no static set to encode there.
/// Do not unify these.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ClaudeEffort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct OpenCodePreset {
    pub model: Option<String>,
    pub variant: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CursorPreset {
    pub model: Option<String>,
}
