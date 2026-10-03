use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::provider::Provider;
use crate::setting::SettingKey;

/// Resolved model presets: preset name → per-provider model config.
pub type ProviderPresetsMap = HashMap<String, ProviderPresets>;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderPresets {
    pub claude: Option<ClaudePreset>,
    pub cursor: Option<CursorPreset>,
    pub opencode: Option<OpenCodePreset>,
}

impl ProviderPresets {
    /// The settings this preset configures for `provider`.
    ///
    /// Lives here rather than in `compile.rs` because `ProviderPresets`
    /// exposes three differently-shaped typed fields with no way to ask about
    /// a provider by value — computing this from the orchestrator would mean a
    /// `match provider` outside `src/adapters/`, which
    /// `.claude/rules/provider-logic-in-adapters.md` calls a smell. The
    /// fan-out belongs with the config types whose shape it is checking; each
    /// provider block's grammar, by contrast, is checked in its adapter's
    /// `Adapter::validate_declarations`.
    ///
    /// Says nothing about whether the provider can carry what it names: the
    /// loss subtraction raises an intent for every configured setting and
    /// learns what was carried from the deliveries instead. Consulting a
    /// capability table here would make a total drop unreportable, because a
    /// setting is carriable on no emitted kind precisely when the provider
    /// emitted nothing.
    pub fn configured(&self, provider: Provider) -> Vec<SettingKey> {
        let Self {
            claude,
            cursor,
            opencode,
        } = self;
        match provider {
            Provider::Claude => claude
                .as_ref()
                .map(ClaudePreset::configured)
                .unwrap_or_default(),
            Provider::Cursor => cursor
                .as_ref()
                .map(CursorPreset::configured)
                .unwrap_or_default(),
            Provider::OpenCode => opencode
                .as_ref()
                .map(OpenCodePreset::configured)
                .unwrap_or_default(),
        }
    }
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

impl ClaudePreset {
    /// Opens with a destructuring binding so a new field is a compile error
    /// here rather than a setting silently absent from the intent set.
    fn configured(&self) -> Vec<SettingKey> {
        let Self { model, effort } = self;
        [
            model.as_ref().map(|_| SettingKey::Model),
            effort.as_ref().map(|_| SettingKey::Effort),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
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
///
/// The cost of the closed set is forward compatibility: when Claude adds a
/// level, a config Claude itself accepts is a hard parse error here until a
/// release ships. That is the accepted trade for catching a typo at parse time,
/// because Claude clamps an unrecognized level silently rather than reporting
/// it — so the failure this prevents is one the user would never be told about.
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

impl OpenCodePreset {
    /// Opens with a destructuring binding so a new field is a compile error
    /// here rather than a setting silently absent from the intent set.
    fn configured(&self) -> Vec<SettingKey> {
        let Self { model, variant } = self;
        [
            model.as_ref().map(|_| SettingKey::Model),
            variant.as_ref().map(|_| SettingKey::Variant),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CursorPreset {
    /// A bare model id. Cursor's own syntax permits bracket options here, but
    /// the Cursor adapter rejects them: agentspec composes the bracket from
    /// the named fields below, so two spellings of one option cannot coexist.
    pub model: Option<String>,
    /// A named field per model option agentspec types and documents. The
    /// adapter composes these into `model[k=v,k=v]`; authors never write
    /// brackets themselves. This is not the whole option set Cursor accepts —
    /// see `params` below for the rest.
    ///
    /// `effort` stays an untyped string because Cursor documents its legal
    /// values as varying by model and discoverable only at runtime. The
    /// asymmetry with `ClaudeEffort` is deliberate — do not "fix" it into an
    /// enum. `fast` and `context` take the types Cursor documents; a
    /// `BTreeMap<String, String>` was rejected for erasing exactly that, since
    /// it would force `fast = "false"`.
    pub effort: Option<String>,
    pub fast: Option<bool>,
    /// Composed like the other two, but no Cursor oracle can observe it take
    /// effect — the flattened `subagent_model` hides it either way. What is
    /// measured, by `experiments/cursor-subagent-bracket-tolerance/`, is that a
    /// bracket carrying it still applies the options beside it. Do not drop the
    /// field over the asymmetry: the effect was never agentspec's to warrant.
    pub context: Option<String>,
    /// Escape hatch for bracket options agentspec has no named field for.
    ///
    /// The three fields above are not the whole set and cannot be. Cursor
    /// documents bracket options as using "the same `id=value` pairs as the
    /// SDK's model parameters", states that "parameter ids and values vary by
    /// model", and makes the catalog account- and team-specific, discoverable
    /// only through `Cursor.models.list()`. `optimize_for` (`cost` | `balanced`
    /// | `intelligence`, on Router models) is a documented example with no field
    /// here.
    ///
    /// Without this, the ban on hand-written brackets would *delete* those
    /// options rather than relocate them — the one resolution the design ruled
    /// out. Named fields stay for the options agentspec can type and document;
    /// this carries the rest, under the same delimiter and whitespace rules.
    ///
    /// A key that duplicates a named field is rejected: two spellings of one
    /// option cannot coexist, which is the guarantee the ban exists to provide.
    ///
    /// `BTreeMap` rather than `HashMap` so emission order is deterministic
    /// without a separate sort at the composition site.
    pub params: BTreeMap<String, String>,
}

impl CursorPreset {
    /// Opens with a destructuring binding so a new field is a compile error
    /// here rather than a setting silently absent from the intent set — the
    /// same guard the bracket composition site carries, for the same reason.
    ///
    /// Every `params` key becomes its own [`SettingKey::Param`], so an option
    /// agentspec has no named field for still raises an intent and is still
    /// reportable when it reaches no file.
    fn configured(&self) -> Vec<SettingKey> {
        let Self {
            model,
            effort,
            fast,
            context,
            params,
        } = self;
        [
            model.as_ref().map(|_| SettingKey::Model),
            effort.as_ref().map(|_| SettingKey::Effort),
            fast.as_ref().map(|_| SettingKey::Fast),
            context.as_ref().map(|_| SettingKey::Context),
        ]
        .into_iter()
        .flatten()
        .chain(params.keys().map(|k| SettingKey::Param(k.clone())))
        .collect()
    }
}

#[cfg(test)]
mod configured_tests {
    use std::collections::BTreeMap;

    use super::{ClaudeEffort, ClaudePreset, CursorPreset, OpenCodePreset, ProviderPresets};
    use crate::provider::Provider;
    use crate::setting::SettingKey;

    #[test]
    fn test_claude_configured_names_both_fields() {
        let presets = ProviderPresets {
            claude: Some(ClaudePreset {
                model: Some("claude-opus-5".to_owned()),
                effort: Some(ClaudeEffort::High),
            }),
            ..ProviderPresets::default()
        };
        assert_eq!(
            presets.configured(Provider::Claude),
            vec![SettingKey::Model, SettingKey::Effort]
        );
    }

    #[test]
    fn test_cursor_configured_names_every_param_key() {
        let presets = ProviderPresets {
            cursor: Some(CursorPreset {
                model: Some("claude-opus-5".to_owned()),
                effort: Some("high".to_owned()),
                fast: None,
                context: None,
                params: BTreeMap::from([
                    ("optimize_for".to_owned(), "cost".to_owned()),
                    ("a_first_by_key".to_owned(), "1".to_owned()),
                ]),
            }),
            ..ProviderPresets::default()
        };
        assert_eq!(
            presets.configured(Provider::Cursor),
            vec![
                SettingKey::Model,
                SettingKey::Effort,
                SettingKey::Param("a_first_by_key".to_owned()),
                SettingKey::Param("optimize_for".to_owned()),
            ]
        );
    }

    #[test]
    fn test_opencode_configured_names_model_and_variant() {
        let presets = ProviderPresets {
            opencode: Some(OpenCodePreset {
                model: Some("anthropic/claude-opus-5".to_owned()),
                variant: Some("thinking".to_owned()),
            }),
            ..ProviderPresets::default()
        };
        assert_eq!(
            presets.configured(Provider::OpenCode),
            vec![SettingKey::Model, SettingKey::Variant]
        );
    }

    #[test]
    fn test_unconfigured_provider_names_nothing() {
        // A preset that configures one provider raises no intent for the
        // others, so a spec naming it loses nothing on them.
        let presets = ProviderPresets {
            claude: Some(ClaudePreset {
                model: Some("claude-opus-5".to_owned()),
                effort: None,
            }),
            ..ProviderPresets::default()
        };
        assert!(presets.configured(Provider::Cursor).is_empty());
        assert!(presets.configured(Provider::OpenCode).is_empty());

        let empty = ProviderPresets::default();
        for provider in [Provider::Claude, Provider::Cursor, Provider::OpenCode] {
            assert!(empty.configured(provider).is_empty());
        }
    }
}
