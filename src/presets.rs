use std::collections::HashMap;

use anyhow::{Result, bail};
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
    /// A bare model id. Cursor's own syntax permits bracket options here, but
    /// `validate` rejects them: agentspec composes the bracket from the named
    /// fields below, so two spellings of one option cannot coexist.
    pub model: Option<String>,
    /// One named field per model option Cursor documents. The adapter composes
    /// them into `model[k=v,k=v]`; authors never write brackets themselves.
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
}

/// The characters Cursor's `model[k=v,k=v]` grammar uses as delimiters.
///
/// None may appear in a `model` id or in an option value, because agentspec
/// composes the bracket by concatenation and Cursor documents no escaping
/// syntax to compose against. Without this, a single field forges a second
/// option — `effort = "high,context=1m"` emits `model[effort=high,context=1m]`
/// — which is exactly the "two spellings of one option cannot coexist"
/// guarantee the bracket ban exists to provide.
const CURSOR_BRACKET_DELIMITERS: [char; 4] = ['[', ']', ',', '='];

impl CursorPreset {
    /// Cross-field checks, run from `Specs::validate` so every command that
    /// loads specs surfaces them — and so a library consumer calling
    /// `compile_specs` is gated too, not only the CLI.
    pub fn validate(&self, preset_name: &str) -> Result<()> {
        if let Some(model) = self.model.as_deref()
            && model.contains(CURSOR_BRACKET_DELIMITERS)
        {
            bail!(
                "[presets.{preset_name}.cursor] `model` must be a bare model id \
                 with no `[`, `]`, `,`, or `=`; set the `effort`, `fast`, or \
                 `context` field instead of Cursor's `[k=v]` syntax"
            );
        }
        for (field, value) in [
            ("effort", self.effort.as_deref()),
            ("context", self.context.as_deref()),
        ] {
            if value.is_some_and(|v| v.contains(CURSOR_BRACKET_DELIMITERS)) {
                bail!(
                    "[presets.{preset_name}.cursor] `{field}` must not contain \
                     `[`, `]`, `,`, or `=` — agentspec composes Cursor's bracket \
                     syntax from these fields, and a delimiter here would forge \
                     an option the preset did not declare"
                );
            }
        }
        if self.model.is_none() && self.any_option_set() {
            bail!(
                "[presets.{preset_name}.cursor] model options require `model` \
                 (Cursor encodes them as bracket options: `model[effort=high]`)"
            );
        }
        Ok(())
    }

    /// True when any bracket option is configured.
    ///
    /// The destructuring binding is load-bearing: adding a fourth option to
    /// `CursorPreset` fails to compile here until it is accounted for. A plain
    /// `self.effort.is_some() || …` chain would compile against the new field
    /// and silently under-report, which is the failure this function exists to
    /// prevent — an option set with no `model` would then pass validation and
    /// be dropped at composition time with nothing said.
    fn any_option_set(&self) -> bool {
        let Self {
            model: _,
            effort,
            fast,
            context,
        } = self;
        effort.is_some() || fast.is_some() || context.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor(model: Option<&str>) -> CursorPreset {
        CursorPreset {
            model: model.map(str::to_string),
            ..CursorPreset::default()
        }
    }

    #[test]
    fn test_cursor_validate_rejects_bracketed_model() {
        let preset = cursor(Some("claude-opus-5[effort=high]"));
        let err = preset.validate("x").expect_err("expected rejection");
        let msg = err.to_string();
        assert!(msg.contains("presets.x.cursor"), "error: {msg}");
        assert!(msg.contains("bare model id"), "error: {msg}");
    }

    /// Every option arm exercised separately, so `any_option_set` cannot pass
    /// by covering only the first field.
    #[test]
    fn test_cursor_validate_rejects_each_option_without_model() {
        let cases: [(&str, CursorPreset); 3] = [
            (
                "effort",
                CursorPreset {
                    effort: Some("high".to_string()),
                    ..CursorPreset::default()
                },
            ),
            (
                "fast",
                CursorPreset {
                    fast: Some(true),
                    ..CursorPreset::default()
                },
            ),
            (
                "context",
                CursorPreset {
                    context: Some("300k".to_string()),
                    ..CursorPreset::default()
                },
            ),
        ];

        for (field, preset) in cases {
            let Err(err) = preset.validate("x") else {
                panic!("{field} alone should be rejected");
            };
            let msg = err.to_string();
            assert!(msg.contains("presets.x.cursor"), "{field}: {msg}");
            assert!(msg.contains("require `model`"), "{field}: {msg}");
        }
    }

    #[test]
    fn test_cursor_validate_accepts_bare_model() {
        cursor(Some("claude-opus-5"))
            .validate("x")
            .expect("bare model should validate");
    }

    #[test]
    fn test_cursor_validate_accepts_model_with_all_options() {
        let preset = CursorPreset {
            model: Some("claude-opus-5".to_string()),
            effort: Some("high".to_string()),
            fast: Some(false),
            context: Some("300k".to_string()),
        };
        preset
            .validate("x")
            .expect("model plus all options should validate");
    }

    /// A preset configuring nothing at all is inert, not invalid.
    #[test]
    fn test_cursor_validate_accepts_empty() {
        CursorPreset::default()
            .validate("x")
            .expect("empty preset should validate");
    }
}
