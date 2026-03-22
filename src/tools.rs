use crate::types::Provider;

/// Returns the provider-specific name for a canonical tool.
///
/// Three-way result:
/// - `Unknown` — unknown canonical tool; caller should emit `MissingMapping`
/// - `Unsupported` — intentionally unsupported on this provider; silently drop
/// - `Mapped(name)` — use this provider-specific tool name
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolMapping {
    Unknown,
    Unsupported,
    Mapped(&'static str),
}

pub fn tool_name(canonical: &str, provider: Provider) -> ToolMapping {
    match (canonical, provider) {
        ("read", Provider::Claude) => ToolMapping::Mapped("Read"),
        ("read", Provider::OpenCode) => ToolMapping::Mapped("read"),
        ("read", Provider::Codex) => ToolMapping::Mapped("read"),
        ("read", Provider::Cursor) => ToolMapping::Unsupported,
        ("write", Provider::Claude) => ToolMapping::Mapped("Write"),
        ("write", Provider::OpenCode) => ToolMapping::Mapped("write"),
        ("write", Provider::Codex) => ToolMapping::Mapped("write"),
        ("write", Provider::Cursor) => ToolMapping::Unsupported,
        ("edit", Provider::Claude) => ToolMapping::Mapped("Edit"),
        ("edit", Provider::OpenCode) => ToolMapping::Mapped("edit"),
        ("edit", Provider::Codex) => ToolMapping::Mapped("edit"),
        ("edit", Provider::Cursor) => ToolMapping::Unsupported,
        ("grep", Provider::Claude) => ToolMapping::Mapped("Grep"),
        ("grep", Provider::OpenCode) => ToolMapping::Mapped("grep"),
        ("grep", Provider::Codex) => ToolMapping::Mapped("grep"),
        ("grep", Provider::Cursor) => ToolMapping::Unsupported,
        ("glob", Provider::Claude) => ToolMapping::Mapped("Glob"),
        ("glob", Provider::OpenCode) => ToolMapping::Mapped("glob"),
        ("glob", Provider::Codex) => ToolMapping::Mapped("glob"),
        ("glob", Provider::Cursor) => ToolMapping::Unsupported,
        ("bash", Provider::Claude) => ToolMapping::Mapped("Bash"),
        ("bash", Provider::OpenCode) => ToolMapping::Mapped("bash"),
        ("bash", Provider::Codex) => ToolMapping::Mapped("bash"),
        ("bash", Provider::Cursor) => ToolMapping::Unsupported,
        ("webfetch", Provider::Claude) => ToolMapping::Mapped("WebFetch"),
        ("webfetch", Provider::OpenCode) => ToolMapping::Mapped("webfetch"),
        ("webfetch", Provider::Codex) => ToolMapping::Mapped("webfetch"),
        ("webfetch", Provider::Cursor) => ToolMapping::Unsupported,
        ("websearch", Provider::Claude) => ToolMapping::Mapped("WebSearch"),
        ("websearch", Provider::OpenCode) => ToolMapping::Mapped("websearch"),
        ("websearch", Provider::Codex) => ToolMapping::Mapped("websearch"),
        ("websearch", Provider::Cursor) => ToolMapping::Unsupported,
        ("task", Provider::Claude) => ToolMapping::Mapped("Task"),
        ("task", Provider::OpenCode) => ToolMapping::Mapped("task"),
        ("task", Provider::Codex) => ToolMapping::Mapped("task"),
        ("task", Provider::Cursor) => ToolMapping::Unsupported,
        ("todowrite", Provider::Claude) => ToolMapping::Mapped("TodoWrite"),
        ("todowrite", Provider::OpenCode) => ToolMapping::Mapped("todowrite"),
        ("todowrite", Provider::Codex) => ToolMapping::Mapped("todowrite"),
        ("todowrite", Provider::Cursor) => ToolMapping::Unsupported,
        // ls is Claude-only; all other providers silently drop it
        ("ls", Provider::Claude) => ToolMapping::Mapped("LS"),
        ("ls", _) => ToolMapping::Unsupported,
        // Unknown canonical tool name
        _ => ToolMapping::Unknown,
    }
}

/// Returns the sorted list of all tool names valid for a given provider.
///
/// Used by the `OpenCode` adapter to build its boolean tool map (universe of all tools).
pub fn all_tool_names(provider: Provider) -> Vec<&'static str> {
    const CANONICAL: &[&str] = &[
        "bash",
        "edit",
        "glob",
        "grep",
        "ls",
        "read",
        "task",
        "todowrite",
        "webfetch",
        "websearch",
        "write",
    ];
    let mut names: Vec<&'static str> = CANONICAL
        .iter()
        .filter_map(|&t| match tool_name(t, provider) {
            ToolMapping::Mapped(name) => Some(name),
            ToolMapping::Unsupported | ToolMapping::Unknown => None,
        })
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
        assert_eq!(
            tool_name("read", Provider::Claude),
            ToolMapping::Mapped("Read")
        );
        assert_eq!(
            tool_name("bash", Provider::OpenCode),
            ToolMapping::Mapped("bash")
        );
        assert_eq!(
            tool_name("grep", Provider::Codex),
            ToolMapping::Mapped("grep")
        );
    }

    #[test]
    fn test_cursor_tools_unsupported() {
        assert_eq!(
            tool_name("read", Provider::Cursor),
            ToolMapping::Unsupported
        );
        assert_eq!(
            tool_name("bash", Provider::Cursor),
            ToolMapping::Unsupported
        );
    }

    #[test]
    fn test_ls_claude_only() {
        assert_eq!(tool_name("ls", Provider::Claude), ToolMapping::Mapped("LS"));
        assert_eq!(
            tool_name("ls", Provider::OpenCode),
            ToolMapping::Unsupported
        );
        assert_eq!(tool_name("ls", Provider::Codex), ToolMapping::Unsupported);
        assert_eq!(tool_name("ls", Provider::Cursor), ToolMapping::Unsupported);
    }

    #[test]
    fn test_unknown_tool_returns_none() {
        assert_eq!(
            tool_name("unknown_tool", Provider::Claude),
            ToolMapping::Unknown
        );
        assert_eq!(
            tool_name("foobar", Provider::OpenCode),
            ToolMapping::Unknown
        );
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
