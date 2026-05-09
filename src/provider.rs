use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::adapters::{Adapter, ClaudeAdapter, CursorAdapter, HookAdapter, OpenCodeAdapter};

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

    /// Returns the provider's hook adapter, if it emits hooks.
    ///
    /// `Some(_)` for Claude and Cursor; `None` for `OpenCode`. Call sites
    /// thread the `None` through via `Option::map` rather than branching on
    /// the provider — `provider.hook_adapter().is_none()` is the canonical
    /// "does this provider emit hooks?" check.
    ///
    /// Bridge-phase carry-over: this method survives sub-step C so the
    /// `compile_specs` orchestrator can emit the `SkippedHook` diagnostic
    /// without branching on `Provider`. Sub-step D replaces the predicate
    /// with an `Adapter`-side capability accessor and deletes this method.
    pub fn hook_adapter(self) -> Option<&'static dyn HookAdapter> {
        match self {
            Self::Claude => Some(&ClaudeAdapter),
            Self::Cursor => Some(&CursorAdapter),
            Self::OpenCode => None,
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
    fn test_hook_adapter_present_for_claude_and_cursor() {
        assert!(Provider::Claude.hook_adapter().is_some());
        assert!(Provider::Cursor.hook_adapter().is_some());
    }

    #[test]
    fn test_hook_adapter_absent_for_opencode() {
        assert!(Provider::OpenCode.hook_adapter().is_none());
    }

    #[test]
    fn test_adapter_returns_dyn_adapter() {
        // Compile-time check that `Provider::adapter()` returns a value
        // satisfying the `Adapter` trait. Replaces the legacy
        // `test_adapter_file_kinds_*` test that exercised the now-deleted
        // `ProviderAdapter::file_kinds` method.
        let _: &dyn Adapter = Provider::Claude.adapter();
        let _: &dyn Adapter = Provider::Cursor.adapter();
        let _: &dyn Adapter = Provider::OpenCode.adapter();
    }
}
