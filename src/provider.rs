use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(
    Clone,
    Copy,
    Debug,
    Deserialize,
    Eq,
    Hash,
    PartialEq,
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
