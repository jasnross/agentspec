use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use globset::GlobBuilder;

use crate::presets::ProviderPresetsMap;
use crate::spec::Spec;

/// A validation error (spec semantics or config shape).
#[derive(Debug)]
pub struct ValidationError {
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for ValidationError {}

/// Run semantic validation checks on loaded specs.
///
/// Returns all errors found. An empty vec means all checks pass.
/// This function does no I/O and cannot fail structurally.
///
/// `config_path` is where `agentspec.toml` was discovered; preset errors are
/// reported against it. The library cannot derive it — `AgentspecConfig::discover`
/// walks up parent directories, so a bare `"agentspec.toml"` would name a file
/// that need not exist in the caller's cwd.
pub fn validate_semantics(
    specs: &[Spec],
    presets: &ProviderPresetsMap,
    config_path: &Path,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let mut id_set = std::collections::HashSet::new();

    for spec in specs {
        // Duplicate ID check
        if !id_set.insert(spec.id()) {
            errors.push(ValidationError {
                path: spec.path().to_path_buf(),
                message: format!("duplicate id '{}'", spec.id()),
            });
        }

        // Empty body check
        // Hook specs intentionally have empty bodies (they are TOML-driven, not
        // markdown-bodied), so they are exempt from this check.
        if spec.body().is_empty() && !matches!(spec, Spec::Hook(_)) {
            errors.push(ValidationError {
                path: spec.path().to_path_buf(),
                message: "instruction body cannot be empty".to_string(),
            });
        }

        // Skill invocability check
        if let Spec::Skill(skill_spec) = spec
            && !skill_spec.frontmatter.user_invocable
            && !skill_spec.frontmatter.agent_invocable
        {
            errors.push(ValidationError {
                path: skill_spec.path.clone(),
                message: "at least one of user_invocable or agent_invocable must be true"
                    .to_string(),
            });
        }

        if let Spec::Hook(hook_spec) = spec
            && hook_spec.frontmatter.matcher.is_some()
        {
            validate_hook_matcher(hook_spec, &mut errors);
        }

        if let Spec::Hook(hook_spec) = spec
            && hook_spec.frontmatter.args.is_some()
        {
            validate_hook_args(hook_spec, &mut errors);
        }

        if let Spec::Rule(rule_spec) = spec
            && let Some(paths) = &rule_spec.frontmatter.paths
        {
            validate_rule_paths(&rule_spec.path, paths, &mut errors);
        }

        let execution = match spec {
            Spec::Agent(agent_spec) => &agent_spec.frontmatter.execution,
            Spec::Skill(skill_spec) => &skill_spec.frontmatter.execution,
            Spec::Rule(_) | Spec::Hook(_) => &None,
        };

        // Preset validation (skip if no presets loaded)
        if let Some(preset_name) = execution.as_ref().and_then(|x| x.preset.as_ref()) {
            match presets.get(preset_name) {
                Some(_) => (),
                None => {
                    errors.push(ValidationError {
                        path: spec.path().to_path_buf(),
                        message: format!("unknown preset '{preset_name}'"),
                    });
                }
            }
        }
    }

    // Per-type underscore-normalization collision check.
    // Keyed template access normalizes hyphens to underscores, so IDs that
    // differ only in hyphen/underscore placement would collide within the same map.
    // Key: (spec_type, normalized_id) → list of (original_id, path).
    let mut normalized_groups: HashMap<(&str, String), Vec<(&str, &Path)>> = HashMap::new();
    for spec in specs {
        let normalized = spec.id().replace('-', "_");
        normalized_groups
            .entry((spec.spec_type(), normalized))
            .or_default()
            .push((spec.id(), spec.path()));
    }
    for ((spec_type, normalized), entries) in &normalized_groups {
        if entries.len() > 1 {
            let names: Vec<&str> = entries.iter().map(|(id, _)| *id).collect();
            // Skip if all entries share the same original ID — that case is
            // already caught by the duplicate-ID check above.
            if names.iter().all(|n| *n == names[0]) {
                continue;
            }
            for (_, path) in entries {
                errors.push(ValidationError {
                    path: path.to_path_buf(),
                    message: format!(
                        "{spec_type} IDs {} all normalize to '{normalized}' \
                         (hyphens \u{2192} underscores) and would collide in template keyed access",
                        names
                            .iter()
                            .map(|n| format!("'{n}'"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
            }
        }
    }

    errors.extend(validate_preset_config(presets, config_path));
    errors
}

/// Cross-field checks on the provider blocks inside every `[presets.<name>]`.
///
/// Lives here, in the library, rather than beside the binary's other config
/// checks: `compile::run` is public API, so a consumer that never touches the
/// CLI can still reach the Cursor adapter's bracket composition. `ValidatedSpecs`
/// carries the map this ran against and `compile::run` reads presets from there,
/// so on that path a caller cannot validate one map and compile with another.
///
/// That guarantee covers `compile::run`, not every route to an adapter.
/// `Provider::adapter()`, `Adapter::compile`, and `CompileCtx`'s fields are all
/// public, so a consumer calling an adapter directly supplies its own presets
/// and is guarded only by `debug_assert!`s, which compile out in release. The
/// composition trusts this gate rather than re-checking.
///
/// Presets are keyed by a `HashMap`, so iteration order is nondeterministic and
/// a multi-error run would report differently each time. Sort by preset name
/// first. At most one error per preset — each provider's `validate` reports its
/// first failing check.
fn validate_preset_config(
    presets: &ProviderPresetsMap,
    config_path: &Path,
) -> Vec<ValidationError> {
    let mut names: Vec<&String> = presets.keys().collect();
    names.sort();

    names
        .into_iter()
        .filter_map(|name| {
            presets
                .get(name)?
                .validate(name)
                .err()
                .map(|e| ValidationError {
                    path: config_path.to_path_buf(),
                    message: e.to_string(),
                })
        })
        .collect()
}

fn validate_hook_matcher(hook_spec: &crate::spec::HookSpec, errors: &mut Vec<ValidationError>) {
    let bad_events: Vec<_> = hook_spec
        .frontmatter
        .events
        .iter()
        .filter(|e| !e.allows_matcher())
        .collect();
    if !bad_events.is_empty() {
        errors.push(ValidationError {
            path: hook_spec.path.clone(),
            message: format!(
                "hook '{}' sets `matcher` but targets event(s) that do not accept one: {}; \
                 only pre_tool_use, post_tool_use, post_tool_use_failure, \
                 subagent_start, and subagent_stop may use a matcher",
                hook_spec.frontmatter.id,
                bad_events
                    .iter()
                    .map(|e| e.snake_case())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        });
    }
}

/// Reject a NUL byte in any `args` entry.
///
/// `sh_quote`'s single-quote escaping is otherwise unconditional — every
/// other byte a shell can represent survives quoting intact — but `\0`
/// terminates a C string at the OS `exec` layer, before the shell ever
/// sees it. An author's argument would silently truncate at the null
/// rather than reach the script, with a clean `agentspec validate` and no
/// diagnostic anywhere downstream. Other control characters (newline,
/// tab) round-trip through the shell byte-exact and are not restricted.
fn validate_hook_args(hook_spec: &crate::spec::HookSpec, errors: &mut Vec<ValidationError>) {
    let Some(args) = &hook_spec.frontmatter.args else {
        return;
    };
    for (i, arg) in args.iter().enumerate() {
        if arg.contains('\0') {
            errors.push(ValidationError {
                path: hook_spec.path.clone(),
                message: format!(
                    "hook '{}' args[{i}] contains a NUL byte, which cannot survive to the \
                     shell-invoked script",
                    hook_spec.frontmatter.id,
                ),
            });
        }
    }
}

fn validate_rule_paths(
    path: &std::path::Path,
    paths: &[String],
    errors: &mut Vec<ValidationError>,
) {
    if paths.is_empty() {
        errors.push(ValidationError {
            path: path.to_path_buf(),
            message: "paths must contain at least one pattern when specified".to_string(),
        });
        return;
    }
    for pat in paths {
        if let Err(err) = GlobBuilder::new(pat).literal_separator(true).build() {
            errors.push(ValidationError {
                path: path.to_path_buf(),
                message: format!("invalid glob pattern '{pat}': {err}"),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use indexmap::IndexMap;

    use super::*;
    use crate::presets::{ProviderPresets, ProviderPresetsMap};
    use crate::spec::{
        AgentFrontmatter, AgentSpec, ExecutionFrontmatter, HookEvent, HookFrontmatter, HookSpec,
        RuleFrontmatter, RuleSpec, SkillFrontmatter, SkillSpec,
    };

    // -- Helpers --

    /// Stand-in for the discovered `agentspec.toml`. Preset errors are reported
    /// against it; no test here asserts on the path itself.
    fn test_config_path() -> &'static Path {
        Path::new("/tmp/agentspec.toml")
    }

    fn make_agent(id: &str, body: &str) -> Spec {
        Spec::Agent(AgentSpec {
            path: PathBuf::from(format!("{id}.md")),
            frontmatter: AgentFrontmatter {
                id: id.to_string(),
                description: "test description".to_string(),
                tags: None,
                execution: None,
                capabilities: None,
            },
            body: body.to_string(),
        })
    }

    fn make_skill(id: &str, body: &str) -> Spec {
        Spec::Skill(SkillSpec {
            path: PathBuf::from(format!("{id}.md")),
            frontmatter: SkillFrontmatter {
                id: id.to_string(),
                description: None,
                tags: None,
                user_invocable: true,
                agent_invocable: false,
                execution: None,
                capabilities: None,
            },
            body: body.to_string(),
            supporting_files: IndexMap::new(),
        })
    }

    fn make_rule(id: &str, body: &str) -> Spec {
        make_rule_with_paths(id, body, None)
    }

    fn make_rule_with_paths(id: &str, body: &str, paths: Option<Vec<String>>) -> Spec {
        Spec::Rule(RuleSpec {
            path: PathBuf::from(format!("{id}.md")),
            frontmatter: RuleFrontmatter {
                id: id.to_string(),
                description: None,
                tags: None,
                paths,
            },
            body: body.to_string(),
        })
    }

    fn make_hook(id: &str, events: Vec<HookEvent>, matcher: Option<&str>) -> Spec {
        make_hook_with_args(id, events, matcher, None)
    }

    fn make_hook_with_args(
        id: &str,
        events: Vec<HookEvent>,
        matcher: Option<&str>,
        args: Option<Vec<String>>,
    ) -> Spec {
        Spec::Hook(HookSpec {
            path: PathBuf::from("hooks.toml"),
            frontmatter: HookFrontmatter {
                id: id.to_string(),
                events,
                script: PathBuf::from(format!("scripts/{id}.sh")),
                matcher: matcher.map(str::to_string),
                timeout: None,
                description: None,
                tags: None,
                args,
            },
            body: String::new(),
            supporting_files: IndexMap::new(),
        })
    }

    // -- Semantic validation tests --

    #[test]
    fn test_semantics_clean() {
        let specs = vec![make_agent("alpha", "body"), make_skill("beta", "body")];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
    }

    #[test]
    fn test_semantics_duplicate_id() {
        let specs = vec![make_agent("dup", "body a"), make_agent("dup", "body b")];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("duplicate id 'dup'"));
    }

    #[test]
    fn test_semantics_empty_body() {
        let specs = vec![make_agent("empty", "")];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("body cannot be empty"))
        );
    }

    #[test]
    fn test_semantics_skill_not_invocable() {
        let mut spec = make_skill("no-invoke", "body");
        let Spec::Skill(ref mut s) = spec else {
            panic!("expected Skill variant")
        };
        s.frontmatter.user_invocable = false;
        s.frontmatter.agent_invocable = false;
        let errors = validate_semantics(&[spec], &ProviderPresetsMap::new(), test_config_path());
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("user_invocable or agent_invocable"))
        );
    }

    #[test]
    fn test_semantics_unknown_preset() {
        let mut spec = make_agent("unknown", "body");
        let Spec::Agent(ref mut s) = spec else {
            panic!("expected Agent variant")
        };
        s.frontmatter.execution = Some(ExecutionFrontmatter {
            preset: Some("nonexistent".to_string()),
        });
        let mut presets = ProviderPresetsMap::new();
        presets.insert("known".to_string(), ProviderPresets::default());
        let errors = validate_semantics(&[spec], &presets, test_config_path());
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("unknown preset 'nonexistent'"))
        );
    }

    #[test]
    fn test_semantics_known_preset_no_error() {
        let mut spec = make_agent("known", "body");
        let Spec::Agent(ref mut s) = spec else {
            panic!("expected Agent variant")
        };
        s.frontmatter.execution = Some(ExecutionFrontmatter {
            preset: Some("fast".to_string()),
        });
        let mut presets = ProviderPresetsMap::new();
        presets.insert("fast".to_string(), ProviderPresets::default());
        let errors = validate_semantics(&[spec], &presets, test_config_path());
        assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
    }

    #[test]
    fn test_semantics_rule_passes_all_checks() {
        let specs = vec![make_rule("my-rule", "body")];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
    }

    // -- Underscore-normalization collision tests --

    #[test]
    fn test_semantics_underscore_collision_same_type() {
        let specs = vec![
            make_skill("gh-safe", "body one"),
            make_skill("gh_safe", "body two"),
        ];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        let collision_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.message.contains("normalize"))
            .collect();
        assert_eq!(
            collision_errors.len(),
            2,
            "expected 2 collision errors (one per spec), got: {collision_errors:?}"
        );
        assert!(collision_errors[0].message.contains("'gh-safe'"));
        assert!(collision_errors[0].message.contains("'gh_safe'"));
        assert!(collision_errors[0].message.contains("skill"));
    }

    #[test]
    fn test_semantics_underscore_collision_cross_type_no_error() {
        let specs = vec![
            make_agent("gh-safe", "body one"),
            make_skill("gh_safe", "body two"),
        ];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        let collision_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.message.contains("normalize"))
            .collect();
        assert!(
            collision_errors.is_empty(),
            "cross-type collision should not error, got: {collision_errors:?}"
        );
    }

    #[test]
    fn test_semantics_underscore_no_collision_single_spec() {
        let specs = vec![make_skill("foo-bar", "body")];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        let collision_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.message.contains("normalize"))
            .collect();
        assert!(
            collision_errors.is_empty(),
            "single spec should not collide, got: {collision_errors:?}"
        );
    }

