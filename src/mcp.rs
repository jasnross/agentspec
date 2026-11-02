//! MCP server declarations: the `[mcp.<name>]` sections of `agentspec.toml`.
//!
//! Like `presets.rs`, this declares fields and holds no provider
//! interpretation. Composing a provider's tool id from a declaration, and
//! checking an override against a provider's naming rules, belong to the
//! adapters. A provider block may also map logical tool names to the real
//! tool names of the server that provider reaches; [`real_tool_name`] looks
//! one up, `resolve_named_tools` resolves a whole grant, and the adapter
//! spells the result.

use std::collections::BTreeMap;

use serde::Deserialize;

/// Declared MCP servers: logical server name → per-provider registration.
///
/// A `BTreeMap` so every check and every composition visits servers in name
/// order without a separate sort.
pub type McpServers = BTreeMap<String, McpServer>;

/// A provider block's `tool-names`: logical tool name → the real tool name
/// of the server that provider reaches. A `BTreeMap` so checks visit
/// entries in name order.
pub type McpToolNames = BTreeMap<String, String>;

/// One `[mcp.<name>]` declaration.
///
/// The table's key is the server's logical name, which specs grant from
/// `capabilities.mcp` and bodies name in `mcp_tool()`. Each provider block
/// overrides how that provider registers the server, and may map the
/// logical tool names specs use to the real names of that provider's
/// server. An absent block, or a block with no `server`, means the provider
/// registers it under the logical name; resolving that default is the
/// adapter's job, so an empty `[mcp.quip]` is a complete declaration.
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
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct ClaudeMcpServer {
    /// The name Claude Code registers the server under.
    pub server: Option<String>,
    /// The Claude plugin that bundles the server, which changes the prefix
    /// Claude Code gives its tools.
    pub plugin: Option<String>,
    /// Logical tool name → the real tool name of the server this provider
    /// reaches. Absent means every tool keeps its logical name.
    pub tool_names: Option<McpToolNames>,
}

/// `[mcp.<name>.cursor]`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct CursorMcpServer {
    /// The name Cursor registers the server under.
    pub server: Option<String>,
    /// Logical tool name → the real tool name of the server this provider
    /// reaches. Absent means every tool keeps its logical name.
    pub tool_names: Option<McpToolNames>,
}

/// `[mcp.<name>.opencode]`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct OpenCodeMcpServer {
    /// The name `OpenCode` registers the server under.
    pub server: Option<String>,
    /// Logical tool name → the real tool name of the server this provider
    /// reaches. Absent means every tool keeps its logical name.
    pub tool_names: Option<McpToolNames>,
}

/// `[A-Za-z0-9_-]+`: the only characters every provider leaves unchanged.
pub fn is_mcp_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The real name of `tool` under `names`, or `tool` itself when the block
/// declares no map. Validation guarantees every tool a provider writes has
/// an entry; a missing one is a wiring bug.
pub fn real_tool_name<'a>(names: Option<&'a McpToolNames>, tool: &'a str) -> &'a str {
    let Some(names) = names else {
        return tool;
    };
    let real = names.get(tool).map(String::as_str);
    debug_assert!(
        real.is_some(),
        "unmapped MCP tool '{tool}' should have been rejected by validation"
    );
    real.unwrap_or(tool)
}

/// The real tools `tools` resolve to under `names`: deduplicated, in name
/// order. Two logical tools mapped to one real tool resolve to it once, so
/// a grant naming both emits one id.
pub(crate) fn resolve_named_tools<'a>(
    names: Option<&'a McpToolNames>,
    tools: &'a [String],
) -> Vec<&'a str> {
    let mut real: Vec<&str> = tools
        .iter()
        .map(|tool| real_tool_name(names, tool))
        .collect();
    real.sort_unstable();
    real.dedup();
    real
}

#[cfg(test)]
mod tests {
    use super::{McpToolNames, is_mcp_name, real_tool_name, resolve_named_tools};

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

    #[test]
    fn test_tool_names_absent_map_keeps_logical_name() {
        assert_eq!(real_tool_name(None, "t"), "t");
    }

    #[test]
    fn test_tool_names_map_gives_real_name() {
        let map = McpToolNames::from([("t".to_owned(), "real_t".to_owned())]);
        assert_eq!(real_tool_name(Some(&map), "t"), "real_t");
    }

    #[test]
    fn test_resolve_named_tools_dedups_shared_real_name() {
        let map = McpToolNames::from([
            ("a".to_owned(), "x".to_owned()),
            ("b".to_owned(), "x".to_owned()),
            ("c".to_owned(), "y".to_owned()),
        ]);
        let tools = ["b", "c", "a"].map(String::from);
        assert_eq!(resolve_named_tools(Some(&map), &tools), ["x", "y"]);
    }

    #[test]
    fn test_resolve_named_tools_without_map_sorts_logical_names() {
        let tools = ["b", "a"].map(String::from);
        assert_eq!(resolve_named_tools(None, &tools), ["a", "b"]);
    }
}
