use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use minijinja::Environment;

use super::ExtraIncludeDir;
use crate::provider::Provider;
use crate::spec::{Spec, ToolFrontmatter};

/// Build a `MiniJinja` environment for `spec` with a lazy loader that
/// resolves include paths relative to `sources_dir`, plus named extra dirs.
///
/// Enables `{% include "fragments/shared.md" %}`, `{% include "./detail.md" %}`
/// (self-relative), and `{{ tool("<canonical>") }}` calls in all specs.
///
/// `script()` is additionally registered when `spec` is `Spec::Skill(_)`.
pub fn build_environment(
    sources_dir: &Path,
    extra_dirs: &[ExtraIncludeDir],
    provider: Option<Provider>,
    spec: &Spec,
) -> Environment<'static> {
    let mut env = Environment::new();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Lenient);

    env.set_path_join_callback(|name: &str, parent: &str| -> Cow<'_, str> {
        if !name.starts_with("./") {
            return Cow::Borrowed(name);
        }
        debug_assert!(
            !parent.is_empty(),
            "parent should not be empty with template_from_named_str"
        );
        let mut segments: Vec<&str> = parent.split('/').collect();
        segments.pop();
        for part in name.split('/') {
            if part == ".." {
                return Cow::Borrowed(name);
            }
            if part != "." && !part.is_empty() {
                segments.push(part);
            }
        }
        segments.join("/").into()
    });

    let sources_owned = sources_dir.to_path_buf();
    let extra_owned = extra_dirs.to_vec();
    env.set_loader(move |name: &str| resolve_include(name, &sources_owned, &extra_owned));

    env.add_function("tool", move |name: String| resolve_tool(&name, provider));

    if let Spec::Skill(s) = spec {
        let known_scripts: HashSet<PathBuf> = s.supporting_files.keys().cloned().collect();
        env.add_function("script", move |name: String| {
            resolve_script(&name, provider, &known_scripts)
        });
    }
    env
}

/// Resolve an include path to file content, with two-tier error handling:
/// author mistakes return `Err` (actionable); security boundaries return
/// `Ok(None)` (silent).
pub(super) fn resolve_include(
    name: &str,
    sources_dir: &Path,
    extra_dirs: &[ExtraIncludeDir],
) -> Result<Option<String>, minijinja::Error> {
    if Path::new(name).has_root() {
        return Ok(None);
    }

    if Path::new(name)
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("parent directory traversal (..) is not allowed in include paths: \"{name}\""),
        ));
    }

    if !std::path::Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
    {
        return Err(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("include paths must use .md extension, got \"{name}\""),
        ));
    }

    let full = sources_dir.join(name);
    if let Some(content) = read_if_within(sources_dir, &full)? {
        return Ok(Some(content));
    }

    for extra in extra_dirs {
        let prefix = format!("{}/", extra.name);
        if let Some(rest) = name.strip_prefix(&prefix) {
            let full = extra.path.join(rest);
            if let Some(content) = read_if_within(&extra.path, &full)? {
                return Ok(Some(content));
            }
        }
    }

    Ok(None)
}

fn read_if_within(root: &Path, full: &Path) -> Result<Option<String>, minijinja::Error> {
    let canonical_root = match std::fs::canonicalize(root) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                format!("failed to resolve include root directory: {e}"),
            ));
        }
    };
    let canonical_full = match std::fs::canonicalize(full) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                format!("failed to resolve include path: {e}"),
            ));
        }
    };

    if !canonical_full.starts_with(&canonical_root) {
        return Ok(None);
    }

    match std::fs::read_to_string(&canonical_full) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("failed to read include file: {e}"),
        )),
    }
}

fn resolve_script(
    name: &str,
    provider: Option<Provider>,
    known_scripts: &HashSet<PathBuf>,
) -> Result<String, minijinja::Error> {
    let relative = Path::new(name);

    if relative.has_root()
        || relative
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("script() path must be relative without '..', got: \"{name}\""),
        ));
    }

    let full_path = PathBuf::from("scripts").join(relative);

    if !known_scripts.contains(&full_path) {
        return Err(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!(
                "script(\"{name}\") references a file not found in this skill's scripts/ directory"
            ),
        ));
    }

    // Use explicit '/' so skill content always contains POSIX paths regardless of host OS.
    let posix_path = format!("scripts/{name}");
    Ok(match provider.and_then(|p| p.adapter().body_skill_root()) {
        Some(root) => format!("{root}/{posix_path}"),
        None => posix_path,
    })
}