    // -- Hook validation tests --

    #[test]
    fn test_hook_empty_body_does_not_error() {
        let specs = vec![make_hook("init", vec![HookEvent::SessionStart], None)];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        assert!(
            errors.is_empty(),
            "expected no errors for hook with empty body, got: {errors:?}"
        );
    }

    #[test]
    fn test_hook_matcher_on_tool_event_passes() {
        let specs = vec![make_hook(
            "audit",
            vec![HookEvent::PreToolUse],
            Some("Bash|Edit"),
        )];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        assert!(
            errors.is_empty(),
            "expected no errors for matcher on pre_tool_use, got: {errors:?}"
        );
    }

    #[test]
    fn test_hook_matcher_on_non_tool_event_errors() {
        let specs = vec![make_hook(
            "init",
            vec![HookEvent::SessionStart],
            Some("anything"),
        )];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        let matcher_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.message.contains("do not accept one"))
            .collect();
        assert_eq!(
            matcher_errors.len(),
            1,
            "expected exactly one matcher error, got all: {errors:?}"
        );
        assert!(matcher_errors[0].message.contains("'init'"));
        assert!(matcher_errors[0].message.contains("session_start"));
    }

    #[test]
    fn test_hook_matcher_on_mixed_events_errors() {
        let specs = vec![make_hook(
            "mixed",
            vec![HookEvent::PreToolUse, HookEvent::SessionStart],
            Some("Edit"),
        )];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        let matcher_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.message.contains("do not accept one"))
            .collect();
        assert_eq!(
            matcher_errors.len(),
            1,
            "expected one error for mixed events with matcher, got: {errors:?}"
        );
        assert!(matcher_errors[0].message.contains("session_start"));
        assert!(
            matcher_errors[0]
                .message
                .starts_with("hook 'mixed' sets `matcher` but targets event(s) that do not accept one: session_start"),
            "error should list only the offending event, got: {}",
            matcher_errors[0].message
        );
    }

    #[test]
    fn test_hook_matcher_on_all_tool_events_passes() {
        let specs = vec![make_hook(
            "audit",
            vec![HookEvent::PreToolUse, HookEvent::PostToolUse],
            Some("Bash"),
        )];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        assert!(
            errors.is_empty(),
            "expected no errors for matcher on all-tool-events, got: {errors:?}"
        );
    }

    #[test]
    fn test_hook_matcher_on_subagent_start_passes() {
        let specs = vec![make_hook(
            "subagent-audit",
            vec![HookEvent::SubagentStart],
            Some("general"),
        )];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        assert!(
            errors.is_empty(),
            "expected no errors for matcher on subagent_start, got: {errors:?}"
        );
    }

    #[test]
    fn test_hook_matcher_on_subagent_stop_passes() {
        let specs = vec![make_hook(
            "subagent-audit",
            vec![HookEvent::SubagentStop],
            Some("general"),
        )];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        assert!(
            errors.is_empty(),
            "expected no errors for matcher on subagent_stop, got: {errors:?}"
        );
    }

    #[test]
    fn test_hook_args_with_nul_byte_errors() {
        let specs = vec![make_hook_with_args(
            "audit",
            vec![HookEvent::PreToolUse],
            None,
            Some(vec!["a\0b".to_string()]),
        )];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        assert_eq!(errors.len(), 1, "expected one error, got: {errors:?}");
        assert!(errors[0].message.contains("NUL byte"));
        assert!(errors[0].message.contains("args[0]"));
    }

    #[test]
    fn test_hook_args_with_other_control_chars_passes() {
        // Newline and tab round-trip through the shell byte-exact (unlike
        // NUL, which terminates a C string before the shell ever sees it),
        // so the ban is narrow — only NUL is rejected.
        let specs = vec![make_hook_with_args(
            "audit",
            vec![HookEvent::PreToolUse],
            None,
            Some(vec!["line1\nline2".to_string(), "tab\there".to_string()]),
        )];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        assert!(
            errors.is_empty(),
            "expected no errors for newline/tab args, got: {errors:?}"
        );
    }

    #[test]
    fn test_hook_id_collides_with_skill_id() {
        let specs = vec![
            make_skill("gh-safe", "body"),
            make_hook("gh-safe", vec![HookEvent::SessionStart], None),
        ];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("duplicate id 'gh-safe'")),
            "expected cross-spec-type duplicate-ID error, got: {errors:?}"
        );
    }

    #[test]
    fn test_hook_duplicate_ids_within_hooks_error() {
        let specs = vec![
            make_hook("init", vec![HookEvent::SessionStart], None),
            make_hook("init", vec![HookEvent::SessionEnd], None),
        ];
        let errors = validate_semantics(&specs, &ProviderPresetsMap::new(), test_config_path());
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("duplicate id 'init'")),
            "expected duplicate-id error, got: {errors:?}"
        );
    }

    #[test]
    fn test_semantics_rule_paths_empty_rejected() {
        let spec = make_rule_with_paths("my-rule", "body", Some(vec![]));
        let errors = validate_semantics(&[spec], &ProviderPresetsMap::new(), test_config_path());
        assert!(
            errors.iter().any(|e| e
                .message
                .contains("paths must contain at least one pattern")),
            "expected empty-paths error, got: {errors:?}"
        );
    }

    #[test]
    fn test_semantics_rule_paths_invalid_glob_rejected() {
        let spec = make_rule_with_paths("my-rule", "body", Some(vec!["[unterminated".to_string()]));
        let errors = validate_semantics(&[spec], &ProviderPresetsMap::new(), test_config_path());
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("invalid glob pattern '[unterminated'")),
            "expected invalid-glob error, got: {errors:?}"
        );
    }

    #[test]
    fn test_semantics_rule_paths_valid_passes() {
        let spec = make_rule_with_paths(
            "my-rule",
            "body",
            Some(vec![
                "src/components/**/*.tsx".to_string(),
                "src/hooks/**/*.ts".to_string(),
            ]),
        );
        let errors = validate_semantics(&[spec], &ProviderPresetsMap::new(), test_config_path());
        assert!(
            errors.is_empty(),
            "expected no errors for valid paths, got: {errors:?}"
        );
    }
}

