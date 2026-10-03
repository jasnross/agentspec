//! MCP server declarations: the `[mcp.<name>]` sections of `agentspec.toml`.
//!
//! Like `presets.rs`, this declares fields and holds no provider
//! interpretation. Composing a provider's tool id from a declaration, and
//! checking an override against a provider's naming rules, belong to the
//! adapters.

use std::collections::BTreeMap;

use serde::Deserialize;

/// Declared MCP servers: logical server name → per-provider registration.
///
/// A `BTreeMap` so every check and every composition visits servers in name
/// order without a separate sort.
pub type McpServers = BTreeMap<String, McpServer>;

/// One `[mcp.<name>]` declaration.
///
/// The table's key is the server's logical name, which specs grant from
/// `capabilities.mcp` and bodies name in `mcp_tool()`. Each provider block
/// overrides how that provider registers the server. An absent block, or a
/// block with no `server`, means the provider registers it under the logical
/// name; resolving that default is the adapter's job, so an empty `[mcp.quip]`
/// is a complete declaration.
///
/// The names are a contract the generated output places on whoever runs it,
/// such as a published plugin's consumers. agentspec never checks them against
/// the compiling machine's provider installation.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct McpServer {
    pub claude: Option<ClaudeMcpServer>,
    pub cursor: Option<CursorMcpServer>,
    pub opencode: Option<OpenCodeMcpServer>,
}

/// `[mcp.<name>.claude]`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ClaudeMcpServer {
    /// The name Claude Code registers the server under.
    pub server: Option<String>,
    /// The Claude plugin that bundles the server, which changes the prefix
    /// Claude Code gives its tools.
    pub plugin: Option<String>,
}

/// `[mcp.<name>.cursor]`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CursorMcpServer {
    /// The name Cursor registers the server under.
    pub server: Option<String>,
}

/// `[mcp.<name>.opencode]`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct OpenCodeMcpServer {
    /// The name `OpenCode` registers the server under.
    pub server: Option<String>,
}

/// `[A-Za-z0-9_-]+`: the only characters every provider leaves unchanged.
pub fn is_mcp_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::is_mcp_name;

    #[test]
    fn test_is_mcp_name_accepts_provider_neutral_names() {
        for name in ["quip", "work-tools", "fx_2", "A"] {
            assert!(is_mcp_name(name), "{name}");
        }
    }

    #[test]
    fn test_is_mcp_name_rejects_empty_and_other_characters() {
        for name in ["", "a.b", "a b", "a:b", "a/b", "ä"] {
            assert!(!is_mcp_name(name), "{name:?}");
        }
    }
}
