use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::adapters::{Adapter, ClaudeAdapter, CursorAdapter, OpenCodeAdapter};

#[derive(
    Clone,
    Copy,
    Debug,
    Deserialize,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
    ValueEnum,
    strum::Display,
    strum::VariantArray,
)]
#[clap(rename_all = "lower")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum Provider {
    Claude,
    Cursor,
    OpenCode,
}

impl Provider {
    /// Human-readable name for CLI output (e.g. `"Claude"`, `"OpenCode"`).
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Cursor => "Cursor",
            Self::OpenCode => "OpenCode",
        }
    }

    /// Returns the provider's adapter as a trait object.
    ///
    /// This is the dispatch root for every provider-specific decision in the
    /// codebase. Non-adapter modules MUST go through this method rather than
    /// naming a specific adapter directly.
    pub fn adapter(self) -> &'static dyn Adapter {
        match self {
            Self::Claude => &ClaudeAdapter,
            Self::Cursor => &CursorAdapter,
            Self::OpenCode => &OpenCodeAdapter,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_name_title_case() {
        assert_eq!(Provider::Claude.display_name(), "Claude");
        assert_eq!(Provider::Cursor.display_name(), "Cursor");
        assert_eq!(Provider::OpenCode.display_name(), "OpenCode");
    }

    #[test]
    fn test_emits_hooks_capability() {
        // Claude / Cursor produce hook entries; OpenCode does not. The
        // `compile_specs` orchestrator consults this to push `SkippedHook`
        // diagnostics for hook specs the active provider can't emit.
        assert!(Provider::Claude.adapter().emits_hooks());
        assert!(Provider::Cursor.adapter().emits_hooks());
        assert!(!Provider::OpenCode.adapter().emits_hooks());
    }

    #[test]
    fn test_adapter_returns_dyn_adapter() {
        // Compile-time check that `Provider::adapter()` returns a value
        // satisfying the `Adapter` trait — the single dispatch point for all
        // provider-specific decisions.
        let _: &dyn Adapter = Provider::Claude.adapter();
        let _: &dyn Adapter = Provider::Cursor.adapter();
        let _: &dyn Adapter = Provider::OpenCode.adapter();
    }

    #[test]
    fn test_supports_path_scoped_rules_capability() {
        assert!(Provider::Claude.adapter().supports_path_scoped_rules());
        assert!(Provider::Cursor.adapter().supports_path_scoped_rules());
        assert!(!Provider::OpenCode.adapter().supports_path_scoped_rules());
    }
}