#[cfg(test)]
mod preset_config_tests {
    use std::collections::HashMap;

    use super::*;
    use crate::presets::{CursorPreset, ProviderPresets, ProviderPresetsMap};

    fn presets_with_cursor(name: &str, cursor: CursorPreset) -> ProviderPresetsMap {
        HashMap::from([(
            name.to_string(),
            ProviderPresets {
                claude: None,
                cursor: Some(cursor),
                opencode: None,
            },
        )])
    }

    #[test]
    fn test_rejects_bracketed_cursor_model() {
        let presets = presets_with_cursor(
            "x",
            CursorPreset {
                model: Some("claude-opus-5[effort=high]".to_string()),
                ..CursorPreset::default()
            },
        );
        let errors = validate_preset_config(&presets, Path::new("/tmp/agentspec.toml"));
        assert_eq!(errors.len(), 1, "errors: {errors:?}");
        assert!(
            errors[0].message.contains("bare model id"),
            "error: {}",
            errors[0].message
        );
    }

    /// The gate must reach spec compilation, so it lives in `validate_semantics`
    /// rather than beside the binary's other config checks — a library consumer
    /// of `compile_specs` is gated by the same call.
    #[test]
    fn test_validate_semantics_surfaces_preset_errors_with_no_specs() {
        let presets = presets_with_cursor(
            "x",
            CursorPreset {
                model: Some("claude-opus-5[effort=high]".to_string()),
                ..CursorPreset::default()
            },
        );
        let errors = validate_semantics(&[], &presets, Path::new("/tmp/agentspec.toml"));
        assert_eq!(errors.len(), 1, "errors: {errors:?}");
    }