/// Resolve a canonical tool name to the provider-specific body-level name.
///
/// Returns a `MiniJinja` render error if `name` is not a known canonical
/// tool. When `provider` is `None`, the canonical name is returned unchanged
/// (after the round-trip through `ToolFrontmatter` confirms it is valid).
fn resolve_tool(name: &str, provider: Option<Provider>) -> Result<String, minijinja::Error> {
    let tool: ToolFrontmatter = name.parse().map_err(|_| {
        minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("unknown canonical tool name '{name}'"),
        )
    })?;
    let Some(p) = provider else {
        return Ok(name.to_owned());
    };
    Ok(p.adapter().body_tool_name(&tool).to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use indexmap::IndexMap;

    use super::*;
    use crate::provider::Provider;
    use crate::spec::{
        AgentFrontmatter, AgentSpec, HookEvent, HookFrontmatter, HookSpec, RuleFrontmatter,
        RuleSpec, SkillFrontmatter, SkillSpec, SupportingFile,
    };

    fn write_source_files(dir: &std::path::Path, files: &HashMap<String, String>) {
        for (name, content) in files {
            let path = dir.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("expected value");
            }
            std::fs::write(&path, content).expect("expected value");
        }
    }

    fn dummy_agent_spec() -> Spec {
        Spec::Agent(AgentSpec {
            path: PathBuf::from("/tmp/agent.md"),
            frontmatter: AgentFrontmatter {
                id: "dummy-agent".to_string(),
                description: "dummy".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: String::new(),
        })
    }

    fn dummy_skill_spec() -> Spec {
        dummy_skill_spec_with_files(&["foo.sh"])
    }

    fn dummy_skill_spec_with_files(names: &[&str]) -> Spec {
        let mut supporting_files = IndexMap::new();
        for name in names {
            supporting_files.insert(
                PathBuf::from(format!("scripts/{name}")),
                SupportingFile {
                    content: vec![],
                    mode: 0o755,
                },
            );
        }
        Spec::Skill(SkillSpec {
            path: PathBuf::from("/tmp/skill.md"),
            frontmatter: SkillFrontmatter {
                id: "dummy-skill".to_string(),
                description: None,
                tags: None,
                user_invocable: false,
                agent_invocable: false,
                execution: None,
                capabilities: None,
            },
            body: String::new(),
            supporting_files,
        })
    }

    fn dummy_rule_spec() -> Spec {
        Spec::Rule(RuleSpec {
            path: PathBuf::from("/tmp/rule.md"),
            frontmatter: RuleFrontmatter {
                id: "dummy-rule".to_string(),
                description: None,
                tags: None,
                paths: None,
            },
            body: String::new(),
        })
    }

    fn dummy_hook_spec() -> Spec {
        Spec::Hook(HookSpec {
            path: PathBuf::from("/tmp/hooks.toml"),
            frontmatter: HookFrontmatter {
                id: "dummy-hook".to_string(),
                events: vec![HookEvent::SessionStart],
                script: PathBuf::from("scripts/init.sh"),
                matcher: None,
                timeout: None,
                description: None,
                tags: None,
                args: None,
            },
            body: String::new(),
            supporting_files: IndexMap::new(),
        })
    }

    fn render_body(body: &str, provider: Option<Provider>, spec: &Spec) -> String {
        let tmp = tempfile::tempdir().expect("expected value");
        let env = build_environment(tmp.path(), &[], provider, spec);
        let template = env.template_from_str(body).expect("expected value");
        template
            .render(minijinja::context! {})
            .expect("expected value")
    }

    fn render_with_templates(
        body: &str,
        files: &HashMap<String, String>,
        provider: Option<Provider>,
        spec: &Spec,
    ) -> Result<String, String> {
        let tmp = tempfile::tempdir().map_err(|e| format!("{e:#}"))?;
        write_source_files(tmp.path(), files);
        let env = build_environment(tmp.path(), &[], provider, spec);
        let template = env.template_from_str(body).map_err(|e| format!("{e:#}"))?;
        template
            .render(minijinja::context! {})
            .map_err(|e| format!("{e:#}"))
    }

    #[test]
    fn test_simple_include() {
        let tmp = tempfile::tempdir().expect("expected value");
        std::fs::write(tmp.path().join("greeting.md"), "Hello, world!").expect("expected value");

        let env = build_environment(tmp.path(), &[], None, &dummy_skill_spec());
        let template = env
            .template_from_str("Before.\n{% include \"greeting.md\" %}\nAfter.")
            .expect("expected value");
        let result = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(result, "Before.\nHello, world!\nAfter.");
    }

    #[test]
    fn test_include_with_variables() {
        let tmp = tempfile::tempdir().expect("expected value");
        std::fs::write(tmp.path().join("greeting.md"), "Hello, {{ name }}!")
            .expect("expected value");

        let env = build_environment(tmp.path(), &[], None, &dummy_skill_spec());
        let template = env
            .template_from_str(
                "{% with name = \"Alice\" %}{% include \"greeting.md\" %}{% endwith %}",
            )
            .expect("expected value");
        let result = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(result, "Hello, Alice!");
    }

    #[test]
    fn test_nested_includes() {
        let tmp = tempfile::tempdir().expect("expected value");
        std::fs::write(tmp.path().join("inner.md"), "inner content").expect("expected value");
        std::fs::write(
            tmp.path().join("outer.md"),
            "before {% include \"inner.md\" %} after",
        )
        .expect("expected value");

        let env = build_environment(tmp.path(), &[], None, &dummy_skill_spec());
        let template = env
            .template_from_str("start {% include \"outer.md\" %} end")
            .expect("expected value");
        let result = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(result, "start before inner content after end");
    }

    #[test]
    fn test_missing_fragment_errors() {
        let tmp = tempfile::tempdir().expect("expected value");
        let env = build_environment(tmp.path(), &[], None, &dummy_skill_spec());
        let template = env
            .template_from_str("{% include \"nonexistent.md\" %}")
            .expect("expected value");
        let result = template.render(minijinja::context! {});
        assert!(result.is_err());
    }

    #[test]
    fn test_filter_indent_with_include() {
        let tmp = tempfile::tempdir().expect("expected value");
        std::fs::write(tmp.path().join("rules.md"), "Rule 1\nRule 2\nRule 3")
            .expect("expected value");

        let env = build_environment(tmp.path(), &[], None, &dummy_skill_spec());
        let template = env
            .template_from_str(
                "Items:\n   {% filter indent(3, first=false) %}{% include \"rules.md\" %}{% endfilter %}",
            )
            .expect("expected value");
        let result = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(result, "Items:\n   Rule 1\n   Rule 2\n   Rule 3");
    }

    #[test]
    fn test_filter_indent_with_variables() {
        let tmp = tempfile::tempdir().expect("expected value");
        std::fs::write(
            tmp.path().join("greeting.md"),
            "Hello, {{ name }}!\nWelcome aboard.",
        )
        .expect("expected value");

        let env = build_environment(tmp.path(), &[], None, &dummy_skill_spec());
        let template = env
            .template_from_str(
                "Message:\n    {% filter indent(4, first=false) %}{% with name = \"Alice\" %}{% include \"greeting.md\" %}{% endwith %}{% endfilter %}",
            )
            .expect("expected value");
        let result = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(result, "Message:\n    Hello, Alice!\n    Welcome aboard.");
    }

    #[test]
    fn test_tool_resolves_for_claude() {
        let out = render_body(
            r#"{{ tool("question") }}"#,
            Some(Provider::Claude),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "AskUserQuestion");
        let out = render_body(
            r#"{{ tool("subagent") }}"#,
            Some(Provider::Claude),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "Agent");
        let out = render_body(
            r#"{{ tool("skill") }}"#,
            Some(Provider::Claude),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "Skill");
    }

    #[test]
    fn test_tool_resolves_for_cursor() {
        let out = render_body(
            r#"{{ tool("question") }}"#,
            Some(Provider::Cursor),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "Ask questions");
        let out = render_body(
            r#"{{ tool("subagent") }}"#,
            Some(Provider::Cursor),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "Task");
        let out = render_body(
            r#"{{ tool("skill") }}"#,
            Some(Provider::Cursor),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "Skill runner");
    }

    #[test]
    fn test_tool_resolves_for_opencode() {
        let out = render_body(
            r#"{{ tool("question") }}"#,
            Some(Provider::OpenCode),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "question");
        let out = render_body(
            r#"{{ tool("subagent") }}"#,
            Some(Provider::OpenCode),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "task");
        let out = render_body(
            r#"{{ tool("skill") }}"#,
            Some(Provider::OpenCode),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "skill");
    }

    #[test]
    fn test_tool_passes_through_canonical_when_provider_is_none() {
        let out = render_body(r#"{{ tool("question") }}"#, None, &dummy_skill_spec());
        assert_eq!(out, "question");
    }

    #[test]
    fn test_tool_resolves_inside_included_fragment() {
        let tmp = tempfile::tempdir().expect("expected value");
        std::fs::write(
            tmp.path().join("tool-ref.md"),
            r#"Use {{ tool("question") }}."#,
        )
        .expect("expected value");
        let env = build_environment(tmp.path(), &[], Some(Provider::Claude), &dummy_skill_spec());
        let template = env
            .template_from_str(r#"{% include "tool-ref.md" %}"#)
            .expect("expected value");
        let out = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(out, "Use AskUserQuestion.");
    }

    #[test]
    fn test_tool_unknown_name_errors() {
        let tmp = tempfile::tempdir().expect("expected value");
        let env = build_environment(tmp.path(), &[], Some(Provider::Claude), &dummy_skill_spec());
        let template = env
            .template_from_str(r#"{{ tool("nope") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for unknown tool");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("nope"),
            "error message should contain offending name 'nope', got: {msg}"
        );
    }

    #[test]
    fn test_script_registered_for_skill_body() {
        let out = render_body(
            r#"{{ script("foo.sh") }}"#,
            Some(Provider::Claude),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "${CLAUDE_SKILL_DIR}/scripts/foo.sh");
    }

    #[test]
    fn test_script_passes_through_for_cursor_skill() {
        let out = render_body(
            r#"{{ script("foo.sh") }}"#,
            Some(Provider::Cursor),
            &dummy_skill_spec(),
        );
        assert_eq!(out, "scripts/foo.sh");
    }

    #[test]
    fn test_script_not_registered_for_agent_body() {
        let tmp = tempfile::tempdir().expect("expected value");
        let env = build_environment(tmp.path(), &[], Some(Provider::Claude), &dummy_agent_spec());
        let template = env
            .template_from_str(r#"{{ script("foo.sh") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for agent spec");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unknown"),
            "expected 'unknown' in error, got: {msg}"
        );
        assert!(
            msg.contains("script"),
            "expected 'script' in error, got: {msg}"
        );
    }

    #[test]
    fn test_script_not_registered_for_rule_body() {
        let tmp = tempfile::tempdir().expect("expected value");
        let env = build_environment(tmp.path(), &[], Some(Provider::Claude), &dummy_rule_spec());
        let template = env
            .template_from_str(r#"{{ script("foo.sh") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for rule spec");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unknown"),
            "expected 'unknown' in error, got: {msg}"
        );
        assert!(
            msg.contains("script"),
            "expected 'script' in error, got: {msg}"
        );
    }

    #[test]
    fn test_script_not_registered_for_hook_body() {
        let tmp = tempfile::tempdir().expect("expected value");
        let env = build_environment(tmp.path(), &[], Some(Provider::Claude), &dummy_hook_spec());
        let template = env
            .template_from_str(r#"{{ script("foo.sh") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for hook spec");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unknown"),
            "expected 'unknown' in error, got: {msg}"
        );
        assert!(
            msg.contains("script"),
            "expected 'script' in error, got: {msg}"
        );
    }

    #[test]
    fn test_script_validate_mode_skill_renders() {
        let out = render_body(r#"{{ script("foo.sh") }}"#, None, &dummy_skill_spec());
        assert_eq!(out, "scripts/foo.sh");
    }

    #[test]
    fn test_script_validate_mode_agent_errors() {
        let tmp = tempfile::tempdir().expect("expected value");
        let env = build_environment(tmp.path(), &[], None, &dummy_agent_spec());
        let template = env
            .template_from_str(r#"{{ script("foo.sh") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for agent spec in validate mode");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unknown"),
            "expected 'unknown' in error, got: {msg}"
        );
        assert!(
            msg.contains("script"),
            "expected 'script' in error, got: {msg}"
        );
    }

    #[test]
    fn test_script_missing_file_errors() {
        let spec = dummy_skill_spec_with_files(&["exists.sh"]);
        let tmp = tempfile::tempdir().expect("expected value");
        let env = build_environment(tmp.path(), &[], Some(Provider::Claude), &spec);
        let template = env
            .template_from_str(r#"{{ script("missing.sh") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for missing script");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("missing.sh"),
            "error should name the missing file, got: {msg}"
        );
        assert!(
            msg.contains("not found"),
            "error should say 'not found', got: {msg}"
        );
    }

    #[test]
    fn test_script_missing_file_errors_in_validate_mode() {
        let spec = dummy_skill_spec_with_files(&["exists.sh"]);
        let tmp = tempfile::tempdir().expect("expected value");
        let env = build_environment(tmp.path(), &[], None, &spec);
        let template = env
            .template_from_str(r#"{{ script("missing.sh") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for missing script in validate mode");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("missing.sh"),
            "error should name the missing file, got: {msg}"
        );
        assert!(
            msg.contains("not found"),
            "error should say 'not found', got: {msg}"
        );
    }

    #[test]
    fn test_script_allows_nested_path() {
        let spec = dummy_skill_spec_with_files(&["subdir/nested.sh"]);
        let out = render_body(
            r#"{{ script("subdir/nested.sh") }}"#,
            Some(Provider::Claude),
            &spec,
        );
        assert_eq!(out, "${CLAUDE_SKILL_DIR}/scripts/subdir/nested.sh");
        let out = render_body(
            r#"{{ script("subdir/nested.sh") }}"#,
            Some(Provider::Cursor),
            &spec,
        );
        assert_eq!(out, "scripts/subdir/nested.sh");
    }

    #[test]
    fn test_script_rejects_parent_traversal() {
        let tmp = tempfile::tempdir().expect("expected value");
        let env = build_environment(tmp.path(), &[], Some(Provider::Claude), &dummy_skill_spec());
        let template = env
            .template_from_str(r#"{{ script("../foo.sh") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for parent traversal");
        let msg = format!("{err:#}");
        assert!(msg.contains(".."), "error should mention '..', got: {msg}");
    }

    #[test]
    fn test_script_rejects_absolute_path() {
        let tmp = tempfile::tempdir().expect("expected value");
        let env = build_environment(tmp.path(), &[], Some(Provider::Claude), &dummy_skill_spec());
        let template = env
            .template_from_str(r#"{{ script("/etc/foo.sh") }}"#)
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected render error for absolute path");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("relative"),
            "error should mention 'relative', got: {msg}"
        );
    }

    #[test]
    fn test_tool_remains_registered_for_all_spec_types() {
        let tmp = tempfile::tempdir().expect("expected value");
        let agent = dummy_agent_spec();
        let skill = dummy_skill_spec();
        let rule = dummy_rule_spec();
        let hook = dummy_hook_spec();
        for spec in [&agent, &skill, &rule, &hook] {
            let env = build_environment(tmp.path(), &[], Some(Provider::Claude), spec);
            let template = env
                .template_from_str(r#"{{ tool("question") }}"#)
                .expect("expected value");
            let out = template
                .render(minijinja::context! {})
                .expect("expected value");
            assert_eq!(
                out, "AskUserQuestion",
                "tool() should resolve for all spec types"
            );
        }
    }

    #[test]
    fn test_extends_basic_block_override() {
        let files = HashMap::from([(
            "templates/base.md".to_string(),
            "Header\n{% block title %}Default Title{% endblock %}\nFooter".to_string(),
        )]);
        let out = render_with_templates(
            "{% extends \"templates/base.md\" %}{% block title %}My Title{% endblock %}",
            &files,
            None,
            &dummy_agent_spec(),
        )
        .expect("expected value");
        assert_eq!(out, "Header\nMy Title\nFooter");
    }

    #[test]
    fn test_extends_optional_block_default() {
        let files = HashMap::from([(
            "templates/base.md".to_string(),
            "Before\n{% block optional %}fallback content{% endblock %}\nAfter".to_string(),
        )]);
        let out = render_with_templates(
            "{% extends \"templates/base.md\" %}",
            &files,
            None,
            &dummy_agent_spec(),
        )
        .expect("expected value");
        assert_eq!(out, "Before\nfallback content\nAfter");
    }

    #[test]
    fn test_extends_with_fragment_include_in_block() {
        let files = HashMap::from([
            ("note.md".to_string(), "included note".to_string()),
            (
                "templates/base.md".to_string(),
                "Start\n{% block body %}default{% endblock %}\nEnd".to_string(),
            ),
        ]);
        let out = render_with_templates(
            "{% extends \"templates/base.md\" %}{% block body %}{% include \"note.md\" %}{% endblock %}",
            &files,
            None,
            &dummy_agent_spec(),
        )
        .expect("expected value");
        assert_eq!(out, "Start\nincluded note\nEnd");
    }

    #[test]
    fn test_extends_with_variable_threading() {
        let files = HashMap::from([
            ("frag.md".to_string(), "Hello, {{ subject }}!".to_string()),
            (
                "templates/base.md".to_string(),
                concat!(
                    "{% block greeting %}",
                    "{% with subject = \"default\" %}{% include \"frag.md\" %}{% endwith %}",
                    "{% endblock %}"
                )
                .to_string(),
            ),
        ]);
        let out = render_with_templates(
            concat!(
                "{% extends \"templates/base.md\" %}",
                "{% block greeting %}",
                "{% with subject = \"world\" %}{% include \"frag.md\" %}{% endwith %}",
                "{% endblock %}"
            ),
            &files,
            None,
            &dummy_agent_spec(),
        )
        .expect("expected value");
        assert_eq!(out, "Hello, world!");
    }

    #[test]
    fn test_extends_missing_template_errors() {
        let result = render_with_templates(
            "{% extends \"templates/nonexistent.md\" %}{% block x %}y{% endblock %}",
            &HashMap::new(),
            None,
            &dummy_agent_spec(),
        );
        assert!(result.is_err());
        let msg = result.expect_err("expected render error");
        assert!(
            msg.contains("nonexistent"),
            "expected template name in error, got: {msg}"
        );
    }

    #[test]
    fn test_extends_multi_level_chain() {
        let files = HashMap::from([
            (
                "templates/grandparent.md".to_string(),
                "GP-start\n{% block a %}gp-a{% endblock %}\n{% block b %}gp-b{% endblock %}\nGP-end"
                    .to_string(),
            ),
            (
                "templates/parent.md".to_string(),
                concat!(
                    "{% extends \"templates/grandparent.md\" %}",
                    "{% block a %}parent-a{% endblock %}"
                )
                .to_string(),
            ),
        ]);
        let out = render_with_templates(
            concat!(
                "{% extends \"templates/parent.md\" %}",
                "{% block b %}child-b{% endblock %}"
            ),
            &files,
            None,
            &dummy_agent_spec(),
        )
        .expect("expected value");
        assert_eq!(out, "GP-start\nparent-a\nchild-b\nGP-end");
    }

    #[test]
    fn test_extends_super_call() {
        let files = HashMap::from([(
            "templates/base.md".to_string(),
            "{% block content %}base content{% endblock %}".to_string(),
        )]);
        let out = render_with_templates(
            concat!(
                "{% extends \"templates/base.md\" %}",
                "{% block content %}{{ super() }} + child content{% endblock %}"
            ),
            &files,
            None,
            &dummy_agent_spec(),
        )
        .expect("expected value");
        assert_eq!(out, "base content + child content");
    }

    #[test]
    fn test_extends_script_function_works_in_derived_skill() {
        let files = HashMap::from([(
            "templates/skill-base.md".to_string(),
            "{% block run %}default{% endblock %}".to_string(),
        )]);
        let out = render_with_templates(
            concat!(
                "{% extends \"templates/skill-base.md\" %}",
                "{% block run %}{{ script(\"foo.sh\") }}{% endblock %}"
            ),
            &files,
            Some(Provider::Claude),
            &dummy_skill_spec(),
        )
        .expect("expected value");
        assert_eq!(out, "${CLAUDE_SKILL_DIR}/scripts/foo.sh");
    }

    #[test]
    fn test_extends_script_not_registered_for_derived_rule() {
        let files = HashMap::from([(
            "templates/base.md".to_string(),
            "{% block body %}default{% endblock %}".to_string(),
        )]);
        let result = render_with_templates(
            concat!(
                "{% extends \"templates/base.md\" %}",
                "{% block body %}{{ script(\"foo.sh\") }}{% endblock %}"
            ),
            &files,
            Some(Provider::Claude),
            &dummy_rule_spec(),
        );
        assert!(result.is_err());
        let msg = result.expect_err("expected render error");
        assert!(
            msg.contains("unknown"),
            "expected 'unknown' in error, got: {msg}"
        );
    }

    #[test]
    fn test_extends_tool_function_works_in_derived_spec() {
        let files = HashMap::from([(
            "templates/base.md".to_string(),
            "{% block body %}default{% endblock %}".to_string(),
        )]);
        let out = render_with_templates(
            concat!(
                "{% extends \"templates/base.md\" %}",
                "{% block body %}{{ tool(\"question\") }}{% endblock %}"
            ),
            &files,
            Some(Provider::Claude),
            &dummy_agent_spec(),
        )
        .expect("expected value");
        assert_eq!(out, "AskUserQuestion");
    }

    #[test]
    fn test_required_block_enforced() {
        let files = HashMap::from([(
            "templates/strict.md".to_string(),
            "Preamble\n{% block title required %}{% endblock %}\nEnd".to_string(),
        )]);
        let result = render_with_templates(
            "{% extends \"templates/strict.md\" %}",
            &files,
            None,
            &dummy_agent_spec(),
        );
        assert!(result.is_err());
        let msg = result.expect_err("expected render error");
        assert!(
            msg.contains("required"),
            "expected 'required' in error, got: {msg}"
        );
    }

    #[test]
    fn test_required_block_satisfied() {
        let files = HashMap::from([(
            "templates/strict.md".to_string(),
            "Preamble\n{% block title required %}{% endblock %}\nEnd".to_string(),
        )]);
        let out = render_with_templates(
            concat!(
                "{% extends \"templates/strict.md\" %}",
                "{% block title %}My Title{% endblock %}"
            ),
            &files,
            None,
            &dummy_agent_spec(),
        )
        .expect("expected value");
        assert_eq!(out, "Preamble\nMy Title\nEnd");
    }

    #[test]
    fn test_required_block_error_includes_spec_path() {
        let tmp = tempfile::tempdir().expect("expected value");
        std::fs::create_dir_all(tmp.path().join("templates")).expect("expected value");
        std::fs::write(
            tmp.path().join("templates/strict.md"),
            "{% block title required %}{% endblock %}",
        )
        .expect("expected value");

        let templating =
            crate::templating::Templating::from_sources(tmp.path().to_path_buf(), vec![]);
        let specs = vec![Spec::Agent(AgentSpec {
            path: tmp.path().join("skills/my-spec.md"),
            frontmatter: AgentFrontmatter {
                id: "test".to_string(),
                description: "test".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: "{% extends \"templates/strict.md\" %}".to_string(),
        })];

        let ctx = crate::templating::TemplateContext::from_specs(&[]);
        let err = crate::templating::resolve_fragments(specs, &templating, None, &ctx)
            .expect_err("expected error for missing required block");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("skills/my-spec.md"),
            "error should include spec path, got: {msg}"
        );
    }

    #[test]
    fn test_optional_and_required_blocks_mixed() {
        let files = HashMap::from([(
            "templates/mixed.md".to_string(),
            concat!(
                "{% block optional %}default content{% endblock %}\n",
                "{% block mandatory required %}{% endblock %}"
            )
            .to_string(),
        )]);
        let out = render_with_templates(
            concat!(
                "{% extends \"templates/mixed.md\" %}",
                "{% block mandatory %}filled in{% endblock %}"
            ),
            &files,
            None,
            &dummy_agent_spec(),
        )
        .expect("expected value");
        assert_eq!(out, "default content\nfilled in");
    }

    #[test]
    fn test_colocated_include_full_path() {
        let tmp = tempfile::tempdir().expect("expected value");
        std::fs::create_dir_all(tmp.path().join("skills/my-skill")).expect("expected value");
        std::fs::write(
            tmp.path().join("skills/my-skill/detail.md"),
            "colocated content",
        )
        .expect("expected value");

        let env = build_environment(tmp.path(), &[], None, &dummy_skill_spec());
        let template = env
            .template_from_str("{% include \"skills/my-skill/detail.md\" %}")
            .expect("expected value");
        let result = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(result, "colocated content");
    }

    #[test]
    fn test_colocated_include_self_relative() {
        let tmp = tempfile::tempdir().expect("expected value");
        std::fs::create_dir_all(tmp.path().join("skills/my-skill")).expect("expected value");
        std::fs::write(
            tmp.path().join("skills/my-skill/detail.md"),
            "self-relative content",
        )
        .expect("expected value");

        let env = build_environment(tmp.path(), &[], None, &dummy_skill_spec());
        let template = env
            .template_from_named_str("skills/my-skill/SKILL.md", "{% include \"./detail.md\" %}")
            .expect("expected value");
        let result = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(result, "self-relative content");
    }

    #[test]
    fn test_self_relative_nested() {
        let tmp = tempfile::tempdir().expect("expected value");
        std::fs::create_dir_all(tmp.path().join("skills/my-skill")).expect("expected value");
        std::fs::write(
            tmp.path().join("skills/my-skill/subsection.md"),
            "nested content",
        )
        .expect("expected value");
        std::fs::write(
            tmp.path().join("skills/my-skill/detail.md"),
            "detail: {% include \"./subsection.md\" %}",
        )
        .expect("expected value");

        let env = build_environment(tmp.path(), &[], None, &dummy_skill_spec());
        let template = env
            .template_from_named_str("skills/my-skill/SKILL.md", "{% include \"./detail.md\" %}")
            .expect("expected value");
        let result = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(result, "detail: nested content");
    }

    #[test]
    fn test_self_relative_with_subdirectory() {
        let tmp = tempfile::tempdir().expect("expected value");
        std::fs::create_dir_all(tmp.path().join("skills/my-skill/sub")).expect("expected value");
        std::fs::write(tmp.path().join("skills/my-skill/sub/part.md"), "sub part")
            .expect("expected value");

        let env = build_environment(tmp.path(), &[], None, &dummy_skill_spec());
        let template = env
            .template_from_named_str(
                "skills/my-skill/SKILL.md",
                "{% include \"./sub/part.md\" %}",
            )
            .expect("expected value");
        let result = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(result, "sub part");
    }

    #[test]
    fn test_non_self_relative_bypasses_path_join() {
        let tmp = tempfile::tempdir().expect("expected value");
        std::fs::create_dir_all(tmp.path().join("fragments")).expect("expected value");
        std::fs::write(tmp.path().join("fragments/shared.md"), "shared content")
            .expect("expected value");

        let env = build_environment(tmp.path(), &[], None, &dummy_skill_spec());
        let template = env
            .template_from_named_str(
                "skills/my-skill/SKILL.md",
                "{% include \"fragments/shared.md\" %}",
            )
            .expect("expected value");
        let result = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(result, "shared content");
    }

    #[test]
    fn test_extra_dir_resolution() {
        let tmp = tempfile::tempdir().expect("expected value");
        let extra = tmp.path().join("external");
        std::fs::create_dir_all(&extra).expect("expected value");
        std::fs::write(extra.join("note.md"), "extra dir content").expect("expected value");

        let extra_dirs = vec![super::ExtraIncludeDir {
            name: "ext".to_string(),
            path: extra,
        }];
        let env = build_environment(tmp.path(), &extra_dirs, None, &dummy_skill_spec());
        let template = env
            .template_from_str("{% include \"ext/note.md\" %}")
            .expect("expected value");
        let result = template
            .render(minijinja::context! {})
            .expect("expected value");
        assert_eq!(result, "extra dir content");
    }

    #[test]
    fn test_extra_dir_name_collision_with_spec_tree() {
        let tmp = tempfile::tempdir().expect("expected value");
        let extra = tmp.path().join("external");
        std::fs::create_dir_all(&extra).expect("expected value");
        std::fs::create_dir_all(tmp.path().join("skills")).expect("expected value");

        let extra_dirs = vec![super::ExtraIncludeDir {
            name: "skills".to_string(),
            path: extra,
        }];
        let err = crate::templating::Templating::new(tmp.path(), &extra_dirs)
            .expect_err("expected collision error");
        let msg = err.to_string();
        assert!(
            msg.contains("collides"),
            "error should mention collision: {msg}"
        );
    }

    #[test]
    fn test_duplicate_extra_dir_names() {
        let tmp = tempfile::tempdir().expect("expected value");
        let extra_a = tmp.path().join("a");
        let extra_b = tmp.path().join("b");
        std::fs::create_dir_all(&extra_a).expect("expected value");
        std::fs::create_dir_all(&extra_b).expect("expected value");

        let extra_dirs = vec![
            super::ExtraIncludeDir {
                name: "shared".to_string(),
                path: extra_a,
            },
            super::ExtraIncludeDir {
                name: "shared".to_string(),
                path: extra_b,
            },
        ];
        let err = crate::templating::Templating::new(tmp.path(), &extra_dirs)
            .expect_err("expected duplicate error");
        let msg = err.to_string();
        assert!(
            msg.contains("duplicate"),
            "error should mention duplicate: {msg}"
        );
    }

    #[test]
    fn test_empty_extra_dir_name() {
        let tmp = tempfile::tempdir().expect("expected value");
        let extra = tmp.path().join("external");
        std::fs::create_dir_all(&extra).expect("expected value");

        let extra_dirs = vec![super::ExtraIncludeDir {
            name: String::new(),
            path: extra,
        }];
        let err = crate::templating::Templating::new(tmp.path(), &extra_dirs)
            .expect_err("expected empty name error");
        let msg = err.to_string();
        assert!(
            msg.contains("empty or whitespace"),
            "error should mention empty: {msg}"
        );
    }

    #[test]
    fn test_whitespace_only_extra_dir_name() {
        let tmp = tempfile::tempdir().expect("expected value");
        let extra = tmp.path().join("external");
        std::fs::create_dir_all(&extra).expect("expected value");

        let extra_dirs = vec![super::ExtraIncludeDir {
            name: "  ".to_string(),
            path: extra,
        }];
        let err = crate::templating::Templating::new(tmp.path(), &extra_dirs)
            .expect_err("expected whitespace name error");
        let msg = err.to_string();
        assert!(
            msg.contains("empty or whitespace"),
            "error should mention whitespace: {msg}"
        );
    }

    #[test]
    fn test_slash_in_extra_dir_name() {
        let tmp = tempfile::tempdir().expect("expected value");
        let extra = tmp.path().join("external");
        std::fs::create_dir_all(&extra).expect("expected value");

        let extra_dirs = vec![super::ExtraIncludeDir {
            name: "a/b".to_string(),
            path: extra,
        }];
        let err = crate::templating::Templating::new(tmp.path(), &extra_dirs)
            .expect_err("expected slash error");
        let msg = err.to_string();
        assert!(
            msg.contains("path separators"),
            "error should mention separators: {msg}"
        );
    }

    #[test]
    fn test_dotdot_extra_dir_name() {
        let tmp = tempfile::tempdir().expect("expected value");
        let extra = tmp.path().join("external");
        std::fs::create_dir_all(&extra).expect("expected value");

        let extra_dirs = vec![super::ExtraIncludeDir {
            name: "..".to_string(),
            path: extra,
        }];
        let err = crate::templating::Templating::new(tmp.path(), &extra_dirs)
            .expect_err("expected dotdot error");
        let msg = err.to_string();
        assert!(msg.contains("must not be \".\" or \"..\""), "error: {msg}");
    }

    #[test]
    fn test_dot_extra_dir_name() {
        let tmp = tempfile::tempdir().expect("expected value");
        let extra = tmp.path().join("external");
        std::fs::create_dir_all(&extra).expect("expected value");

        let extra_dirs = vec![super::ExtraIncludeDir {
            name: ".".to_string(),
            path: extra,
        }];
        let err = crate::templating::Templating::new(tmp.path(), &extra_dirs)
            .expect_err("expected dot error");
        let msg = err.to_string();
        assert!(msg.contains("must not be \".\" or \"..\""), "error: {msg}");
    }

    #[test]
    fn test_dotdot_substring_in_extra_dir_name_allowed() {
        let tmp = tempfile::tempdir().expect("expected value");
        let extra = tmp.path().join("external");
        std::fs::create_dir_all(&extra).expect("expected value");

        let extra_dirs = vec![super::ExtraIncludeDir {
            name: "foo..bar".to_string(),
            path: extra,
        }];
        crate::templating::Templating::new(tmp.path(), &extra_dirs)
            .expect("foo..bar should be accepted — not a path traversal");
    }

    #[test]
    fn test_missing_extra_dir_path() {
        let tmp = tempfile::tempdir().expect("expected value");
        let extra_dirs = vec![super::ExtraIncludeDir {
            name: "missing".to_string(),
            path: tmp.path().join("nonexistent"),
        }];
        let err = crate::templating::Templating::new(tmp.path(), &extra_dirs)
            .expect_err("expected missing dir error");
        let msg = err.to_string();
        assert!(
            msg.contains("does not exist"),
            "error should mention missing: {msg}"
        );
    }

    #[test]
    fn test_path_traversal_rejection() {
        let result = resolve_include("../secret.md", Path::new("/tmp"), &[]);
        let err = result.expect_err("expected traversal error");
        let msg = err.to_string();
        assert!(msg.contains("parent directory traversal"), "error: {msg}");
    }

    #[test]
    fn test_self_relative_parent_traversal_rejection() {
        let tmp = tempfile::tempdir().expect("expected value");
        std::fs::create_dir_all(tmp.path().join("skills/my-skill")).expect("expected value");

        let env = build_environment(tmp.path(), &[], None, &dummy_skill_spec());
        let template = env
            .template_from_named_str("skills/my-skill/SKILL.md", "{% include \"../secret.md\" %}")
            .expect("expected value");
        let err = template
            .render(minijinja::context! {})
            .expect_err("expected error for ../ include");
        let msg = format!("{err:#}");
        assert!(msg.contains("parent directory traversal"), "error: {msg}");
    }

    #[test]
    fn test_absolute_path_rejection() {
        let result = resolve_include("/etc/passwd", Path::new("/tmp"), &[]);
        assert!(
            result.expect("should return Ok").is_none(),
            "absolute path should be silently rejected"
        );
    }

    #[test]
    fn test_non_md_extension_rejection() {
        let result = resolve_include("data.json", Path::new("/tmp"), &[]);
        let err = result.expect_err("expected extension error");
        let msg = err.to_string();
        assert!(msg.contains("must use .md extension"), "error: {msg}");
    }

    #[test]
    fn test_symlink_containment() {
        let tmp = tempfile::tempdir().expect("expected value");
        let outside = tempfile::tempdir().expect("expected value");
        std::fs::write(outside.path().join("secret.md"), "secret").expect("expected value");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                outside.path().join("secret.md"),
                tmp.path().join("escape.md"),
            )
            .expect("expected value");

            let result = resolve_include("escape.md", tmp.path(), &[]).expect("should return Ok");
            assert!(
                result.is_none(),
                "symlink escape should be silently rejected"
            );
        }
    }
}
