use clap::ValueEnum;
use serde::{Deserialize, Serialize};

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
}
