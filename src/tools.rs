use crate::types::Provider;

/// Returns the provider-specific name for a canonical tool.
///
/// Three-way result:
/// - `None` — unknown canonical tool; caller should emit `MissingMapping`
/// - `Some(None)` — intentionally unsupported on this provider; silently drop
/// - `Some(Some(name))` — use this provider-specific tool name
pub fn tool_name(canonical: &str, provider: Provider) -> Option<Option<&'static str>> {
    match (canonical, provider) {
        ("read",      Provider::Claude)   => Some(Some("Read")),
        ("read",      Provider::OpenCode) => Some(Some("read")),
        ("read",      Provider::Codex)    => Some(Some("read")),
        ("read",      Provider::Cursor)   => Some(None),
        ("write",     Provider::Claude)   => Some(Some("Write")),
        ("write",     Provider::OpenCode) => Some(Some("write")),
        ("write",     Provider::Codex)    => Some(Some("write")),
        ("write",     Provider::Cursor)   => Some(None),
        ("edit",      Provider::Claude)   => Some(Some("Edit")),
        ("edit",      Provider::OpenCode) => Some(Some("edit")),
        ("edit",      Provider::Codex)    => Some(Some("edit")),
        ("edit",      Provider::Cursor)   => Some(None),
        ("grep",      Provider::Claude)   => Some(Some("Grep")),
        ("grep",      Provider::OpenCode) => Some(Some("grep")),
        ("grep",      Provider::Codex)    => Some(Some("grep")),
        ("grep",      Provider::Cursor)   => Some(None),
        ("glob",      Provider::Claude)   => Some(Some("Glob")),
        ("glob",      Provider::OpenCode) => Some(Some("glob")),
        ("glob",      Provider::Codex)    => Some(Some("glob")),
        ("glob",      Provider::Cursor)   => Some(None),
        ("bash",      Provider::Claude)   => Some(Some("Bash")),
        ("bash",      Provider::OpenCode) => Some(Some("bash")),
        ("bash",      Provider::Codex)    => Some(Some("bash")),
        ("bash",      Provider::Cursor)   => Some(None),
        ("webfetch",  Provider::Claude)   => Some(Some("WebFetch")),
        ("webfetch",  Provider::OpenCode) => Some(Some("webfetch")),
        ("webfetch",  Provider::Codex)    => Some(Some("webfetch")),
        ("webfetch",  Provider::Cursor)   => Some(None),
        ("websearch", Provider::Claude)   => Some(Some("WebSearch")),
        ("websearch", Provider::OpenCode) => Some(Some("websearch")),
        ("websearch", Provider::Codex)    => Some(Some("websearch")),
        ("websearch", Provider::Cursor)   => Some(None),
        ("task",      Provider::Claude)   => Some(Some("Task")),
        ("task",      Provider::OpenCode) => Some(Some("task")),
        ("task",      Provider::Codex)    => Some(Some("task")),
        ("task",      Provider::Cursor)   => Some(None),
        ("todowrite", Provider::Claude)   => Some(Some("TodoWrite")),
        ("todowrite", Provider::OpenCode) => Some(Some("todowrite")),
        ("todowrite", Provider::Codex)    => Some(Some("todowrite")),
        ("todowrite", Provider::Cursor)   => Some(None),
        // ls is Claude-only; all other providers silently drop it
        ("ls",        Provider::Claude)   => Some(Some("LS")),
        ("ls",        _)                  => Some(None),
        // Unknown canonical tool name
        _                                 => None,
    }
}

/// Returns the sorted list of all tool names valid for a given provider.
///
/// Used by the `OpenCode` adapter to build its boolean tool map (universe of all tools).
pub fn all_tool_names(provider: Provider) -> Vec<&'static str> {
    const CANONICAL: &[&str] = &[
        "bash", "edit", "glob", "grep", "ls", "read", "task", "todowrite", "webfetch",
        "websearch", "write",
    ];
    let mut names: Vec<&'static str> = CANONICAL
        .iter()
        .filter_map(|&t| tool_name(t, provider).flatten())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_tools_mapped() {
        assert_eq!(tool_name("read", Provider::Claude), Some(Some("Read")));
        assert_eq!(tool_name("bash", Provider::OpenCode), Some(Some("bash")));
        assert_eq!(tool_name("grep", Provider::Codex), Some(Some("grep")));
    }

    #[test]
    fn test_cursor_tools_unsupported() {
        assert_eq!(tool_name("read", Provider::Cursor), Some(None));
        assert_eq!(tool_name("bash", Provider::Cursor), Some(None));
    }

    #[test]
    fn test_ls_claude_only() {
        assert_eq!(tool_name("ls", Provider::Claude), Some(Some("LS")));
        assert_eq!(tool_name("ls", Provider::OpenCode), Some(None));
        assert_eq!(tool_name("ls", Provider::Codex), Some(None));
        assert_eq!(tool_name("ls", Provider::Cursor), Some(None));
    }

    #[test]
    fn test_unknown_tool_returns_none() {
        assert_eq!(tool_name("unknown_tool", Provider::Claude), None);
        assert_eq!(tool_name("foobar", Provider::OpenCode), None);
    }

    #[test]
    fn test_all_tool_names_opencode_excludes_ls() {
        let names = all_tool_names(Provider::OpenCode);
        assert!(!names.contains(&"LS"));
        assert!(!names.contains(&"ls"));
        assert!(names.contains(&"bash"));
        assert!(names.contains(&"read"));
    }

    #[test]
    fn test_all_tool_names_claude_includes_ls() {
        let names = all_tool_names(Provider::Claude);
        assert!(names.contains(&"LS"));
    }

    #[test]
    fn test_all_tool_names_sorted() {
        let names = all_tool_names(Provider::Claude);
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }
}