    #[test]
    fn test_rejects_delimiter_in_option_value() {
        for (field, preset) in [
            (
                "effort",
                CursorPreset {
                    model: Some("claude-opus-5".to_string()),
                    effort: Some("high,context=1m".to_string()),
                    ..CursorPreset::default()
                },
            ),
            (
                "context",
                CursorPreset {
                    model: Some("claude-opus-5".to_string()),
                    context: Some("300k]evil".to_string()),
                    ..CursorPreset::default()
                },
            ),
        ] {
            let errors = validate_preset_config(
                &presets_with_cursor("x", preset),
                Path::new("/tmp/agentspec.toml"),
            );
            assert_eq!(errors.len(), 1, "{field}: {errors:?}");
            assert!(
                errors[0].message.contains(field),
                "{field}: {}",
                errors[0].message
            );
        }
    }

    #[test]
    fn test_accepts_well_formed_preset() {
        let presets = presets_with_cursor(
            "x",
            CursorPreset {
                model: Some("claude-opus-5".to_string()),
                effort: Some("high".to_string()),
                fast: Some(false),
                context: Some("300k".to_string()),
                params: std::collections::BTreeMap::from([(
                    "optimize_for".to_string(),
                    "cost".to_string(),
                )]),
            },
        );
        assert!(validate_preset_config(&presets, Path::new("/tmp/agentspec.toml")).is_empty());
    }

    fn bad(model: &str) -> ProviderPresets {
        ProviderPresets {
            claude: None,
            cursor: Some(CursorPreset {
                model: Some(model.to_string()),
                ..CursorPreset::default()
            }),
            opencode: None,
        }
    }

    /// `presets` is a `HashMap`, so without the sort a multi-error run would
    /// report in a different order each time. Two failing presets pin it.
    #[test]
    fn test_errors_are_ordered_by_preset_name() {
        let presets: ProviderPresetsMap = HashMap::from([
            ("zebra".to_string(), bad("m[effort=high]")),
            ("alpha".to_string(), bad("m[effort=high]")),
            ("middle".to_string(), bad("m[effort=high]")),
        ]);

        let errors = validate_preset_config(&presets, Path::new("/tmp/agentspec.toml"));
        assert_eq!(errors.len(), 3, "errors: {errors:?}");

        let order: Vec<&str> = errors
            .iter()
            .map(|e| {
                if e.message.contains("presets.alpha") {
                    "alpha"
                } else if e.message.contains("presets.middle") {
                    "middle"
                } else {
                    "zebra"
                }
            })
            .collect();
        assert_eq!(order, ["alpha", "middle", "zebra"]);
    }
}
