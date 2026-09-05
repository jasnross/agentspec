//! The provider-neutral vocabulary for settings a spec author writes.
//!
//! Depends on nothing else in the crate. `adapters`, `presets`, and `compile`
//! all name settings through this module, which is what keeps `presets.rs`
//! from having to import from `adapters` to say what a preset configures.

use std::borrow::Cow;

/// A setting an author writes, named as authors name it.
///
/// Provider-neutral by construction: Cursor composes [`SettingKey::Effort`]
/// into its `model` value and Claude emits it as its own key, and both record
/// `SettingKey::Effort`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SettingKey {
    /// The spec's own content. A spec that produces no file for a provider
    /// loses this, which is how "this provider emits no hooks" is expressed
    /// without a dedicated variant.
    Body,
    Model,
    Effort,
    Fast,
    Context,
    Variant,
    /// A Cursor bracket option with no named field.
    Param(String),
    Tools,
    Paths,
}

impl SettingKey {
    /// The payload-free classification, for comparison against a capability
    /// table. Every [`SettingKey::Param`] payload maps to
    /// [`SettingKind::Param`].
    pub fn kind(&self) -> SettingKind {
        match self {
            Self::Body => SettingKind::Body,
            Self::Model => SettingKind::Model,
            Self::Effort => SettingKind::Effort,
            Self::Fast => SettingKind::Fast,
            Self::Context => SettingKind::Context,
            Self::Variant => SettingKind::Variant,
            Self::Param(_) => SettingKind::Param,
            Self::Tools => SettingKind::Tools,
            Self::Paths => SettingKind::Paths,
        }
    }

    /// The user-facing name, as it appears in a report line.
    ///
    /// `Body` reads `"content"` because that is what an author calls the part
    /// of a spec file below the frontmatter; every other named field reads as
    /// the field itself.
    pub fn label(&self) -> Cow<'_, str> {
        match self {
            Self::Body => Cow::Borrowed("content"),
            Self::Model => Cow::Borrowed("model"),
            Self::Effort => Cow::Borrowed("effort"),
            Self::Fast => Cow::Borrowed("fast"),
            Self::Context => Cow::Borrowed("context"),
            Self::Variant => Cow::Borrowed("variant"),
            Self::Param(key) => Cow::Owned(format!("params.{key}")),
            Self::Tools => Cow::Borrowed("tools"),
            Self::Paths => Cow::Borrowed("paths"),
        }
    }
}

/// [`SettingKey`] without its payload, so a capability table is
/// const-constructible.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SettingKind {
    Body,
    Model,
    Effort,
    Fast,
    Context,
    Variant,
    /// Cursor's `params` is an open, author-defined key space by design — its
    /// option ids vary by model and the catalog is account-specific — so no
    /// static table can enumerate its members. Declaring `Param` says the
    /// provider accepts arbitrary bracket options, which is the only claim
    /// available and the true one.
    Param,
    Tools,
    Paths,
}

/// The settings a frontmatter struct is carrying, as populated.
///
/// Implemented on the serializable frontmatter structs themselves, so the
/// record comes from the struct that becomes bytes rather than from the inputs
/// the adapter was handed. Most implementations read `Some`-ness off the
/// serialized fields directly. Cursor's agent struct is the exception: it
/// composes every option into one `model` string that cannot be read back
/// without a second parser, so it carries the record in a `#[serde(skip)]`
/// field written by the same expression that builds the string.
pub(crate) trait Carries {
    fn carried(&self) -> Vec<SettingKey>;
}

#[cfg(test)]
mod tests {
    use super::{SettingKey, SettingKind};

    #[test]
    fn test_param_kind_erases_payload() {
        assert_eq!(
            SettingKey::Param("optimize_for".to_owned()).kind(),
            SettingKind::Param
        );
        assert_eq!(
            SettingKey::Param("anything".to_owned()).kind(),
            SettingKey::Param("else".to_owned()).kind()
        );
    }

    #[test]
    fn test_body_labels_as_content() {
        assert_eq!(SettingKey::Body.label(), "content");
    }

    #[test]
    fn test_param_label_is_namespaced() {
        assert_eq!(
            SettingKey::Param("optimize_for".to_owned()).label(),
            "params.optimize_for"
        );
    }
}
