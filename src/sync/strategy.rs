use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub use agentspec::plan::NamePrefixMode;

/// The outcome of a single file sync operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SyncAction {
    /// File was newly created.
    Created,
    /// An existing file was overwritten with updated content.
    Updated,
    /// No change — content already matches.
    Unchanged,
    /// A user-owned file was backed up with a timestamp suffix before writing.
    BackedUp,
}

pub(crate) fn prefix_rel_path(rel: &Path, prefix: Option<&str>) -> PathBuf {
    let Some(prefix) = prefix else {
        return rel.to_path_buf();
    };

    let mut components = rel.components();
    let Some(first) = components.next() else {
        return rel.to_path_buf();
    };

    let mut prefixed_first = OsString::from(prefix);
    prefixed_first.push(first.as_os_str());

    let result = PathBuf::from(prefixed_first);
    let rest = components.as_path();
    // Avoid joining with an empty path: PathBuf::join("") can add a trailing
    // separator on some platforms, producing e.g. "tw-agent.md/" instead of "tw-agent.md".
    if rest.as_os_str().is_empty() {
        result
    } else {
        result.join(rest)
    }
}

pub(crate) fn should_prefix_frontmatter_name(rel_path: &Path, mode: NamePrefixMode) -> bool {
    match mode {
        NamePrefixMode::Skills => rel_path.file_name().is_some_and(|name| name == "SKILL.md"),
        NamePrefixMode::Agents => {
            rel_path.extension().is_some_and(|ext| ext == "md")
                && rel_path
                    .parent()
                    .is_none_or(|parent| parent.as_os_str().is_empty())
        }
    }
}

pub(crate) fn prefix_frontmatter_name(content: &str, prefix: &str) -> String {
    let mut in_frontmatter = false;
    let mut frontmatter_done = false;
    let mut first = true;
    let prefix_marker = format!("{prefix}:");

    content.lines().fold(String::new(), |mut out, line| {
        if first && line == "---" {
            first = false;
            in_frontmatter = true;
        } else {
            first = false;

            if in_frontmatter && !frontmatter_done {
                if line == "---" {
                    frontmatter_done = true;
                    in_frontmatter = false;
                } else if let Some(value) = line.strip_prefix("name: ")
                    && !value.starts_with(&prefix_marker)
                {
                    out.push_str("name: ");
                    out.push_str(prefix);
                    out.push(':');
                    out.push_str(value);
                    out.push('\n');
                    return out;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
        out
    })
}

/// Strips `name:` lines from YAML frontmatter in the given content string.
///
/// Only removes `name:` lines within the frontmatter block (between `---` delimiters),
/// leaving any `name:` occurrences in the Markdown body untouched.
/// Assumes `name:` is always a single-line scalar — the compiler always emits
/// single-line `name:` values in generated frontmatter.
pub(crate) fn strip_frontmatter_name(content: &str) -> String {
    let mut in_frontmatter = false;
    let mut frontmatter_done = false;
    let mut first = true;
    content
        .lines()
        .filter(|line| {
            if first && *line == "---" {
                first = false;
                in_frontmatter = true;
                return true;
            }
            first = false;
            if in_frontmatter && !frontmatter_done {
                if *line == "---" {
                    frontmatter_done = true;
                    in_frontmatter = false;
                    return true;
                }
                return !line.starts_with("name:");
            }
            true
        })
        .fold(String::new(), |mut out, line| {
            out.push_str(line);
            out.push('\n');
            out
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_frontmatter_name_removes_name_lines() {
        let input = "---\nname: my-skill\ndescription: test\n---\n\nbody\n";
        let result = strip_frontmatter_name(input);
        assert!(!result.contains("name:"), "name: line should be removed");
        assert!(
            result.contains("description: test"),
            "other lines preserved"
        );
    }

    #[test]
    fn test_strip_frontmatter_name_preserves_body_name_lines() {
        // `name:` appears both in frontmatter and in the Markdown body — only frontmatter should be stripped.
        let input = "---\nname: my-skill\ndescription: test\n---\n\nname: example\n";
        let result = strip_frontmatter_name(input);
        assert!(
            result.contains("name: example"),
            "body name: line should be preserved"
        );
        assert!(
            !result.contains("name: my-skill"),
            "frontmatter name: line should be removed"
        );
    }
}
