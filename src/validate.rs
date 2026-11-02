use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};

use globset::GlobBuilder;
use strum::VariantArray as _;

use crate::declarations::Declarations;
use crate::mcp::{McpServers, McpToolName, is_mcp_name};
use crate::plan::FileKind;
use crate::provider::Provider;
use crate::setting::SettingKind;
use crate::spec::{McpGrant, McpTools, Spec, ToolFrontmatter};

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
/// `config_path` is where `agentspec.toml` was discovered; declaration errors are
/// reported against it. The library cannot derive it — `AgentspecConfig::discover`
/// walks up parent directories, so a bare `"agentspec.toml"` would name a file
/// that need not exist in the caller's cwd.
pub fn validate_semantics(
    specs: &[Spec],
    declarations: &Declarations,
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

        validate_capabilities(spec, &declarations.mcp, &mut errors);

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
            match declarations.presets.get(preset_name) {
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

    errors.extend(validate_config_declarations(declarations, config_path));
    errors
}

/// The provider-neutral rule on every declared MCP server name, then the
/// provider-neutral rules on every provider block's `tool-names`, then each
/// adapter's checks on its own provider's blocks in every declaration section,
/// all reported against the config file.
///
/// Lives here, in the library, rather than beside the binary's other config
/// checks: `compile::run` is public API, so a consumer that never touches the
/// CLI can still reach an adapter's composition. `ValidatedSpecs` carries the
/// declarations this ran against and `compile::run` reads them from there, so
/// on that path a caller cannot validate one set and compile with another.
///
/// That guarantee covers `compile::run`, not every route to an adapter.
/// `Provider::adapter()`, `Adapter::compile`, and `CompileCtx`'s fields are all
/// public, so a consumer calling an adapter directly supplies its own presets
/// and is guarded only by `debug_assert!`s, which compile out in release. The
/// composition trusts this gate rather than re-checking.
///
/// Providers are visited in `Provider::VARIANTS` order, and each adapter
/// orders its own messages, so a multi-error run reports the same way every
/// time.
fn validate_config_declarations(
    declarations: &Declarations,
    config_path: &Path,
) -> Vec<ValidationError> {
    // The neutral rule on every logical name comes first; adapters check only
    // the overrides they can see, so one bad name yields one error.
    let neutral = declarations
        .mcp
        .keys()
        .filter(|name| !is_mcp_name(name))
        .map(|name| format!("[mcp.{name}] the server name must match [A-Za-z0-9_-]+"));
    neutral
        .chain(tool_names_errors(&declarations.mcp))
        .chain(
            Provider::VARIANTS
                .iter()
                .flat_map(|provider| provider.adapter().validate_declarations(declarations)),
        )
        .map(|message| ValidationError {
            path: config_path.to_path_buf(),
            message,
        })
        .collect()
}

/// The provider-neutral rule on each provider block's `tool-names`: every
/// key is an MCP name, and every value is either a real name, which must be
/// an MCP name too, or `false`.
///
/// Several keys may share a value, for a server whose one tool serves several
/// logical tools; Claude and `OpenCode` grant that tool once. Servers are
/// visited in name order, providers in `Provider::VARIANTS` order, and
/// entries in key order.
fn tool_names_errors(mcp: &McpServers) -> Vec<String> {
    let mut errors = Vec::new();
    for (name, server) in mcp {
        for provider in Provider::VARIANTS {
            let Some(names) = provider.adapter().mcp_tool_names(server) else {
                continue;
            };
            let block = format!("[mcp.{name}.{provider}] `tool-names`");
            for (key, value) in names {
                if !is_mcp_name(key) {
                    errors.push(format!("{block} key '{key}' must match [A-Za-z0-9_-]+"));
                }
                if let McpToolName::Real(value) = value
                    && !is_mcp_name(value)
                {
                    errors.push(format!(
                        "{block} maps '{key}' to '{value}', which must match [A-Za-z0-9_-]+"
                    ));
                }
            }
        }
    }
    errors
}

/// Check an agent's or skill's `capabilities`: the built-in tool list, each
/// MCP grant, and that an agent granting MCP tools also declares its built-in
/// tools.
fn validate_capabilities(spec: &Spec, servers: &McpServers, errors: &mut Vec<ValidationError>) {
    let capabilities = match spec {
        Spec::Agent(agent_spec) => agent_spec.frontmatter.capabilities.as_ref(),
        Spec::Skill(skill_spec) => skill_spec.frontmatter.capabilities.as_ref(),
        Spec::Rule(_) | Spec::Hook(_) => None,
    };
    if let Some(tools) = capabilities.and_then(|c| c.tools.as_deref()) {
        validate_unique_tools(spec.path(), tools, errors);
    }

    let Some(grants) = spec.mcp_grants() else {
        return;
    };
    validate_mcp_grants(spec.path(), grants, servers, errors);
    validate_mcp_tool_coverage(spec, grants, servers, errors);
    if matches!(spec, Spec::Agent(_)) && !spec.declares_tools() {
        errors.push(ValidationError {
            path: spec.path().to_path_buf(),
            message: "an agent that declares `capabilities.mcp` must declare \
                      `capabilities.tools`; use `tools: []` to grant only the MCP \
                      tools, or remove `mcp` to give the agent every tool"
                .to_string(),
        });
    }
}

/// Reject a `capabilities.tools` list that names a tool twice.
///
/// A repeat usually stands where the author meant a different tool, and it
/// otherwise compiles without complaint. One error per repeated tool, in the
/// order each first repeats.
fn validate_unique_tools(
    path: &Path,
    tools: &[ToolFrontmatter],
    errors: &mut Vec<ValidationError>,
) {
    let mut reported: Vec<&ToolFrontmatter> = Vec::new();
    for (i, tool) in tools.iter().enumerate() {
        if tools[..i].contains(tool) && !reported.contains(&tool) {
            reported.push(tool);
            let name: &'static str = tool.into();
            errors.push(ValidationError {
                path: path.to_path_buf(),
                message: format!("`capabilities.tools` lists '{name}' more than once"),
            });
        }
    }
}

/// Check each `capabilities.mcp` grant against the declared servers and the
/// provider-neutral name rule.
///
/// Only the neutral rule applies here: whether a name collides with another
/// once a provider composes it is that provider's check, in its adapter's
/// `validate_declarations`.
fn validate_mcp_grants(
    path: &Path,
    grants: &BTreeMap<String, McpGrant>,
    servers: &McpServers,
    errors: &mut Vec<ValidationError>,
) {
    let mut push = |message: String| {
        errors.push(ValidationError {
            path: path.to_path_buf(),
            message,
        });
    };
    for (server, McpGrant { tools }) in grants {
        if !servers.contains_key(server) {
            push(format!("unknown MCP server '{server}'"));
        }
        let McpTools::Named(named) = tools else {
            continue;
        };
        if named.is_empty() {
            push(format!(
                "`capabilities.mcp.{server}.tools` must list at least one tool, or be `all`"
            ));
        }
        let mut reported: Vec<&String> = Vec::new();
        for (i, tool) in named.iter().enumerate() {
            if !is_mcp_name(tool) {
                push(format!(
                    "`capabilities.mcp.{server}.tools` names '{tool}', which must match \
                     [A-Za-z0-9_-]+"
                ));
            }
            if named[..i].contains(tool) && !reported.contains(&tool) {
                reported.push(tool);
                push(format!(
                    "`capabilities.mcp.{server}.tools` lists '{tool}' more than once"
                ));
            }
        }
    }
}

/// The file kinds a spec's grants could reach on any provider.
fn grant_kinds(spec: &Spec) -> &'static [FileKind] {
    match spec {
        Spec::Agent(_) => &[FileKind::Agents],
        Spec::Skill(_) => &[FileKind::Skills, FileKind::Commands],
        Spec::Rule(_) | Spec::Hook(_) => &[],
    }
}

