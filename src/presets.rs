use std::collections::{BTreeMap, HashMap};

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

impl ProviderPresets {
    /// Run every configured provider block's cross-field checks.
    ///
    /// This does not make provider knowledge disappear — `presets.rs` is outside
    /// `adapters/` and this function names `cursor`. What it buys is narrower
    /// and still worth having: the rest of the pipeline (`validate.rs` and up)
    /// stops naming a provider, and the fan-out sits with the config types whose
    /// shape it is checking rather than in the semantic-validation pass.
    ///
    /// The destructuring binding makes a new provider block a compile error
    /// here, and each arm is independent — so a second provider's `validate`
    /// cannot be silently skipped for presets that do not configure the first,
    /// which the earlier `presets.get(name)?.cursor.as_ref()?` chain would have
    /// done.
    ///
    /// Adding a provider therefore still means a new `FooPreset` and a new arm
    /// here, not an adapter alone. Moving each block's grammar and validation
    /// into its own adapter would close that gap; it is tracked, not done.
    pub fn validate(&self, preset_name: &str) -> Result<()> {
        let Self {
            claude: _,
            cursor,
            opencode: _,
        } = self;
        // Claude and OpenCode have no cross-field constraints: Claude's `effort`
        // is independent of `model`, and an OpenCode `variant` with no `model`
        // is accepted and inert. Cursor's options cannot exist apart from the
        // model id they suffix, which is why only it validates.
        if let Some(cursor) = cursor {
            cursor.validate(preset_name)?;
        }
        Ok(())
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

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CursorPreset {
    /// A bare model id. Cursor's own syntax permits bracket options here, but
    /// `validate` rejects them: agentspec composes the bracket from the named
    /// fields below, so two spellings of one option cannot coexist.
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

/// The characters Cursor's `model[k=v,k=v]` grammar uses as delimiters.
///
/// None may appear in a `model` id or in an option value, because agentspec
/// composes the bracket by concatenation and Cursor documents no escaping
/// syntax to compose against. Without this, a single field forges a second
/// option — `effort = "high,context=1m"` emits `model[effort=high,context=1m]`
/// — which is exactly the "two spellings of one option cannot coexist"
/// guarantee the bracket ban exists to provide.
pub(crate) const CURSOR_BRACKET_DELIMITERS: [char; 4] = ['[', ']', ',', '='];

/// Option ids that have a named `CursorPreset` field. A `params` key matching
/// one of these — in any case — is rejected rather than merged, so an option can
/// only be spelled one way.
///
/// Hand-maintained, unlike the destructuring bindings that guard the validation
/// and composition sites. `test_named_cursor_options_are_real_fields` catches a
/// rename or removal by round-tripping each entry through `deny_unknown_fields`;
/// a field *added* without being listed here is not caught, and would let
/// `params` re-spell it into a duplicate option. See that test for why.
const NAMED_CURSOR_OPTIONS: [&str; 3] = ["effort", "fast", "context"];

/// `check_composable` plus the delimiter ban — the full set of rules a bracket
/// option id or value must satisfy.
fn check_bracket_safe(field: &str, value: &str, preset_name: &str) -> Result<()> {
    if value.contains(CURSOR_BRACKET_DELIMITERS) {
        bail!(
            "[presets.{preset_name}.cursor] `{field}` must not contain \
             `[`, `]`, `,`, or `=` — agentspec composes Cursor's bracket \
             syntax from these fields, and a delimiter here would forge \
             an option the preset did not declare"
        );
    }
    check_composable(field, value, preset_name)
}

/// Reject a `model` id or option value that cannot survive bracket composition.
///
/// Empty and whitespace-bearing values both compose something malformed, and
/// Cursor rejects nothing — so the result is silently discarded, plausibly
/// taking the well-formed options beside it down with the whole bracket:
///
/// - `model = ""` composes `[effort=high]`, a bracket with no model in front.
///   An empty `model` is still `Some`, so it satisfies the model-less-options
///   check without this.
/// - `effort = ""` composes `model[effort=]`.
/// - `model = " claude-opus-5 "` composes ` claude-opus-5 [effort=high]`, which
///   serde then emits as a *quoted* scalar — changing the model id itself.
///
/// No Cursor model id or documented option value contains whitespace, so
/// rejecting it outright costs nothing and needs no trimming rule to explain.
fn check_composable(field: &str, value: &str, preset_name: &str) -> Result<()> {
    if value.is_empty() {
        bail!(
            "[presets.{preset_name}.cursor] `{field}` must not be empty; \
             omit the key entirely to leave it unset"
        );
    }
    if value.contains(char::is_whitespace) {
        bail!(
            "[presets.{preset_name}.cursor] `{field}` must not contain whitespace \
             (got {value:?}) — agentspec composes Cursor's bracket syntax by \
             concatenation, and Cursor silently discards a malformed bracket"
        );
    }
    Ok(())
}

impl CursorPreset {
    /// Cross-field checks, reached via `ProviderPresets::validate` from
    /// `Specs::validate`, so every command that loads specs surfaces them.
    /// `ValidatedSpecs` carries the map it was validated against and
    /// `compile::run` reads presets from there, so on that path the map
    /// reaching the Cursor adapter is one that passed here.
    ///
    /// A consumer calling `Provider::adapter().compile(...)` directly supplies
    /// its own presets and bypasses this; the adapter's `debug_assert!`s are
    /// all that stand there, and they compile out in release.
    pub fn validate(&self, preset_name: &str) -> Result<()> {
        if let Some(model) = self.model.as_deref() {
            if model.contains(CURSOR_BRACKET_DELIMITERS) {
                bail!(
                    "[presets.{preset_name}.cursor] `model` must be a bare model id \
                     with no `[`, `]`, `,`, or `=`; set the `effort`, `fast`, or \
                     `context` field instead of Cursor's `[k=v]` syntax"
                );
            }
            check_composable("model", model, preset_name)?;
        }
        for (field, value) in [
            ("effort", self.effort.as_deref()),
            ("context", self.context.as_deref()),
        ] {
            let Some(value) = value else { continue };
            check_bracket_safe(field, value, preset_name)?;
        }

        // `params` keys must not collide with each other either, on the same
        // reasoning as the named-field check below: `optimize_for` beside
        // `Optimize_For` is one option spelled twice whichever way Cursor folds
        // ids. `BTreeMap` orders by byte, so case variants are not adjacent —
        // this needs a set, not a neighbour compare.
        let mut folded: HashMap<String, &String> = HashMap::new();
        for key in self.params.keys() {
            if let Some(first) = folded.insert(key.to_ascii_lowercase(), key) {
                bail!(
                    "[presets.{preset_name}.cursor] `params` keys `{first}` and \
                     `{key}` differ only in case; Cursor's option ids are not \
                     known to be case-folded, so one of them would be a silently \
                     duplicated option"
                );
            }
        }

        for (key, value) in &self.params {
            // Case-insensitive: whether Cursor folds option-id case is
            // unmeasured, and both readings are bad. If it folds, an untyped
            // `params` entry silently overrides the typed field; if it does not,
            // the user gets an option they believe is set and Cursor ignores.
            // Either way `[effort=high,Effort=low]` is one option spelled twice.
            if let Some(named) = NAMED_CURSOR_OPTIONS
                .iter()
                .find(|n| n.eq_ignore_ascii_case(key))
            {
                bail!(
                    "[presets.{preset_name}.cursor] `params.{key}` duplicates the \
                     `{named}` field; set one or the other, not both — two \
                     spellings of one option cannot coexist"
                );
            }
            // Labelled separately so a malformed key is distinguishable from a
            // malformed value — an empty key would otherwise report as
            // `params.`, naming nothing.
            check_bracket_safe(&format!("params key {key:?}"), key, preset_name)?;
            check_bracket_safe(&format!("params.{key}"), value, preset_name)?;
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
            params,
        } = self;
        effort.is_some() || fast.is_some() || context.is_some() || !params.is_empty()
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

    /// An empty value composes a malformed bracket rather than being skipped:
    /// `model = ""` yields `[effort=high]` and `effort = ""` yields
    /// `model[effort=]`. Cursor rejects nothing, so both degrade silently.
    #[test]
    fn test_cursor_validate_rejects_empty_values() {
        let cases: [(&str, CursorPreset); 4] = [
            ("model", cursor(Some(""))),
            ("model", cursor(Some("   "))),
            (
                "effort",
                CursorPreset {
                    model: Some("claude-opus-5".to_string()),
                    effort: Some(String::new()),
                    ..CursorPreset::default()
                },
            ),
            (
                "context",
                CursorPreset {
                    model: Some("claude-opus-5".to_string()),
                    context: Some("  ".to_string()),
                    ..CursorPreset::default()
                },
            ),
        ];

        for (field, preset) in cases {
            let Err(err) = preset.validate("x") else {
                panic!("empty {field} should be rejected");
            };
            let msg = err.to_string();
            assert!(msg.contains("presets.x.cursor"), "{field}: {msg}");
            assert!(
                msg.contains("must not be empty") || msg.contains("must not contain whitespace"),
                "{field}: {msg}"
            );
        }
    }

    /// Whitespace anywhere in a value composes a malformed bracket, and a
    /// leading space additionally forces serde to emit a quoted scalar —
    /// changing the model id rather than only the option suffix.
    #[test]
    fn test_cursor_validate_rejects_whitespace_in_values() {
        let cases: [(&str, CursorPreset); 3] = [
            ("model", cursor(Some(" claude-opus-5 "))),
            (
                "effort",
                CursorPreset {
                    model: Some("claude-opus-5".to_string()),
                    effort: Some("high 5".to_string()),
                    ..CursorPreset::default()
                },
            ),
            (
                "context",
                CursorPreset {
                    model: Some("claude-opus-5".to_string()),
                    context: Some("300 k".to_string()),
                    ..CursorPreset::default()
                },
            ),
        ];

        for (field, preset) in cases {
            let Err(err) = preset.validate("x") else {
                panic!("whitespace in {field} should be rejected");
            };
            let msg = err.to_string();
            assert!(msg.contains("presets.x.cursor"), "{field}: {msg}");
            assert!(
                msg.contains("must not contain whitespace"),
                "{field}: {msg}"
            );
        }
    }

    /// Every `NAMED_CURSOR_OPTIONS` entry names a real `CursorPreset` field.
    ///
    /// Deserializing `<name> = 0` fails either way — the three fields are
    /// `String`, `bool`, `String` — but the *error* distinguishes the cases: a
    /// live field gives a type error, a renamed or removed one gives
    /// `unknown field` under `deny_unknown_fields`. So renaming `context` to
    /// `thinking` without updating the array fails here, which is the drift that
    /// would otherwise let `params.context` re-spell a named option.
    ///
    /// The destructuring binding below is the other half: adding a field is a
    /// compile error here, forcing whoever adds it to look at this test. What
    /// neither half catches is a fourth field added, bound as `_`, and left out
    /// of the array — Rust has no field-name reflection to close that without a
    /// macro, so it is a known limit rather than a covered case.
    #[test]
    fn test_named_cursor_options_are_real_fields() {
        let CursorPreset {
            model: _,
            effort: _,
            fast: _,
            context: _,
            params: _,
        } = CursorPreset::default();

        for name in NAMED_CURSOR_OPTIONS {
            let err = toml::from_str::<CursorPreset>(&format!("{name} = 0"))
                .expect_err("0 is the wrong type for every named option")
                .to_string();
            assert!(
                !err.contains("unknown field"),
                "`{name}` is in NAMED_CURSOR_OPTIONS but is not a CursorPreset field: {err}"
            );
        }
    }

    /// `params` keys must not collide with each other, not just with the named
    /// fields — `BTreeMap` orders by byte, so case variants are not adjacent.
    #[test]
    fn test_cursor_validate_rejects_params_keys_colliding_with_each_other() {
        let preset = CursorPreset {
            model: Some("auto-smart".to_string()),
            params: BTreeMap::from([
                ("optimize_for".to_string(), "cost".to_string()),
                ("Optimize_For".to_string(), "balanced".to_string()),
            ]),
            ..CursorPreset::default()
        };
        let Err(err) = preset.validate("x") else {
            panic!("params keys differing only in case should collide");
        };
        assert!(err.to_string().contains("differ only in case"), "{err}");
    }

    /// Case-insensitive, because `[effort=high,Effort=low]` is one option
    /// spelled twice whichever way Cursor folds ids.
    #[test]
    fn test_cursor_validate_rejects_params_key_colliding_case_insensitively() {
        let preset = CursorPreset {
            model: Some("claude-opus-5".to_string()),
            effort: Some("high".to_string()),
            params: BTreeMap::from([("Effort".to_string(), "low".to_string())]),
            ..CursorPreset::default()
        };
        let Err(err) = preset.validate("x") else {
            panic!("differently-cased params key should collide");
        };
        let msg = err.to_string();
        assert!(msg.contains("params.Effort"), "{msg}");
        assert!(msg.contains("`effort` field"), "{msg}");
    }

    /// A `params` key that duplicates a named field would give one option two
    /// spellings — the exact thing the bracket ban exists to prevent.
    #[test]
    fn test_cursor_validate_rejects_params_key_colliding_with_named_field() {
        for named in ["effort", "fast", "context"] {
            let preset = CursorPreset {
                model: Some("claude-opus-5".to_string()),
                params: BTreeMap::from([(named.to_string(), "x".to_string())]),
                ..CursorPreset::default()
            };
            let Err(err) = preset.validate("x") else {
                panic!("params.{named} should collide with the named field");
            };
            let msg = err.to_string();
            assert!(msg.contains(&format!("params.{named}")), "{named}: {msg}");
            assert!(msg.contains("duplicates"), "{named}: {msg}");
        }
    }

    /// Keys are composed into the bracket just like values, so they carry the
    /// same delimiter and whitespace rules.
    #[test]
    fn test_cursor_validate_rejects_malformed_params_key_or_value() {
        let cases = [
            ("bad=key", "cost"),
            ("optimize for", "cost"),
            ("optimize_for", "co,st"),
            ("optimize_for", ""),
        ];
        for (key, value) in cases {
            let preset = CursorPreset {
                model: Some("claude-opus-5".to_string()),
                params: BTreeMap::from([(key.to_string(), value.to_string())]),
                ..CursorPreset::default()
            };
            assert!(
                preset.validate("x").is_err(),
                "params {key:?}={value:?} should be rejected"
            );
        }
    }

    /// `params` alone still requires a `model` — Cursor cannot express a bracket
    /// option apart from the id it suffixes.
    #[test]
    fn test_cursor_validate_rejects_params_without_model() {
        let preset = CursorPreset {
            params: BTreeMap::from([("optimize_for".to_string(), "cost".to_string())]),
            ..CursorPreset::default()
        };
        let Err(err) = preset.validate("x") else {
            panic!("params with no model should be rejected");
        };
        assert!(err.to_string().contains("require `model`"), "{err}");
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
            params: BTreeMap::from([("optimize_for".to_string(), "cost".to_string())]),
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