/// Require an entry for every named tool of every grant in each provider
/// block's `tool-names`, for the providers that write the grant. A real name
/// and `false` each cover the tool.
///
/// A provider whose `carriable` omits `Mcp` for the spec's kinds writes no
/// grant for the spec, so requiring an entry there would demand config that
/// changes no output. A block with no map
/// keeps every logical name and needs no entry. An undeclared server and a
/// malformed tool name are `validate_mcp_grants`'s to report, so they are
/// skipped here; each `(tool, provider)` pair is reported once, however often
/// the grant repeats the tool.
fn validate_mcp_tool_coverage(
    spec: &Spec,
    grants: &BTreeMap<String, McpGrant>,
    servers: &McpServers,
    errors: &mut Vec<ValidationError>,
) {
    let kinds = grant_kinds(spec);
    let writers: Vec<Provider> = Provider::VARIANTS
        .iter()
        .copied()
        .filter(|provider| {
            let adapter = provider.adapter();
            kinds
                .iter()
                .any(|&kind| adapter.carriable(kind).contains(&SettingKind::Mcp))
        })
        .collect();
    for (server, McpGrant { tools }) in grants {
        let Some(declaration) = servers.get(server) else {
            continue;
        };
        let McpTools::Named(named) = tools else {
            continue;
        };
        for (i, tool) in named.iter().enumerate() {
            if !is_mcp_name(tool) || named[..i].contains(tool) {
                continue;
            }
            for provider in &writers {
                let Some(names) = provider.adapter().mcp_tool_names(declaration) else {
                    continue;
                };
                if !names.contains_key(tool) {
                    errors.push(ValidationError {
                        path: spec.path().to_path_buf(),
                        message: format!(
                            "`capabilities.mcp.{server}.tools` names '{tool}', which \
                             [mcp.{server}.{provider}] `tool-names` does not map; add \
                             `{tool} = \"<real name>\"`, or `{tool} = false` if that server \
                             has no equivalent"
                        ),
                    });
                }
            }
        }
    }
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
    use crate::mcp::{
        ClaudeMcpServer, CursorMcpServer, McpServer, McpToolNames, OpenCodeMcpServer,
    };
    use crate::presets::{ProviderPresets, ProviderPresetsMap};
    use crate::spec::{
        AgentFrontmatter, AgentSpec, CapabilitiesFrontmatter, ExecutionFrontmatter, HookEvent,
        HookFrontmatter, HookSpec, RuleFrontmatter, RuleSpec, SkillFrontmatter, SkillSpec,
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
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
        assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
    }

    #[test]
    fn test_semantics_duplicate_id() {
        let specs = vec![make_agent("dup", "body a"), make_agent("dup", "body b")];
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("duplicate id 'dup'"));
    }

    #[test]
    fn test_semantics_empty_body() {
        let specs = vec![make_agent("empty", "")];
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(&[spec], &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(
            &[spec],
            &Declarations {
                presets,
                ..Declarations::default()
            },
            test_config_path(),
        );
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
        let errors = validate_semantics(
            &[spec],
            &Declarations {
                presets,
                ..Declarations::default()
            },
            test_config_path(),
        );
        assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
    }

    fn with_tools(mut spec: Spec, tools: Vec<ToolFrontmatter>) -> Spec {
        let capabilities = Some(CapabilitiesFrontmatter {
            tools: Some(tools),
            mcp: None,
        });
        match &mut spec {
            Spec::Agent(a) => a.frontmatter.capabilities = capabilities,
            Spec::Skill(s) => s.frontmatter.capabilities = capabilities,
            Spec::Rule(_) | Spec::Hook(_) => panic!("only agents and skills declare tools"),
        }
        spec
    }

    #[test]
    fn test_semantics_duplicate_builtin_tool_rejected() {
        for spec in [make_agent("a", "body"), make_skill("s", "body")] {
            let spec = with_tools(
                spec,
                vec![
                    ToolFrontmatter::Read,
                    ToolFrontmatter::Grep,
                    ToolFrontmatter::Read,
                    ToolFrontmatter::Read,
                ],
            );
            let errors = validate_semantics(&[spec], &Declarations::default(), test_config_path());
            assert_eq!(errors.len(), 1, "errors: {errors:?}");
            assert_eq!(
                errors[0].message,
                "`capabilities.tools` lists 'read' more than once"
            );
        }
    }

    /// One error per repeated tool, in the order each first repeats, and each
    /// named in its authored spelling — `webfetch` pins strum's rendering of a
    /// multi-word variant against serde's.
    #[test]
    fn test_semantics_duplicate_builtin_tools_each_reported_once() {
        let spec = with_tools(
            make_agent("a", "body"),
            vec![
                ToolFrontmatter::WebFetch,
                ToolFrontmatter::Grep,
                ToolFrontmatter::Grep,
                ToolFrontmatter::WebFetch,
            ],
        );
        let errors = validate_semantics(&[spec], &Declarations::default(), test_config_path());
        let messages: Vec<&str> = errors.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "`capabilities.tools` lists 'grep' more than once",
                "`capabilities.tools` lists 'webfetch' more than once",
            ]
        );
    }

    fn with_mcp(
        mut spec: Spec,
        tools: Option<Vec<ToolFrontmatter>>,
        grants: &[(&str, McpTools)],
    ) -> Spec {
        let capabilities = Some(CapabilitiesFrontmatter {
            tools,
            mcp: Some(
                grants
                    .iter()
                    .map(|(server, tools)| {
                        (
                            (*server).to_owned(),
                            McpGrant {
                                tools: tools.clone(),
                            },
                        )
                    })
                    .collect(),
            ),
        });
        match &mut spec {
            Spec::Agent(a) => a.frontmatter.capabilities = capabilities,
            Spec::Skill(s) => s.frontmatter.capabilities = capabilities,
            Spec::Rule(_) | Spec::Hook(_) => panic!("only agents and skills declare grants"),
        }
        spec
    }

    fn named(tools: &[&str]) -> McpTools {
        McpTools::Named(tools.iter().map(|t| (*t).to_owned()).collect())
    }

    fn mcp_declarations(servers: &[&str]) -> Declarations {
        Declarations {
            mcp: servers
                .iter()
                .map(|s| ((*s).to_owned(), McpServer::default()))
                .collect(),
            ..Declarations::default()
        }
    }

    fn mcp_messages(spec: Spec, declarations: &Declarations) -> Vec<String> {
        validate_semantics(&[spec], declarations, test_config_path())
            .into_iter()
            .map(|e| e.message)
            .collect()
    }

    #[test]
    fn test_semantics_valid_mcp_grant_passes() {
        let spec = with_mcp(
            make_agent("a", "body"),
            Some(vec![]),
            &[
                ("quip", named(&["search_documents"])),
                ("jira", McpTools::All),
            ],
        );
        assert_eq!(
            mcp_messages(spec, &mcp_declarations(&["quip", "jira"])),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_semantics_mcp_grant_unknown_server_rejected() {
        let spec = with_mcp(make_skill("s", "body"), None, &[("quip", McpTools::All)]);
        assert_eq!(
            mcp_messages(spec, &mcp_declarations(&[])),
            ["unknown MCP server 'quip'"]
        );
    }

    #[test]
    fn test_semantics_mcp_grant_bad_tool_name_rejected() {
        let spec = with_mcp(make_skill("s", "body"), None, &[("quip", named(&["a.b"]))]);
        assert_eq!(
            mcp_messages(spec, &mcp_declarations(&["quip"])),
            ["`capabilities.mcp.quip.tools` names 'a.b', which must match [A-Za-z0-9_-]+"]
        );
    }

    #[test]
    fn test_semantics_mcp_grant_empty_list_rejected() {
        let spec = with_mcp(make_skill("s", "body"), None, &[("quip", named(&[]))]);
        assert_eq!(
            mcp_messages(spec, &mcp_declarations(&["quip"])),
            ["`capabilities.mcp.quip.tools` must list at least one tool, or be `all`"]
        );
    }

    #[test]
    fn test_semantics_mcp_grant_repeated_tool_rejected() {
        let spec = with_mcp(
            make_skill("s", "body"),
            None,
            &[("quip", named(&["a", "b", "a", "a"]))],
        );
        assert_eq!(
            mcp_messages(spec, &mcp_declarations(&["quip"])),
            ["`capabilities.mcp.quip.tools` lists 'a' more than once"]
        );
    }

    #[test]
    fn test_semantics_mcp_grant_on_agent_without_tools_rejected() {
        let spec = with_mcp(make_agent("a", "body"), None, &[("quip", McpTools::All)]);
        let messages = mcp_messages(spec, &mcp_declarations(&["quip"]));
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(
            messages[0].starts_with(
                "an agent that declares `capabilities.mcp` must declare `capabilities.tools`"
            ),
            "{}",
            messages[0]
        );
    }

    #[test]
    fn test_semantics_mcp_grant_on_skill_without_tools_passes() {
        let spec = with_mcp(make_skill("s", "body"), None, &[("quip", McpTools::All)]);
        assert_eq!(
            mcp_messages(spec, &mcp_declarations(&["quip"])),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_validate_config_declarations_rejects_bad_mcp_server_name() {
        let errors = validate_config_declarations(&mcp_declarations(&["a.b"]), test_config_path());
        let messages: Vec<&str> = errors.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(
            messages,
            ["[mcp.a.b] the server name must match [A-Za-z0-9_-]+"]
        );
    }

    fn tool_names(entries: &[(&str, &str)]) -> McpToolNames {
        entries
            .iter()
            .map(|(k, v)| ((*k).to_owned(), McpToolName::Real((*v).to_owned())))
            .collect()
    }

    /// A Claude block whose `tool-names` sets each of `tools` to `false`.
    fn claude_absent(tools: &[&str]) -> McpServer {
        McpServer {
            claude: Some(ClaudeMcpServer {
                tool_names: Some(
                    tools
                        .iter()
                        .map(|t| ((*t).to_owned(), McpToolName::Absent))
                        .collect(),
                ),
                ..ClaudeMcpServer::default()
            }),
            ..McpServer::default()
        }
    }

    fn claude_mapped(entries: &[(&str, &str)]) -> McpServer {
        McpServer {
            claude: Some(ClaudeMcpServer {
                tool_names: Some(tool_names(entries)),
                ..ClaudeMcpServer::default()
            }),
            ..McpServer::default()
        }
    }

    fn opencode_mapped(entries: &[(&str, &str)]) -> McpServer {
        McpServer {
            opencode: Some(OpenCodeMcpServer {
                tool_names: Some(tool_names(entries)),
                ..OpenCodeMcpServer::default()
            }),
            ..McpServer::default()
        }
    }

    fn cursor_mapped(entries: &[(&str, &str)]) -> McpServer {
        McpServer {
            cursor: Some(CursorMcpServer {
                tool_names: Some(tool_names(entries)),
                ..CursorMcpServer::default()
            }),
            ..McpServer::default()
        }
    }

    /// `[mcp.s]` declared as `server`.
    fn declaring(server: McpServer) -> Declarations {
        Declarations {
            mcp: McpServers::from([("s".to_owned(), server)]),
            ..Declarations::default()
        }
    }

    fn declaration_messages(server: McpServer) -> Vec<String> {
        validate_config_declarations(&declaring(server), test_config_path())
            .into_iter()
            .map(|e| e.message)
            .collect()
    }

    fn coverage_message(tool: &str, provider: &str) -> String {
        format!(
            "`capabilities.mcp.s.tools` names '{tool}', which [mcp.s.{provider}] `tool-names` \
             does not map; add `{tool} = \"<real name>\"`, or `{tool} = false` if that server has \
             no equivalent"
        )
    }

    #[test]
    fn test_tool_names_rejects_bad_key() {
        assert_eq!(
            declaration_messages(claude_mapped(&[("a.b", "x")])),
            ["[mcp.s.claude] `tool-names` key 'a.b' must match [A-Za-z0-9_-]+"]
        );
    }

    #[test]
    fn test_tool_names_rejects_bad_value() {
        assert_eq!(
            declaration_messages(opencode_mapped(&[("a", "x.y")])),
            ["[mcp.s.opencode] `tool-names` maps 'a' to 'x.y', which must match [A-Za-z0-9_-]+"]
        );
    }

    #[test]
    fn test_tool_names_accepts_shared_value() {
        assert_eq!(
            declaration_messages(cursor_mapped(&[("a", "x"), ("b", "x")])),
            Vec::<String>::new()
        );
    }

    /// A malformed value shared by two keys is reported once per key.
    #[test]
    fn test_tool_names_reports_each_malformed_shared_value() {
        assert_eq!(
            declaration_messages(claude_mapped(&[("a", "x.y"), ("b", "x.y")])),
            [
                "[mcp.s.claude] `tool-names` maps 'a' to 'x.y', which must match [A-Za-z0-9_-]+",
                "[mcp.s.claude] `tool-names` maps 'b' to 'x.y', which must match [A-Za-z0-9_-]+",
            ]
        );
    }

    #[test]
    fn test_tool_names_false_value_needs_no_name_check() {
        assert_eq!(
            declaration_messages(claude_absent(&["a"])),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_tool_names_false_entry_covers_grant() {
        let spec = with_mcp(
            make_agent("a", "body"),
            Some(vec![]),
            &[("s", named(&["t"]))],
        );
        assert_eq!(
            mcp_messages(spec, &declaring(claude_absent(&["t"]))),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_tool_names_accepts_identity_entry() {
        assert_eq!(
            declaration_messages(claude_mapped(&[("x", "x"), ("y", "z")])),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_tool_names_claude_map_must_cover_agent_grant() {
        let spec = with_mcp(
            make_agent("a", "body"),
            Some(vec![]),
            &[("s", named(&["t"]))],
        );
        assert_eq!(
            mcp_messages(spec, &declaring(claude_mapped(&[]))),
            [coverage_message("t", "claude")]
        );
    }

    #[test]
    fn test_tool_names_opencode_map_must_cover_agent_grant() {
        let spec = with_mcp(
            make_agent("a", "body"),
            Some(vec![]),
            &[("s", named(&["t"]))],
        );
        assert_eq!(
            mcp_messages(spec, &declaring(opencode_mapped(&[("u", "v")]))),
            [coverage_message("t", "opencode")]
        );
    }

    /// `OpenCode` writes no grant on a skill's files.
    #[test]
    fn test_tool_names_opencode_map_ignores_skill_grant() {
        let spec = with_mcp(make_skill("s", "body"), None, &[("s", named(&["t"]))]);
        assert_eq!(
            mcp_messages(spec, &declaring(opencode_mapped(&[]))),
            Vec::<String>::new()
        );
    }

    /// Cursor writes no grant on any file kind.
    #[test]
    fn test_tool_names_cursor_map_ignores_grants() {
        for spec in [
            with_mcp(
                make_agent("a", "body"),
                Some(vec![]),
                &[("s", named(&["t"]))],
            ),
            with_mcp(make_skill("k", "body"), None, &[("s", named(&["t"]))]),
        ] {
            assert_eq!(
                mcp_messages(spec, &declaring(cursor_mapped(&[]))),
                Vec::<String>::new()
            );
        }
    }

    #[test]
    fn test_tool_names_whole_server_grant_needs_no_entry() {
        let spec = with_mcp(
            make_agent("a", "body"),
            Some(vec![]),
            &[("s", McpTools::All)],
        );
        assert_eq!(
            mcp_messages(spec, &declaring(claude_mapped(&[]))),
            Vec::<String>::new()
        );
    }

    /// An undeclared server and a malformed tool name each keep their one
    /// existing message; a valid tool beside the malformed one is still
    /// checked.
    #[test]
    fn test_tool_names_skip_already_reported_grants() {
        let spec = with_mcp(make_skill("k", "body"), None, &[("q", named(&["t"]))]);
        assert_eq!(
            mcp_messages(spec, &declaring(claude_mapped(&[]))),
            ["unknown MCP server 'q'"]
        );

        let spec = with_mcp(
            make_skill("k", "body"),
            None,
            &[("s", named(&["a.b", "t"]))],
        );
        assert_eq!(
            mcp_messages(spec, &declaring(claude_mapped(&[]))),
            [
                "`capabilities.mcp.s.tools` names 'a.b', which must match [A-Za-z0-9_-]+"
                    .to_owned(),
                coverage_message("t", "claude"),
            ]
        );
    }

    #[test]
    fn test_tool_names_repeated_tool_reported_once() {
        let server = McpServer {
            claude: claude_mapped(&[]).claude,
            opencode: opencode_mapped(&[]).opencode,
            ..McpServer::default()
        };
        let spec = with_mcp(
            make_agent("a", "body"),
            Some(vec![]),
            &[("s", named(&["t", "t"]))],
        );
        let coverage: Vec<String> = mcp_messages(spec, &declaring(server))
            .into_iter()
            .filter(|m| m.contains("`tool-names` does not map"))
            .collect();
        assert_eq!(
            coverage,
            [
                coverage_message("t", "claude"),
                coverage_message("t", "opencode")
            ]
        );
    }

    #[test]
    fn test_tool_names_absent_everywhere_adds_no_message() {
        let spec = with_mcp(
            make_agent("a", "body"),
            Some(vec![]),
            &[("s", named(&["t"]))],
        );
        assert_eq!(
            mcp_messages(spec, &declaring(McpServer::default())),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_semantics_distinct_builtin_tools_pass() {
        let spec = with_tools(
            make_agent("a", "body"),
            vec![ToolFrontmatter::Read, ToolFrontmatter::Grep],
        );
        let errors = validate_semantics(&[spec], &Declarations::default(), test_config_path());
        assert!(errors.is_empty(), "errors: {errors:?}");
    }

    #[test]
    fn test_semantics_rule_passes_all_checks() {
        let specs = vec![make_rule("my-rule", "body")];
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
        assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
    }

    // -- Underscore-normalization collision tests --

    #[test]
    fn test_semantics_underscore_collision_same_type() {
        let specs = vec![
            make_skill("gh-safe", "body one"),
            make_skill("gh_safe", "body two"),
        ];
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(&specs, &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(&[spec], &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(&[spec], &Declarations::default(), test_config_path());
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
        let errors = validate_semantics(&[spec], &Declarations::default(), test_config_path());
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
        let errors = validate_config_declarations(
            &Declarations {
                presets,
                ..Declarations::default()
            },
            Path::new("/tmp/agentspec.toml"),
        );
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
        let errors = validate_semantics(
            &[],
            &Declarations {
                presets,
                ..Declarations::default()
            },
            Path::new("/tmp/agentspec.toml"),
        );
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
            let errors = validate_config_declarations(
                &Declarations {
                    presets: presets_with_cursor("x", preset),
                    ..Declarations::default()
                },
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
        assert!(
            validate_config_declarations(
                &Declarations {
                    presets,
                    ..Declarations::default()
                },
                Path::new("/tmp/agentspec.toml")
            )
            .is_empty()
        );
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

        let errors = validate_config_declarations(
            &Declarations {
                presets,
                ..Declarations::default()
            },
            Path::new("/tmp/agentspec.toml"),
        );
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
