//! Integration tests using a self-contained fixture under `tests/fixtures/agent-config/`.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("agent-config")
}

/// Copy fixture to a temporary directory and restore executable bits on `.sh` files.
///
/// Git does not reliably preserve executable bits, so we set them explicitly
/// after copying. This ensures `parse.rs` detects `helper.sh` as executable and
/// the generated output inherits the correct permissions.
fn setup(tmp: &TempDir) -> PathBuf {
    let dest = tmp.path().join("agent-config");
    let status = std::process::Command::new("cp")
        .arg("-r")
        .arg(fixture_dir())
        .arg(&dest)
        .status();
    assert!(
        status.as_ref().is_ok_and(std::process::ExitStatus::success),
        "cp fixture failed: {status:?}"
    );
    set_script_permissions(&dest);
    dest
}

#[cfg(unix)]
fn set_script_permissions(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            set_script_permissions(&path);
        } else if path.extension().is_some_and(|ext| ext == "sh")
            && let Ok(meta) = std::fs::metadata(&path)
        {
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() | 0o111);
            let _ = std::fs::set_permissions(&path, perms);
        }
    }
}

#[cfg(not(unix))]
fn set_script_permissions(_dir: &Path) {}

#[cfg(unix)]
fn add_symlink_to_fixture(dir: &Path, link_rel: &str, target_rel: &str) {
    let link_path = dir.join(link_rel);
    if let Some(parent) = link_path.parent() {
        let r = std::fs::create_dir_all(parent);
        assert!(r.is_ok(), "create parent for symlink: {r:?}");
    }
    let r = std::os::unix::fs::symlink(std::path::Path::new(target_rel), &link_path);
    assert!(r.is_ok(), "create symlink: {r:?}");
}

fn agentspec() -> &'static str {
    env!("CARGO_BIN_EXE_agentspec")
}

#[test]
fn test_validate_fixture() {
    let output = std::process::Command::new(agentspec())
        .arg("validate")
        .current_dir(fixture_dir())
        .output()
        .expect("failed to run agentspec validate");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "validate failed:\n{stderr}");
    assert!(stderr.contains("validation complete"), "stderr: {stderr}");
}

// This test is an inventory of every file the fixture is expected to produce,
// so its length tracks the fixture's size rather than any branching complexity.
#[allow(clippy::too_many_lines)]
#[test]
fn test_compile_generates_expected_files() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    // Claude: agent flat file + skill directories + script
    assert!(
        dir.join("generated/claude/agents/test-agent.md").exists(),
        "missing claude agent"
    );
    assert!(
        dir.join("generated/claude/skills/basic-skill/SKILL.md")
            .exists(),
        "missing claude basic-skill"
    );
    assert!(
        dir.join("generated/claude/skills/scripted-skill/SKILL.md")
            .exists(),
        "missing claude scripted-skill"
    );
    assert!(
        dir.join("generated/claude/skills/scripted-skill/scripts/helper.sh")
            .exists(),
        "missing claude helper script"
    );
    assert!(
        dir.join("generated/claude/skills/agent-invocable-skill/SKILL.md")
            .exists(),
        "missing claude agent-invocable-skill"
    );

    // OpenCode: agent + flat command files for user-invocable skills
    assert!(
        dir.join("generated/opencode/agents/test-agent.md").exists(),
        "missing opencode agent"
    );
    assert!(
        dir.join("generated/opencode/commands/basic-skill.md")
            .exists(),
        "missing opencode basic-skill command"
    );
    assert!(
        dir.join("generated/opencode/commands/scripted-skill.md")
            .exists(),
        "missing opencode scripted-skill command"
    );
    assert!(
        dir.join("generated/opencode/skills/agent-invocable-skill/SKILL.md")
            .exists(),
        "missing opencode agent-invocable-skill"
    );

    // Cursor: skills and agents
    assert!(
        dir.join("generated/cursor/skills/basic-skill/SKILL.md")
            .exists(),
        "missing cursor basic-skill"
    );
    assert!(
        dir.join("generated/cursor/skills/agent-invocable-skill/SKILL.md")
            .exists(),
        "missing cursor agent-invocable-skill"
    );
    assert!(
        dir.join("generated/cursor/agents/test-agent.md").exists(),
        "missing cursor agent"
    );

    // Fragment was resolved: basic-skill body should contain the included text
    let basic_skill_path = dir.join("generated/claude/skills/basic-skill/SKILL.md");
    let content = std::fs::read_to_string(&basic_skill_path).expect("failed to read basic-skill");
    assert!(
        content.contains("shared via fragment"),
        "fragment not resolved in basic-skill body"
    );

    // --- Rules ---

    // Claude: rules are plain body, no frontmatter
    let claude_general = dir.join("generated/claude/rules/general-guidance.md");
    assert!(
        claude_general.exists(),
        "missing claude general-guidance rule"
    );
    let claude_general_content =
        std::fs::read_to_string(&claude_general).expect("failed to read claude general rule");
    assert!(
        !claude_general_content.starts_with("---"),
        "claude rule should have no frontmatter"
    );

    let claude_api = dir.join("generated/claude/rules/api-design.md");
    assert!(claude_api.exists(), "missing claude api-design rule");

    // Cursor: .mdc extension with alwaysApply frontmatter
    let cursor_general = dir.join("generated/cursor/rules/general-guidance.mdc");
    assert!(
        cursor_general.exists(),
        "missing cursor general-guidance rule"
    );
    let cursor_general_content =
        std::fs::read_to_string(&cursor_general).expect("failed to read cursor general rule");
    assert!(
        cursor_general_content.contains("alwaysApply: true"),
        "cursor rule should have alwaysApply"
    );

    let cursor_api = dir.join("generated/cursor/rules/api-design.mdc");
    assert!(cursor_api.exists(), "missing cursor api-design rule");

    // OpenCode: rules in subdirectories with AGENTS.md
    assert!(
        dir.join("generated/opencode/rules/general-guidance/AGENTS.md")
            .exists(),
        "missing opencode general-guidance rule"
    );
    assert!(
        dir.join("generated/opencode/rules/api-design/AGENTS.md")
            .exists(),
        "missing opencode api-design rule"
    );

    // OpenCode: instructions.json is no longer produced by compile — it is patched by
    // `agentspec sync` into opencode.json directly. Verify it is absent.
    assert!(
        !dir.join("generated/opencode/instructions.json").exists(),
        "instructions.json should not be produced by compile"
    );
}

/// The fixture's `scripted-skill` names the `default` preset, whose `OpenCode`
/// half sets both `model` and `variant` in `agentspec.toml`. Both keys must
/// reach the generated command file.
#[test]
fn test_compile_opencode_command_carries_preset_variant() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    let command_path = dir.join("generated/opencode/commands/scripted-skill.md");
    let content = std::fs::read_to_string(&command_path).expect("failed to read opencode command");

    // Scope the assertions to the frontmatter block so body text can never
    // satisfy them.
    let Some((frontmatter, _)) = content
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---"))
    else {
        panic!("expected a frontmatter block, got:\n{content}");
    };

    assert!(
        frontmatter.contains("model: anthropic/claude-sonnet-4-5"),
        "opencode command should carry the preset model, got:\n{frontmatter}"
    );
    assert!(
        frontmatter.contains("variant: high"),
        "opencode command should carry the preset variant, got:\n{frontmatter}"
    );
}

/// `OpenCode` does not surface `model`, `variant`, or `tools` on skills, so
/// agentspec does not emit them.
///
/// The assertion is not a tautology: the fixture's `agent-invocable-skill`
/// names the `default` preset, whose `OpenCode` half sets both `model` and
/// `variant` in `agentspec.toml`, and its spec declares `capabilities.tools`.
/// All three keys had values available to emit.
#[test]
fn test_compile_opencode_skill_omits_discarded_frontmatter() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    let skill_path = dir.join("generated/opencode/skills/agent-invocable-skill/SKILL.md");
    let content = std::fs::read_to_string(&skill_path).expect("failed to read opencode skill");

    // Scope to the frontmatter block so a word in the body can neither satisfy
    // nor break these assertions.
    let Some((frontmatter, _)) = content
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---"))
    else {
        panic!("expected a frontmatter block, got:\n{content}");
    };

    for key in ["model:", "variant:", "tools:"] {
        assert!(
            !frontmatter.contains(key),
            "opencode skill frontmatter should not carry `{key}`, got:\n{frontmatter}"
        );
    }

    assert!(
        frontmatter.contains("name: agent-invocable-skill"),
        "opencode skill should keep its name, got:\n{frontmatter}"
    );
    assert!(
        frontmatter.contains("description:"),
        "opencode skill should keep its description, got:\n{frontmatter}"
    );
}

#[test]
fn test_compile_path_scoped_rule_outputs() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    // Claude: path-scoped rule emits frontmatter with paths array
    let claude_react = dir.join("generated/claude/rules/react-components.md");
    assert!(
        claude_react.exists(),
        "missing claude react-components rule"
    );
    let claude_react_content =
        std::fs::read_to_string(&claude_react).expect("failed to read claude react-components");
    assert!(
        claude_react_content.starts_with("---"),
        "claude path-scoped rule should have frontmatter"
    );
    assert!(
        claude_react_content.contains("paths:"),
        "claude path-scoped rule should have paths key"
    );
    assert!(
        claude_react_content.contains("src/components/**/*.tsx"),
        "claude path-scoped rule should contain first glob"
    );

    // Cursor: path-scoped rule emits alwaysApply: false and globs field
    let cursor_react = dir.join("generated/cursor/rules/react-components.mdc");
    assert!(
        cursor_react.exists(),
        "missing cursor react-components rule"
    );
    let cursor_react_content =
        std::fs::read_to_string(&cursor_react).expect("failed to read cursor react-components");
    assert!(
        cursor_react_content.contains("alwaysApply: false"),
        "cursor path-scoped rule should have alwaysApply: false"
    );
    assert!(
        cursor_react_content.contains("globs:"),
        "cursor path-scoped rule should have globs field"
    );
    assert!(
        cursor_react_content.contains("src/components/**/*.tsx"),
        "cursor path-scoped rule should contain first glob"
    );

    // OpenCode: path-scoped rule is emitted as always-on (no paths or globs)
    let opencode_react = dir.join("generated/opencode/rules/react-components/AGENTS.md");
    assert!(
        opencode_react.exists(),
        "missing opencode react-components rule"
    );
    let opencode_react_content =
        std::fs::read_to_string(&opencode_react).expect("failed to read opencode react-components");
    assert!(
        !opencode_react_content.contains("paths:"),
        "opencode rule should not contain paths"
    );
    assert!(
        !opencode_react_content.contains("globs:"),
        "opencode rule should not contain globs"
    );

    // The rule's `paths` reaches no OpenCode file, and the loss says so with
    // the derived explanation rather than an adapter-written sentence.
    assert!(
        stderr.contains("no opencode rules file carries `paths`"),
        "expected an OpenCode `paths` loss, got:\n{stderr}"
    );

    // Regression: non-path-scoped rules (general-guidance, api-design) are unaffected
    assert!(
        dir.join("generated/claude/rules/general-guidance.md")
            .exists(),
        "general-guidance regression: claude rule missing"
    );
    assert!(
        dir.join("generated/cursor/rules/general-guidance.mdc")
            .exists(),
        "general-guidance regression: cursor rule missing"
    );
}

#[test]
fn test_compile_resolves_tool_per_provider() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    // The fragment `shared-note.md` uses `{{ tool("question") }}`,
    // `{{ tool("subagent") }}`, and `{{ tool("skill") }}` and is included
    // from `basic-skill`. Each canonical should resolve per provider.
    let claude_path = dir.join("generated/claude/skills/basic-skill/SKILL.md");
    let claude = std::fs::read_to_string(&claude_path).expect("failed to read claude basic-skill");
    assert!(
        claude.contains("AskUserQuestion"),
        "expected 'AskUserQuestion' in claude basic-skill body, got:\n{claude}"
    );
    assert!(
        claude.contains("`Agent`"),
        "expected '`Agent`' in claude basic-skill body, got:\n{claude}"
    );
    assert!(
        claude.contains("`Skill`"),
        "expected '`Skill`' in claude basic-skill body, got:\n{claude}"
    );

    let cursor_path = dir.join("generated/cursor/skills/basic-skill/SKILL.md");
    let cursor = std::fs::read_to_string(&cursor_path).expect("failed to read cursor basic-skill");
    assert!(
        cursor.contains("Ask questions"),
        "expected 'Ask questions' in cursor basic-skill body, got:\n{cursor}"
    );
    assert!(
        cursor.contains("`Task`"),
        "expected '`Task`' in cursor basic-skill body, got:\n{cursor}"
    );
    assert!(
        cursor.contains("`Skill runner`"),
        "expected '`Skill runner`' in cursor basic-skill body, got:\n{cursor}"
    );

    let opencode_path = dir.join("generated/opencode/commands/basic-skill.md");
    let opencode =
        std::fs::read_to_string(&opencode_path).expect("failed to read opencode basic-skill");
    // Match with surrounding backticks to distinguish from the frontmatter
    // `question: false` tool-map entry. Also assert neither the Claude nor
    // Cursor form leaked in — together these prove provider-specific resolution,
    // not a canonical pass-through that happens to coincide with OpenCode's name.
    assert!(
        opencode.contains("`question`"),
        "expected '`question`' in opencode basic-skill body, got:\n{opencode}"
    );
    assert!(
        opencode.contains("`task`"),
        "expected '`task`' in opencode basic-skill body, got:\n{opencode}"
    );
    assert!(
        opencode.contains("`skill`"),
        "expected '`skill`' in opencode basic-skill body, got:\n{opencode}"
    );
    assert!(
        !opencode.contains("AskUserQuestion"),
        "opencode body should not contain the Claude tool name, got:\n{opencode}"
    );
    assert!(
        !opencode.contains("Ask questions"),
        "opencode body should not contain the Cursor tool name, got:\n{opencode}"
    );
    assert!(
        !opencode.contains("`Agent`"),
        "opencode body should not contain the Claude subagent name, got:\n{opencode}"
    );
    assert!(
        !opencode.contains("`Task`"),
        "opencode body should not contain the Cursor subagent name, got:\n{opencode}"
    );
    assert!(
        !opencode.contains("`Skill`"),
        "opencode body should not contain the Claude skill name, got:\n{opencode}"
    );
    assert!(
        !opencode.contains("`Skill runner`"),
        "opencode body should not contain the Cursor skill name, got:\n{opencode}"
    );
}

#[test]
#[cfg(unix)]
fn test_compile_script_is_executable() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let script_path = dir.join("generated/claude/skills/scripted-skill/scripts/helper.sh");
    assert!(script_path.exists(), "script not generated");

    let metadata = std::fs::metadata(&script_path).expect("failed to stat script");
    assert!(
        metadata.permissions().mode() & 0o111 != 0,
        "generated script should be executable"
    );
}

#[test]
#[cfg(unix)]
fn test_compile_preserves_non_executable_supporting_file_mode() {
    // E2E counterpart to `test_compile_script_is_executable`: a deliberately
    // non-executable supporting file (config.toml at 0o600) must round-trip
    // through the binary CLI and land on disk with its mode preserved
    // verbatim, not collapsed to umask-default 0o644.
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    // Drop the non-executable supporting file programmatically (rather than
    // via the shared fixture) so unrelated tests that scan `scripted-skill/`
    // aren't affected by its presence — same pattern as
    // `test_compile_ignores_bats_files`'s programmatic `test_helper.bats`.
    let source_config = dir.join("spec/skills/scripted-skill/scripts/config.toml");
    std::fs::write(&source_config, "[example]\nkey = \"value\"\n")
        .expect("failed to write source config.toml");
    std::fs::set_permissions(&source_config, std::fs::Permissions::from_mode(0o600))
        .expect("failed to chmod source config.toml");

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let generated = dir.join("generated/claude/skills/scripted-skill/scripts/config.toml");
    assert!(generated.exists(), "config.toml not generated");

    let metadata = std::fs::metadata(&generated).expect("failed to stat generated config.toml");
    assert_eq!(
        metadata.permissions().mode() & 0o0777,
        0o600,
        "generated supporting file should preserve 0o600 verbatim"
    );
}

#[test]
fn test_sync_prefix() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user").with_prefix("tw")]);

    let home = dir.join("home");
    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "sync with prefix should succeed:\n{stderr}"
    );
    assert!(
        home.join(".claude/agents/tw-test-agent.md").exists(),
        "prefixed agent file should exist"
    );
    assert!(
        home.join(".claude/skills/tw-basic-skill/SKILL.md").exists(),
        "prefixed skill directory should exist"
    );
}

#[test]
fn test_sync_opencode_commands_prefix_subdir() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    write_sync_config(
        &dir,
        &[SyncEntry::new("opencode", "user").with_prefix("tw")],
    );

    let home = dir.join("home");
    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "opencode"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sync failed:\n{stderr}");
    assert!(
        stderr.contains("OpenCode") && stderr.contains("commands"),
        "expected sync report to mention OpenCode and commands, stderr: {stderr}"
    );
    assert!(
        home.join(".config/opencode/commands/tw/basic-skill.md")
            .exists(),
        "prefixed opencode command file should exist"
    );
}

#[test]
fn test_sync_no_config_errors_with_guidance() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let home = dir.join("home");

    let output = std::process::Command::new(agentspec())
        .args(["sync"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "sync should fail:\n{stderr}");
    assert!(
        stderr.contains("no sync providers are configured"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("--provider"),
        "should suggest explicit provider: {stderr}"
    );
}

#[test]
fn test_sync_without_target_only_syncs_configured_providers() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let home = dir.join("home");

    write_sync_config(&dir, &[SyncEntry::new("cursor", "user")]);

    let output = std::process::Command::new(agentspec())
        .args(["sync"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sync should succeed:\n{stderr}");
    // Cursor is the only configured provider — its destination should be written.
    assert!(
        home.join(".cursor/agents").exists(),
        "cursor agents dir should be created"
    );
    // Claude and OpenCode are not configured and should not be synced.
    assert!(
        !home.join(".claude").exists(),
        "claude dir should not be created: {stderr}"
    );
    assert!(
        !home.join(".config/opencode").exists(),
        "opencode dir should not be created: {stderr}"
    );
}

#[test]
fn test_sync_provider_unconfigured_errors_without_dest() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let home = dir.join("home");

    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --provider claude");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "sync should fail:\n{stderr}");
    assert!(
        stderr.contains("sync is not configured for claude"),
        "stderr: {stderr}"
    );
}

#[test]
fn test_sync_provider_unconfigured_with_dest_without_mode_errors() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let home = dir.join("home");
    let dest = dir.join("sync-out");

    let output = std::process::Command::new(agentspec())
        .args([
            "sync",
            "--provider",
            "claude",
            "--dest",
            dest.to_str().expect("dest path should be valid utf-8"),
        ])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --provider claude --dest");

    assert!(
        !output.status.success(),
        "sync --dest without --mode must error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--mode"),
        "error should mention --mode, got:\n{stderr}"
    );
}

#[test]
fn test_sync_provider_unconfigured_with_mode_user_allowed() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let home = dir.join("home");

    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude", "--mode", "user"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --provider claude --mode user");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sync should succeed:\n{stderr}");
    // mode=user should write to ~/.claude/
    assert!(
        home.join(".claude/agents").exists(),
        "user-mode sync should write to ~/.claude/agents: {stderr}"
    );
}

#[test]
fn test_sync_provider_unconfigured_with_mode_project_allowed() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let home = dir.join("home");

    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude", "--mode", "project"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --provider claude --mode project");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sync should succeed:\n{stderr}");
    // mode=project should write to ./.claude/ relative to cwd
    assert!(
        dir.join(".claude/agents").exists(),
        "project-mode sync should write to .claude/agents: {stderr}"
    );
}

#[test]
fn test_sync_project_mode_with_dest_writes_to_target() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let home = dir.join("home");
    let target_project = dir.join("target-project");
    std::fs::create_dir_all(&target_project).expect("create target-project dir");

    let output = std::process::Command::new(agentspec())
        .args([
            "sync",
            "--provider",
            "claude",
            "--mode",
            "project",
            "--dest",
            target_project
                .to_str()
                .expect("dest path should be valid utf-8"),
        ])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --mode project --dest");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sync should succeed:\n{stderr}");
    assert!(
        target_project.join(".claude/agents").exists(),
        "project-mode+dest should write to <dest>/.claude/agents: {stderr}"
    );
}

#[test]
fn test_sync_dest_without_mode_errors() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let home = dir.join("home");

    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude", "--dest", "/tmp/test"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --dest without --mode");

    assert!(!output.status.success(), "sync should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--mode"),
        "error should mention --mode, got:\n{stderr}"
    );
}

#[test]
fn test_sync_user_mode_with_dest_errors() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let home = dir.join("home");

    let output = std::process::Command::new(agentspec())
        .args([
            "sync",
            "--provider",
            "claude",
            "--mode",
            "user",
            "--dest",
            "/tmp/test",
        ])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --mode user --dest");

    assert!(!output.status.success(), "sync should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("user"),
        "error should mention user mode, got:\n{stderr}"
    );
    assert!(
        stderr.contains("dir"),
        "error should mention dir, got:\n{stderr}"
    );
}

#[test]
fn test_sync_no_config_mode_user_without_provider_errors() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let home = dir.join("home");

    let output = std::process::Command::new(agentspec())
        .args(["sync", "--mode", "user"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --mode user");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "sync should fail:\n{stderr}");
    assert!(
        stderr.contains("no sync providers are configured"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("--provider"),
        "should suggest explicit provider: {stderr}"
    );
}

#[test]
fn test_sync_invalid_base_sync_config_surfaces_parse_error() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let home = dir.join("home");

    // Inline write retained intentionally: this test verifies the parser
    // surfaces an `unknown field` error for an invalid [sync.<provider>]
    // block. Routing through `write_sync_config` would emit a valid block
    // and defeat the test.
    std::fs::write(
        dir.join("agentspec.toml"),
        r#"
[presets.default]
claude = { model = "sonnet" }
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
cursor = { model = "fast" }

[sync.cursor]
invalid_field = "oops"
"#,
    )
    .expect("failed to write agentspec.toml");

    let output = std::process::Command::new(agentspec())
        .args(["sync"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "sync should fail:\n{stderr}");
    assert!(stderr.contains("failed to parse"), "stderr: {stderr}");
    assert!(stderr.contains("unknown field"), "stderr: {stderr}");
}

#[test]
fn test_sync_dest_without_provider_errors() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let home = dir.join("home");
    let dest = dir.join("sync-out");

    let output = std::process::Command::new(agentspec())
        .args([
            "sync",
            "--dest",
            dest.to_str().expect("dest path should be valid utf-8"),
        ])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --dest without --provider");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "sync should fail:\n{stderr}");
    assert!(
        stderr.contains("--dest requires an explicit --provider"),
        "stderr: {stderr}"
    );
}

#[test]
fn test_sync_cli_prefix_overrides_config() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(
        &dir,
        &[SyncEntry::new("claude", "user").with_prefix("original")],
    );

    let home = dir.join("home");
    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude", "--prefix", "cli-pfx"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --prefix");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "sync with --prefix should succeed:\n{stderr}"
    );
    // CLI prefix should win over config prefix — agent file should use "cli-pfx-" prefix.
    assert!(
        home.join(".claude/agents/cli-pfx-test-agent.md").exists(),
        "expected cli-pfx-prefixed agent file, ls: {:?}",
        std::fs::read_dir(home.join(".claude/agents")).map(|d| d
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect::<Vec<_>>())
    );
}

#[test]
fn test_sync_cli_prefix_without_config_prefix() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);

    let home = dir.join("home");
    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude", "--prefix", "ns"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --prefix");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "sync with --prefix should succeed:\n{stderr}"
    );
    // --prefix applied even when config has no prefix
    assert!(
        home.join(".claude/agents/ns-test-agent.md").exists(),
        "expected ns-prefixed agent file, ls: {:?}",
        std::fs::read_dir(home.join(".claude/agents")).map(|d| d
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect::<Vec<_>>())
    );
}

#[test]
fn test_sync_prefix_resolves_spec_references() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user").with_prefix("tw")]);

    let home = dir.join("home");
    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "sync with prefix should succeed:\n{stderr}"
    );

    // basic-skill body references test-agent via {{ specs.agent.test_agent.name }}.
    // With prefix "tw" for Claude, the resolved name should be "tw-test-agent".
    let skill_path = home.join(".claude/skills/tw-basic-skill/SKILL.md");
    let content = std::fs::read_to_string(&skill_path).expect("failed to read basic-skill");
    assert!(
        content.contains("Agent: tw-test-agent"),
        "expected prefixed agent reference 'tw-test-agent' in body, got:\n{content}"
    );
}

#[test]
fn test_sync_opencode_spec_references_agent_prefixed() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(
        &dir,
        &[SyncEntry::new("opencode", "user").with_prefix("tw")],
    );

    let home = dir.join("home");
    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "opencode"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sync failed:\n{stderr}");

    // basic-skill body references test-agent via {{ specs.agent.test_agent.name }}.
    // For OpenCode, agents ARE prefixed (identity from filename), so the resolved
    // name should be "tw-test-agent".
    let cmd_path = home.join(".config/opencode/commands/tw/basic-skill.md");
    let content = std::fs::read_to_string(&cmd_path).expect("failed to read basic-skill command");
    assert!(
        content.contains("Agent: tw-test-agent"),
        "expected prefixed agent reference 'tw-test-agent' in OpenCode command body, got:\n{content}"
    );
}

#[test]
fn test_compile_nonexistent_spec_reference_errors() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    // Create a spec that references a nonexistent spec via keyed access.
    // The chained attribute access (.name on undefined) should error under
    // MiniJinja's Lenient mode.
    let skill_dir = dir.join("spec/skills/bad-ref");
    std::fs::create_dir_all(&skill_dir).expect("failed to create skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nid: bad-ref\ndescription: References a nonexistent spec\nuser_invocable: true\n---\n{{ specs.skill.nonexistent_skill.name }}\n",
    )
    .expect("failed to write bad-ref skill");

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    assert!(
        !output.status.success(),
        "compile should fail when referencing nonexistent spec"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("bad-ref"),
        "error should mention the spec with the bad reference, got:\n{stderr}"
    );
}

#[test]
fn test_sync_content_prefix_without_file_prefix() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(
        &dir,
        &[SyncEntry::new("claude", "user").with_content_prefix("tw:")],
    );

    let home = dir.join("home");
    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "sync with content-prefix should succeed:\n{stderr}"
    );

    // File path should be unprefixed (no `prefix` set)
    assert!(
        home.join(".claude/skills/basic-skill/SKILL.md").exists(),
        "skill directory should be unprefixed"
    );

    // Body content should use colon-prefixed agent reference
    let skill_path = home.join(".claude/skills/basic-skill/SKILL.md");
    let content = std::fs::read_to_string(&skill_path).expect("failed to read basic-skill");
    assert!(
        content.contains("Agent: tw:test-agent"),
        "expected colon-prefixed agent reference 'tw:test-agent' in body, got:\n{content}"
    );
}

#[test]
fn test_sync_prefix_and_content_prefix() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(
        &dir,
        &[SyncEntry::new("claude", "user")
            .with_prefix("tw")
            .with_content_prefix("tw:")],
    );

    let home = dir.join("home");
    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "sync with both prefix and content-prefix should succeed:\n{stderr}"
    );

    // File path uses prefix (hyphen-separated)
    assert!(
        home.join(".claude/skills/tw-basic-skill/SKILL.md").exists(),
        "skill directory should use file prefix"
    );

    // Body content uses content-prefix (colon-separated)
    let skill_path = home.join(".claude/skills/tw-basic-skill/SKILL.md");
    let content = std::fs::read_to_string(&skill_path).expect("failed to read basic-skill");
    assert!(
        content.contains("Agent: tw:test-agent"),
        "expected colon-prefixed agent reference 'tw:test-agent' in body, got:\n{content}"
    );
}

#[test]
fn test_sync_cli_content_prefix_overrides_config() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(
        &dir,
        &[SyncEntry::new("claude", "user").with_content_prefix("original:")],
    );

    let home = dir.join("home");
    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude", "--content-prefix", "cli:"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --content-prefix");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "sync with --content-prefix should succeed:\n{stderr}"
    );

    // CLI content-prefix should override config
    let skill_path = home.join(".claude/skills/basic-skill/SKILL.md");
    let content = std::fs::read_to_string(&skill_path).expect("failed to read basic-skill");
    assert!(
        content.contains("Agent: cli:test-agent"),
        "expected CLI-overridden agent reference 'cli:test-agent' in body, got:\n{content}"
    );
}

// ---------------------------------------------------------------------------
// [spec].ignore tests
// ---------------------------------------------------------------------------

/// Rewrite `agentspec.toml` to retain the default `[compile]` / `[presets]`
/// blocks and add a `[spec]` section with the given `ignore` patterns.
fn write_ignore_config(dir: &Path, patterns: &[&str]) {
    let patterns_toml: Vec<String> = patterns.iter().map(|p| format!("\"{p}\"")).collect();
    let toml = format!(
        r#"
[spec]
sources_dir = "spec"
ignore = [{}]

[compile]
output_dir = "generated"

[presets.default]
claude = {{ model = "sonnet" }}
opencode = {{ model = "anthropic/claude-sonnet-4-5", variant = "high" }}
cursor = {{ model = "fast" }}
"#,
        patterns_toml.join(", ")
    );
    // `allow-expect-in-tests` applies only inside `#[test]` fns, so this
    // helper uses the `assert!(…is_ok…)` idiom established by `setup()`.
    let result = std::fs::write(dir.join("agentspec.toml"), toml);
    assert!(result.is_ok(), "failed to write agentspec.toml: {result:?}");
}

#[test]
fn test_compile_ignores_bats_files() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    // Drop a colocated test file next to the scripted-skill script.
    std::fs::write(
        dir.join("spec/skills/scripted-skill/scripts/test_helper.bats"),
        "bats test\n",
    )
    .expect("failed to write bats file");

    write_ignore_config(&dir, &["**/*.bats"]);

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    // helper.sh should still flow through to providers that emit supporting
    // files (Claude, Cursor). OpenCode emits `commands/<name>.md` and no
    // supporting files, so we only verify the .bats file is absent from
    // providers that would otherwise ship it.
    for provider in ["claude", "cursor"] {
        assert!(
            dir.join(format!(
                "generated/{provider}/skills/scripted-skill/scripts/helper.sh"
            ))
            .exists(),
            "{provider}: helper.sh should be present"
        );
        assert!(
            !dir.join(format!(
                "generated/{provider}/skills/scripted-skill/scripts/test_helper.bats"
            ))
            .exists(),
            "{provider}: test_helper.bats should have been ignored"
        );
    }
}

#[test]
fn test_compile_prunes_ignored_subtree() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    // Add a fixtures/ subtree next to the scripted-skill script.
    let fixtures_dir = dir.join("spec/skills/scripted-skill/fixtures");
    std::fs::create_dir_all(&fixtures_dir).expect("failed to create fixtures dir");
    std::fs::write(fixtures_dir.join("big.dat"), "binary blob\n")
        .expect("failed to write fixture file");

    write_ignore_config(&dir, &["**/fixtures/**"]);

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    for provider in ["claude", "opencode", "cursor"] {
        assert!(
            !dir.join(format!(
                "generated/{provider}/skills/scripted-skill/fixtures"
            ))
            .exists(),
            "{provider}: fixtures/ should have been pruned"
        );
    }

    // The ignore should target only the pruned subtree — sibling supporting
    // files must survive.
    for provider in ["claude", "cursor"] {
        assert!(
            dir.join(format!(
                "generated/{provider}/skills/scripted-skill/scripts/helper.sh"
            ))
            .exists(),
            "{provider}: helper.sh (sibling of fixtures/) should be preserved"
        );
    }
}

#[test]
fn test_validate_malformed_ignore_pattern_errors() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    write_ignore_config(&dir, &["["]);

    let output = std::process::Command::new(agentspec())
        .arg("validate")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec validate");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "validate should fail on malformed pattern; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("invalid ignore pattern"),
        "expected 'invalid ignore pattern' in stderr, got:\n{stderr}"
    );
    assert!(
        stderr.contains("'['"),
        "expected offending pattern '[' in stderr, got:\n{stderr}"
    );
}

#[test]
fn test_validate_lists_ignored_paths() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    std::fs::write(
        dir.join("spec/skills/scripted-skill/scripts/test_helper.bats"),
        "bats test\n",
    )
    .expect("failed to write bats file");
    write_ignore_config(&dir, &["**/*.bats"]);

    let output = std::process::Command::new(agentspec())
        .arg("validate")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec validate");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "validate failed:\n{stderr}");
    assert!(
        stderr.contains("ignoring 1 path"),
        "expected 'ignoring 1 path' in stderr, got:\n{stderr}"
    );
    assert!(
        stderr.contains("test_helper.bats"),
        "expected bats file in stderr, got:\n{stderr}"
    );
    assert!(
        stderr.contains("(pattern: **/*.bats)"),
        "expected pattern annotation in stderr, got:\n{stderr}"
    );
}

#[test]
fn test_validate_warns_unused_pattern() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_ignore_config(&dir, &["**/never-matches.xyz"]);

    let output = std::process::Command::new(agentspec())
        .arg("validate")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec validate");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "validate failed:\n{stderr}");
    assert!(
        stderr.contains("warning: ignore pattern '**/never-matches.xyz' matched no files"),
        "expected unused-pattern warning in stderr, got:\n{stderr}"
    );
}

#[test]
fn test_compile_silent_without_verbose() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    std::fs::write(
        dir.join("spec/skills/scripted-skill/scripts/test_helper.bats"),
        "bats test\n",
    )
    .expect("failed to write bats file");
    write_ignore_config(&dir, &["**/*.bats"]);

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "compile failed:\n{stderr}");
    // All patterns matched, so no unused-pattern warning fires.
    // Without --verbose, the listing should not appear.
    assert!(
        !stderr.contains("ignoring "),
        "expected no listing in non-verbose compile, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("warning: ignore pattern"),
        "expected no warnings when all patterns match, got:\n{stderr}"
    );
}

#[test]
fn test_compile_verbose_lists_ignored() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    std::fs::write(
        dir.join("spec/skills/scripted-skill/scripts/test_helper.bats"),
        "bats test\n",
    )
    .expect("failed to write bats file");
    write_ignore_config(&dir, &["**/*.bats"]);

    let output = std::process::Command::new(agentspec())
        .args(["compile", "--verbose"])
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile --verbose");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "compile failed:\n{stderr}");
    assert!(
        stderr.contains("ignoring "),
        "expected listing with --verbose, got:\n{stderr}"
    );
    assert!(
        stderr.contains("test_helper.bats"),
        "expected bats file in listing, got:\n{stderr}"
    );
}

#[test]
fn test_sync_dry_run_lists_ignored() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    std::fs::write(
        dir.join("spec/skills/scripted-skill/scripts/test_helper.bats"),
        "bats test\n",
    )
    .expect("failed to write bats file");
    // Inline write retained intentionally: this test verifies that
    // `--dry-run` honours `[spec] ignore` patterns. The `ignore` field is
    // the configuration under test; `write_sync_config` does not emit
    // top-level [spec] blocks.
    std::fs::write(
        dir.join("agentspec.toml"),
        r#"
[spec]
sources_dir = "spec"
ignore = ["**/*.bats"]

[compile]
output_dir = "generated"

[presets.default]
claude = { model = "sonnet" }
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
cursor = { model = "fast" }

[sync.claude]
mode = "user"
"#,
    )
    .expect("failed to write agentspec.toml");

    let home = dir.join("home");
    let output = std::process::Command::new(agentspec())
        .args(["sync", "--dry-run"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --dry-run");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "sync dry-run failed:\n{stderr}");
    assert!(
        stderr.contains("ignoring "),
        "expected listing with --dry-run, got:\n{stderr}"
    );
    assert!(
        stderr.contains("test_helper.bats"),
        "expected bats file in listing, got:\n{stderr}"
    );
}

#[test]
fn test_compile_no_ignore_field_still_works() {
    // Baseline regression: an unconfigured `ignore` field must not change the
    // output of a fixture compile.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    assert!(
        dir.join("generated/claude/agents/test-agent.md").exists(),
        "baseline agent file should exist"
    );
    assert!(
        dir.join("generated/claude/skills/scripted-skill/scripts/helper.sh")
            .exists(),
        "baseline script should exist"
    );
}

// ---------------------------------------------------------------------------
// Hook tests
// ---------------------------------------------------------------------------

/// Drop a representative hook spec set into a fixture-derived directory.
///
/// The shared fixture intentionally omits hooks so that existing user/project
/// sync tests don't trip Phase 1's scope guard. Tests that want hooks call
/// this helper after `setup()`.
///
/// `clippy.toml`'s `allow-expect-in-tests` exemption only covers `#[test]`
/// fns — this helper is a free function, so it uses the `assert!(…is_ok…)`
/// idiom established by `setup()` instead of `.expect()`.
fn install_hook_fixture(dir: &Path) {
    let hooks_dir = dir.join("spec/hooks");
    let scripts_dir = hooks_dir.join("scripts");
    let r = std::fs::create_dir_all(&scripts_dir);
    assert!(r.is_ok(), "create hooks dir: {r:?}");
    let r = std::fs::write(
        hooks_dir.join("hooks.toml"),
        r#"
[hooks.init-thoughts]
events = ["user_prompt_submit"]
script = "scripts/init-thoughts.sh"
description = "Seed THOUGHTS_DIR context at the start of each turn"

[hooks.audit-bash]
events = ["pre_tool_use"]
matcher = "shell"
script = "scripts/audit-bash.sh"

[hooks.subagent-gate]
events = ["subagent_start"]
matcher = "general"
script = "scripts/subagent-gate.sh"
"#,
    );
    assert!(r.is_ok(), "write hooks.toml: {r:?}");
    let r = std::fs::write(
        scripts_dir.join("init-thoughts.sh"),
        "#!/bin/sh\nsource \"$(dirname \"$0\")/_common.sh\"\necho '{\"reply\": \"thoughts loaded\"}'\n",
    );
    assert!(r.is_ok(), "write init-thoughts.sh: {r:?}");
    let r = std::fs::write(scripts_dir.join("audit-bash.sh"), "#!/bin/sh\nexit 0\n");
    assert!(r.is_ok(), "write audit-bash.sh: {r:?}");
    let r = std::fs::write(scripts_dir.join("subagent-gate.sh"), "#!/bin/sh\nexit 0\n");
    assert!(r.is_ok(), "write subagent-gate.sh: {r:?}");
    // _common.sh is a helper sourced by init-thoughts.sh; it's not a hook entry
    // point but must still flow through to the destination so the entry script
    // can `source` it at runtime.
    let r = std::fs::write(
        scripts_dir.join("_common.sh"),
        "#!/bin/sh\n# shared helper sourced by entry scripts\n",
    );
    assert!(r.is_ok(), "write _common.sh: {r:?}");
    set_script_permissions(&scripts_dir);
}

#[test]
fn test_compile_emits_hooks_for_claude_and_cursor() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    // Each provider's hook commands anchor on its own plugin-root env var:
    // Claude → ${CLAUDE_PLUGIN_ROOT}, Cursor → ${CURSOR_PLUGIN_ROOT}.
    for (provider, env_var) in [
        ("claude", "CLAUDE_PLUGIN_ROOT"),
        ("cursor", "CURSOR_PLUGIN_ROOT"),
    ] {
        let json = dir.join(format!("generated/{provider}/hooks/hooks.json"));
        assert!(json.exists(), "{provider}: hooks.json should be emitted");
        let content = std::fs::read_to_string(&json).expect("failed to read hooks.json");
        assert!(
            content.contains("init-thoughts"),
            "{provider}: hooks.json should contain init-thoughts agentspec_id, got:\n{content}"
        );
        let expected = format!("${{{env_var}}}/hooks/scripts/init-thoughts.sh");
        assert!(
            content.contains(&expected),
            "{provider}: hooks.json should anchor scripts under {env_var}, got:\n{content}"
        );

        // Both entry scripts AND the `_common.sh` helper that
        // `init-thoughts.sh` sources must land at the destination — otherwise
        // the entry script fails at runtime trying to source a missing file.
        for script in ["init-thoughts.sh", "audit-bash.sh", "_common.sh"] {
            let path = dir.join(format!("generated/{provider}/hooks/scripts/{script}"));
            assert!(path.exists(), "{provider}: {script} should be emitted");
        }

        // Phase 3: each distinct HookEvent in the fixture gets its own shim
        // under `hooks/scripts/_wrappers/<event>.sh`. The fixture targets
        // `user_prompt_submit` and `pre_tool_use` — assert both shims land
        // and that the `hooks.json` command field invokes the shim with
        // the user script as its argument.
        for event in ["user_prompt_submit", "pre_tool_use"] {
            let shim = dir.join(format!(
                "generated/{provider}/hooks/scripts/_wrappers/{event}.sh"
            ));
            assert!(shim.exists(), "{provider}: shim {event}.sh should exist");
            let shim_body = std::fs::read_to_string(&shim).expect("read shim");
            assert!(
                shim_body.starts_with("#!/usr/bin/env sh"),
                "{provider}: shim {event}.sh should be a POSIX shell script, got:\n{shim_body}"
            );
            assert!(
                shim_body.contains("command -v jq"),
                "{provider}: shim {event}.sh should include the jq guard"
            );
            assert!(
                shim_body.contains(&format!("\"{provider}\"")),
                "{provider}: shim {event}.sh should embed its own provider literal"
            );
        }

        // Command field shape: `<shim> <user_script>`. Both halves must
        // anchor under the same `${env_var}` so the user script path
        // resolves identically to the shim's.
        let expected_command_pre_tool_use = format!(
            "${{{env_var}}}/hooks/scripts/_wrappers/pre_tool_use.sh ${{{env_var}}}/hooks/scripts/audit-bash.sh"
        );
        assert!(
            content.contains(&expected_command_pre_tool_use),
            "{provider}: hooks.json command should invoke per-event shim with user script as argument, expected substring:\n  {expected_command_pre_tool_use}\ngot:\n{content}"
        );
    }

    // Cross-host detection: every shim now carries both providers' jq
    // dialects. Verify the banner identifies the plugin provider and both
    // providers' input jq programs are embedded.
    let claude_shim = dir.join("generated/claude/hooks/scripts/_wrappers/pre_tool_use.sh");
    let cursor_shim = dir.join("generated/cursor/hooks/scripts/_wrappers/pre_tool_use.sh");
    let claude_body = std::fs::read_to_string(&claude_shim).expect("read claude shim");
    let cursor_body = std::fs::read_to_string(&cursor_shim).expect("read cursor shim");
    assert!(
        claude_body.contains("agentspec-generated shim: claude"),
        "Claude shim banner must name claude as the plugin provider"
    );
    assert!(
        cursor_body.contains("agentspec-generated shim: cursor"),
        "Cursor shim banner must name cursor as the plugin provider"
    );
    assert!(
        claude_body.contains("\"claude\"") && claude_body.contains("\"cursor\""),
        "Claude shim must contain both providers' jq dialects for cross-host detection"
    );
    assert!(
        cursor_body.contains("\"claude\"") && cursor_body.contains("\"cursor\""),
        "Cursor shim must contain both providers' jq dialects for cross-host detection"
    );

    // OpenCode does not receive hook output in v1.
    assert!(
        !dir.join("generated/opencode/hooks").exists(),
        "OpenCode should not receive hooks/ directory in v1"
    );
}

#[test]
fn test_compile_hooks_with_args_appear_in_hooks_json() {
    // Two hooks.toml entries on the same script with different args — the
    // feature's motivating case: `args` composes correctly through the
    // full TOML-to-JSON pipeline, and quoting survives serialization into
    // the emitted hooks.json.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let hooks_dir = dir.join("spec/hooks");
    let scripts_dir = hooks_dir.join("scripts");
    std::fs::create_dir_all(&scripts_dir).expect("create hooks dir");
    std::fs::write(
        hooks_dir.join("hooks.toml"),
        r#"
[hooks.audit-bash]
events = ["pre_tool_use"]
matcher = "shell"
script = "scripts/audit-bash.sh"

[hooks.audit-bash-strict]
events = ["pre_tool_use"]
matcher = "shell"
script = "scripts/audit-bash.sh"
args = ["--strict", "two words"]
"#,
    )
    .expect("write hooks.toml");
    std::fs::write(scripts_dir.join("audit-bash.sh"), "#!/bin/sh\nexit 0\n")
        .expect("write audit-bash.sh");
    set_script_permissions(&scripts_dir);

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    for (provider, env_var) in [
        ("claude", "CLAUDE_PLUGIN_ROOT"),
        ("cursor", "CURSOR_PLUGIN_ROOT"),
    ] {
        let json = dir.join(format!("generated/{provider}/hooks/hooks.json"));
        let content = std::fs::read_to_string(&json).expect("read hooks.json");

        let no_args_command = format!(
            "${{{env_var}}}/hooks/scripts/_wrappers/pre_tool_use.sh ${{{env_var}}}/hooks/scripts/audit-bash.sh audit-bash"
        );
        assert!(
            content.contains(&no_args_command),
            "{provider}: hooks.json should contain the no-args command, expected substring:\n  {no_args_command}\ngot:\n{content}"
        );

        let with_args_command = format!(
            "${{{env_var}}}/hooks/scripts/_wrappers/pre_tool_use.sh ${{{env_var}}}/hooks/scripts/audit-bash.sh audit-bash-strict '--strict' 'two words'"
        );
        assert!(
            content.contains(&with_args_command),
            "{provider}: hooks.json should contain the args-carrying command, expected substring:\n  {with_args_command}\ngot:\n{content}"
        );
    }
}

#[test]
fn test_compile_dedups_shim_per_event_per_provider() {
    // Phase 3: two hook specs targeting the same canonical HookEvent
    // produce exactly one shim file per provider — deduplication is
    // per-(provider, event), not per-spec.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let hooks_dir = dir.join("spec/hooks");
    let scripts_dir = hooks_dir.join("scripts");
    std::fs::create_dir_all(&scripts_dir).expect("create scripts dir");
    // Two pre_tool_use hooks (different matchers, different scripts) plus
    // one user_prompt_submit. Expect 2 shims per provider.
    std::fs::write(
        hooks_dir.join("hooks.toml"),
        r#"
[hooks.audit-bash]
events = ["pre_tool_use"]
matcher = "Bash"
script = "scripts/audit-bash.sh"

[hooks.audit-edit]
events = ["pre_tool_use"]
matcher = "Edit"
script = "scripts/audit-edit.sh"

[hooks.greet]
events = ["user_prompt_submit"]
script = "scripts/greet.sh"
"#,
    )
    .expect("write hooks.toml");
    for script in ["audit-bash.sh", "audit-edit.sh", "greet.sh"] {
        std::fs::write(scripts_dir.join(script), "#!/bin/sh\nexit 0\n").expect("write script");
    }
    set_script_permissions(&scripts_dir);

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("run agentspec compile");
    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    for provider in ["claude", "cursor"] {
        let wrappers_dir = dir.join(format!("generated/{provider}/hooks/scripts/_wrappers"));
        let mut entries: Vec<String> = std::fs::read_dir(&wrappers_dir)
            .expect("read _wrappers dir")
            .filter_map(|e| {
                e.ok()
                    .and_then(|e| e.file_name().to_str().map(str::to_string))
            })
            .collect();
        entries.sort();
        assert_eq!(
            entries,
            vec![
                "pre_tool_use.sh".to_string(),
                "user_prompt_submit.sh".to_string()
            ],
            "{provider}: expected exactly one shim per distinct event, got: {entries:?}"
        );
    }
}

#[test]
fn test_compile_multi_event_hook_expands_to_separate_entries() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let hooks_dir = dir.join("spec/hooks");
    let scripts_dir = hooks_dir.join("scripts");
    std::fs::create_dir_all(&scripts_dir).expect("create scripts dir");
    std::fs::write(
        hooks_dir.join("hooks.toml"),
        r#"
[hooks.multi]
events = ["pre_tool_use", "session_start"]
matcher = "Bash"
script = "scripts/multi.sh"
"#,
    )
    .expect("write hooks.toml");
    std::fs::write(scripts_dir.join("multi.sh"), "#!/bin/sh\nexit 0\n").expect("write script");
    set_script_permissions(&scripts_dir);

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("run agentspec compile");
    // Should fail validation: matcher on session_start
    assert!(
        !output.status.success(),
        "compile should reject matcher on non-tool event"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("session_start"),
        "error should name offending event, got:\n{stderr}"
    );

    // Fix: remove the non-tool event, keep only tool events
    std::fs::write(
        hooks_dir.join("hooks.toml"),
        r#"
[hooks.multi]
events = ["pre_tool_use", "post_tool_use"]
matcher = "Bash"
script = "scripts/multi.sh"
"#,
    )
    .expect("rewrite hooks.toml");

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("run agentspec compile");
    assert!(
        output.status.success(),
        "compile should succeed for multi-event with matcher on tool events: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify both events appear in the compiled output for each provider
    for provider in ["claude", "cursor"] {
        let json_path = dir.join(format!("generated/{provider}/hooks/hooks.json"));
        let json = std::fs::read_to_string(&json_path).expect("read hooks.json");
        // The same agentspec_id should appear under both event keys
        let count = json.matches("\"multi\"").count();
        assert!(
            count >= 2,
            "{provider}: expected 'multi' agentspec_id under both events, found {count} occurrences in:\n{json}"
        );
    }

    // Both events should get shim files
    for provider in ["claude", "cursor"] {
        let wrappers = dir.join(format!("generated/{provider}/hooks/scripts/_wrappers"));
        assert!(
            wrappers.join("pre_tool_use.sh").exists(),
            "{provider}: pre_tool_use shim should exist"
        );
        assert!(
            wrappers.join("post_tool_use.sh").exists(),
            "{provider}: post_tool_use shim should exist"
        );
    }
}

#[test]
fn test_compile_prunes_hooks_tests_subdir() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    // Drop a `tests/` subtree alongside the hook scripts (e.g., bats tests).
    let tests_dir = dir.join("spec/hooks/scripts/tests");
    std::fs::create_dir_all(&tests_dir).expect("create tests dir");
    std::fs::write(tests_dir.join("audit.bats"), "@test 'noop' { :; }\n").expect("write bats test");
    // Configure agentspec.toml with the conventional `**/scripts/tests/**`
    // ignore pattern, mirroring the thoughts-workflow plugin's config.
    std::fs::write(
        dir.join("agentspec.toml"),
        r#"
[spec]
sources_dir = "spec"
ignore = ["**/scripts/tests/**"]

[compile]
output_dir = "generated"

[presets.default]
claude = { model = "sonnet" }
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
cursor = { model = "fast" }
"#,
    )
    .expect("write agentspec.toml");

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");
    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    for provider in ["claude", "cursor"] {
        // Helper survives.
        assert!(
            dir.join(format!("generated/{provider}/hooks/scripts/_common.sh"))
                .exists(),
            "{provider}: _common.sh should be preserved"
        );
        // tests/ subtree pruned.
        assert!(
            !dir.join(format!("generated/{provider}/hooks/scripts/tests"))
                .exists(),
            "{provider}: hooks/scripts/tests/ should have been pruned by [spec].ignore"
        );
    }
}

#[test]
fn test_compile_rejects_hook_script_path_traversal() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let hooks_dir = dir.join("spec/hooks");
    std::fs::create_dir_all(hooks_dir.join("scripts")).expect("create scripts dir");
    // `script = "../../etc/passwd"` would read arbitrary files into the
    // generated tree — `validate_hook_script_path` must reject it at load.
    std::fs::write(
        hooks_dir.join("hooks.toml"),
        r#"
[hooks.bad]
events = ["session_start"]
script = "../../etc/passwd"
"#,
    )
    .expect("write hooks.toml");

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "compile should fail on path traversal:\n{stderr}"
    );
    assert!(
        stderr.contains("escapes the hooks directory"),
        "expected path-traversal error, got:\n{stderr}"
    );
}

#[test]
fn test_compile_claude_hooks_json_uses_pascal_case_events() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");
    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let claude_json = std::fs::read_to_string(dir.join("generated/claude/hooks/hooks.json"))
        .expect("failed to read claude hooks.json");
    assert!(
        claude_json.contains("\"UserPromptSubmit\""),
        "Claude should use PascalCase event names, got:\n{claude_json}"
    );
    assert!(
        claude_json.contains("\"PreToolUse\""),
        "Claude should map pre_tool_use to PreToolUse, got:\n{claude_json}"
    );
    // Canonical "shell" → Claude's "Bash".
    assert!(
        claude_json.contains("\"matcher\": \"Bash\""),
        "Claude should translate canonical 'shell' to 'Bash', got:\n{claude_json}"
    );
    // Canonical subagent-type "general" → Claude's "general-purpose".
    assert!(
        claude_json.contains("\"matcher\": \"general-purpose\""),
        "Claude should translate canonical 'general' to 'general-purpose', got:\n{claude_json}"
    );
}

#[test]
fn test_compile_cursor_hooks_json_uses_camel_case_and_version() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");
    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cursor_json = std::fs::read_to_string(dir.join("generated/cursor/hooks/hooks.json"))
        .expect("failed to read cursor hooks.json");
    assert!(
        cursor_json.contains("\"version\": 1"),
        "Cursor hooks.json should carry version: 1, got:\n{cursor_json}"
    );
    // user_prompt_submit → beforeSubmitPrompt (the one non-trivial mapping).
    assert!(
        cursor_json.contains("\"beforeSubmitPrompt\""),
        "Cursor should map user_prompt_submit to beforeSubmitPrompt, got:\n{cursor_json}"
    );
    assert!(
        cursor_json.contains("\"preToolUse\""),
        "Cursor should map pre_tool_use to preToolUse, got:\n{cursor_json}"
    );
    // Cursor places matcher per-entry (not on a wrapping group).
    // Canonical "shell" → Cursor's "Shell" (function-call identifier).
    assert!(
        cursor_json.contains("\"matcher\": \"Shell\""),
        "Cursor should translate canonical 'shell' to 'Shell', got:\n{cursor_json}"
    );
    // Canonical subagent-type "general" → Cursor's "generalPurpose".
    assert!(
        cursor_json.contains("\"matcher\": \"generalPurpose\""),
        "Cursor should translate canonical 'general' to 'generalPurpose', got:\n{cursor_json}"
    );
}

#[test]
fn test_compile_opencode_hook_body_loss_renders_as_count_without_verbose() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);

    let output = std::process::Command::new(agentspec())
        .args(["compile", "--provider", "opencode"])
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile --provider opencode");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");
    assert!(
        stderr.contains("3 specs lost `content`"),
        "expected a counted hook body loss, got:\n{stderr}"
    );
    assert!(
        stderr.contains("(--verbose lists them)"),
        "a counted group without --verbose should point at the flag, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("hook/init-thoughts"),
        "subjects should be withheld without --verbose, got:\n{stderr}"
    );
}

#[test]
fn test_compile_opencode_verbose_lists_hook_body_loss_subjects() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);

    let output = std::process::Command::new(agentspec())
        .args(["compile", "--provider", "opencode", "--verbose"])
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile --provider opencode --verbose");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");
    assert!(
        stderr.contains("hook/init-thoughts"),
        "expected per-spec listing under --verbose, got:\n{stderr}"
    );
    assert!(
        stderr.contains("hook/audit-bash"),
        "expected per-spec listing under --verbose, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("(--verbose lists them)"),
        "the hint is for the non-verbose shape only, got:\n{stderr}"
    );
}

#[test]
fn test_sync_claude_plugin_mode_writes_hooks() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    let dest = dir.join("plugin-claude");
    write_sync_config(
        &dir,
        &[SyncEntry::new("claude", "plugin")
            .with_dir(dest.to_str().expect("dest path utf-8"))
            .with_plugin_name("test-plugin")],
    );

    let home = dir.join("home");
    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "sync (plugin mode) should succeed:\n{stderr}"
    );

    assert!(
        dest.join("hooks/hooks.json").exists(),
        "hooks.json should land under the plugin destination"
    );
    assert!(
        dest.join("hooks/scripts/init-thoughts.sh").exists(),
        "hook script should land under the plugin destination"
    );
}

#[test]
fn test_sync_claude_plugin_mode_emits_plugin_manifest() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    let dest = dir.join("plugin-claude");
    write_sync_config(
        &dir,
        &[SyncEntry::new("claude", "plugin")
            .with_dir(dest.to_str().expect("dest path utf-8"))
            .with_plugin_name("tw")
            .with_plugin_version("0.1.0")
            .with_plugin_description("Thoughts workflow")
            .with_plugin_author("Jason", Some("jason@example.com"))],
    );

    let home = dir.join("home");
    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "plugin-mode sync should succeed:\n{stderr}"
    );

    let manifest_path = dest.join(".claude-plugin/plugin.json");
    assert!(
        manifest_path.exists(),
        "Claude plugin manifest should land at .claude-plugin/plugin.json"
    );
    let manifest_str =
        std::fs::read_to_string(&manifest_path).expect("read plugin.json should succeed");
    let parsed: serde_json::Value =
        serde_json::from_str(&manifest_str).expect("plugin.json should be valid JSON");
    assert_eq!(parsed["name"], "tw");
    assert_eq!(parsed["version"], "0.1.0");
    assert_eq!(parsed["description"], "Thoughts workflow");
    assert_eq!(parsed["author"]["name"], "Jason");
    assert_eq!(parsed["author"]["email"], "jason@example.com");

    // Hook command anchors use ${CLAUDE_PLUGIN_ROOT} in plugin mode.
    let hooks_str = std::fs::read_to_string(dest.join("hooks/hooks.json"))
        .expect("read hooks.json should succeed");
    assert!(
        hooks_str.contains("${CLAUDE_PLUGIN_ROOT}/hooks/scripts/"),
        "Claude plugin-mode hooks should anchor at \\${{CLAUDE_PLUGIN_ROOT}}:\n{hooks_str}"
    );
}

#[test]
fn test_sync_cursor_plugin_mode_emits_plugin_manifest_when_fields_set() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    let dest = dir.join("plugin-cursor");
    write_sync_config(
        &dir,
        &[SyncEntry::new("cursor", "plugin")
            .with_dir(dest.to_str().expect("dest path utf-8"))
            .with_plugin_name("tw")
            .with_plugin_version("0.1.0")],
    );

    let home = dir.join("home");
    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "cursor"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "plugin-mode cursor sync should succeed:\n{stderr}"
    );

    let manifest_path = dest.join(".cursor-plugin/plugin.json");
    assert!(
        manifest_path.exists(),
        "Cursor plugin manifest should land at .cursor-plugin/plugin.json"
    );
    let parsed: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path).expect("read cursor plugin.json"),
    )
    .expect("cursor plugin.json should be valid JSON");
    assert_eq!(parsed["name"], "tw");
    assert_eq!(parsed["version"], "0.1.0");

    // Cursor plugin-mode hooks anchor at ${CURSOR_PLUGIN_ROOT}, NOT CLAUDE_PLUGIN_ROOT.
    let hooks_str = std::fs::read_to_string(dest.join("hooks/hooks.json"))
        .expect("read cursor hooks.json should succeed");
    assert!(
        hooks_str.contains("${CURSOR_PLUGIN_ROOT}/hooks/scripts/"),
        "Cursor plugin-mode hooks should anchor at \\${{CURSOR_PLUGIN_ROOT}}:\n{hooks_str}"
    );
    assert!(
        !hooks_str.contains("CLAUDE_PLUGIN_ROOT"),
        "Cursor plugin-mode hooks must NOT reference CLAUDE_PLUGIN_ROOT:\n{hooks_str}"
    );
}

#[test]
fn test_sync_claude_plugin_mode_remove_cleans_manifest() {
    // agentspec remove must clean up the plugin tree including
    // `.claude-plugin/plugin.json` and the manifest sidecar, and rmdir
    // the empty `.claude-plugin/` directory afterwards.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    let dest = dir.join("plugin-claude");
    write_sync_config(
        &dir,
        &[SyncEntry::new("claude", "plugin")
            .with_dir(dest.to_str().expect("dest path utf-8"))
            .with_plugin_name("tw")],
    );

    let home = dir.join("home");
    let sync = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync");
    assert!(
        sync.status.success(),
        "sync setup: {}",
        String::from_utf8_lossy(&sync.stderr)
    );
    assert!(dest.join(".claude-plugin/plugin.json").exists());

    let remove = std::process::Command::new(agentspec())
        .args(["remove", "--provider", "claude"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec remove");
    let stderr = String::from_utf8_lossy(&remove.stderr);
    assert!(remove.status.success(), "remove should succeed:\n{stderr}");

    assert!(
        !dest.join(".claude-plugin/plugin.json").exists(),
        "plugin.json should be removed"
    );
    assert!(
        !dest.join(".claude-plugin").exists(),
        ".claude-plugin/ should be rmdir'd after manifest removal (empty)"
    );
}

#[test]
fn test_sync_plugin_mode_rejects_missing_plugin_name() {
    // Plugin mode requires plugin-name; validation must error with a clear
    // message naming the offending provider.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let dest = dir.join("plugin-claude");
    write_sync_config(
        &dir,
        &[SyncEntry::new("claude", "plugin").with_dir(dest.to_str().expect("dest path utf-8"))],
    );

    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude"])
        .env("HOME", dir.join("home"))
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync");
    assert!(
        !output.status.success(),
        "sync must fail when plugin-name is missing in plugin mode"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("plugin-name"),
        "error should mention plugin-name, got:\n{stderr}"
    );
    assert!(
        stderr.contains("[sync.claude]"),
        "error should name the offending provider, got:\n{stderr}"
    );
}

#[test]
fn test_sync_rejects_mode_path_at_parse_time() {
    // Pre-1.0: `mode = "path"` is deleted; parsing it must produce an
    // unknown-variant error rather than silently accepting it.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let dest = dir.join("plugin-claude");
    let dest_str = dest.to_str().expect("dest path utf-8");
    std::fs::write(
        dir.join("agentspec.toml"),
        format!("[sync.claude]\nmode = \"path\"\ndir = \"{dest_str}\"\n"),
    )
    .expect("write agentspec.toml");

    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude"])
        .env("HOME", dir.join("home"))
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync");
    assert!(
        !output.status.success(),
        "mode = \"path\" must be rejected at parse time"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown variant") || stderr.contains("failed to parse"),
        "error should explain the parse failure, got:\n{stderr}"
    );
}

#[test]
fn test_compile_does_not_emit_plugin_manifest_even_when_plugin_fields_set() {
    // `agentspec compile` produces canonical, provider-config-dir-agnostic
    // output under `generated/<provider>/`. The presence of `plugin-*` fields
    // in `[sync.<provider>]` config must NOT cause compile to emit a manifest
    // (only sync in plugin mode does).
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    let dest = dir.join("plugin-claude");
    write_sync_config(
        &dir,
        &[SyncEntry::new("claude", "plugin")
            .with_dir(dest.to_str().expect("dest path utf-8"))
            .with_plugin_name("tw")],
    );

    let output = std::process::Command::new(agentspec())
        .args(["compile"])
        .env("HOME", dir.join("home"))
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");
    assert!(
        output.status.success(),
        "compile should succeed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let generated_claude = dir.join("generated/claude");
    assert!(
        generated_claude.exists(),
        "compile should produce generated/claude/"
    );
    assert!(
        !generated_claude.join(".claude-plugin").exists(),
        "compile must NOT emit .claude-plugin/ even when plugin-* fields are set: \
         compile output is provider-config-dir-agnostic"
    );
}

#[test]
fn test_sync_claude_project_mode_merges_hooks_into_settings_json() {
    // Phase 2: Project-mode sync merges agentspec hook entries into the
    // hand-edited `<cwd>/.claude/settings.json` via the CST patcher. Scripts
    // flow through to `<cwd>/.claude/hooks/scripts/`; the host config file
    // gains a `hooks` key with sentinel-tagged entries. Mirrors the User-mode
    // test below; the only difference is the anchor (`${CLAUDE_PROJECT_DIR}`
    // for Project mode) and the destination root (`<cwd>` vs `~`).
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(&dir, &[SyncEntry::new("claude", "project")]);

    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude"])
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "sync (project mode) should succeed in Phase 2:\n{stderr}"
    );

    // Scripts (entry + helpers) under <cwd>/.claude/hooks/scripts/.
    for script in ["init-thoughts.sh", "audit-bash.sh", "_common.sh"] {
        assert!(
            dir.join(format!(".claude/hooks/scripts/{script}")).exists(),
            "{script} should land under <cwd>/.claude/hooks/scripts/"
        );
    }

    // Manifest tracks the emitted scripts so subsequent syncs can prune stale entries.
    let manifest = dir.join(".claude/hooks/.agentspec-manifest.json");
    assert!(
        manifest.exists(),
        "manifest should be written alongside the hooks dir"
    );

    // settings.json merged with sentinel-tagged hook entries, anchored at
    // ${CLAUDE_PROJECT_DIR} for Project mode.
    let settings = dir.join(".claude/settings.json");
    assert!(settings.exists(), "settings.json should be created");
    let content = std::fs::read_to_string(&settings).expect("read settings.json");
    assert!(
        content.contains("\"_agentspec_id\""),
        "settings.json should contain sentinel, got:\n{content}"
    );
    // Phase 3: command wraps the user script with a per-event shim
    // invocation — `<shim> <user_script>`.
    assert!(
        content.contains("CLAUDE_PLUGIN_ROOT=${CLAUDE_PROJECT_DIR}/.claude ${CLAUDE_PROJECT_DIR}/.claude/hooks/scripts/_wrappers/user_prompt_submit.sh ${CLAUDE_PROJECT_DIR}/.claude/hooks/scripts/init-thoughts.sh"),
        "command should set CLAUDE_PLUGIN_ROOT inline and invoke per-event shim under \\${{CLAUDE_PROJECT_DIR}} for Project mode, got:\n{content}"
    );
}

#[test]
fn test_sync_claude_project_mode_with_dest_anchors_hooks_at_project_dir() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    let home = dir.join("home");
    let target = dir.join("custom-dest");
    std::fs::create_dir_all(&target).expect("create custom-dest dir");

    let output = std::process::Command::new(agentspec())
        .args([
            "sync",
            "--provider",
            "claude",
            "--mode",
            "project",
            "--dest",
            target.to_str().expect("dest path should be valid utf-8"),
        ])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --mode project --dest");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "sync (project mode + dest) should succeed:\n{stderr}"
    );

    for script in ["init-thoughts.sh", "audit-bash.sh", "_common.sh"] {
        assert!(
            target
                .join(format!(".claude/hooks/scripts/{script}"))
                .exists(),
            "{script} should land under <dest>/.claude/hooks/scripts/"
        );
    }

    let settings = target.join(".claude/settings.json");
    assert!(
        settings.exists(),
        "settings.json should be at <dest>/.claude/"
    );
    let content = std::fs::read_to_string(&settings).expect("read settings.json");
    assert!(
        content.contains("\"_agentspec_id\""),
        "settings.json should contain sentinel, got:\n{content}"
    );
    assert!(
        content.contains("CLAUDE_PLUGIN_ROOT=${CLAUDE_PROJECT_DIR}/.claude ${CLAUDE_PROJECT_DIR}/.claude/hooks/scripts/_wrappers/user_prompt_submit.sh ${CLAUDE_PROJECT_DIR}/.claude/hooks/scripts/init-thoughts.sh"),
        "command should anchor at ${{CLAUDE_PROJECT_DIR}} even with custom dest, got:\n{content}"
    );
}

#[test]
fn test_sync_claude_user_mode_merges_hooks_into_settings_json() {
    // Phase 2: User-mode sync merges agentspec hook entries into the
    // hand-edited `~/.claude/settings.json` via the CST patcher. Scripts
    // still flow through to `~/.claude/hooks/scripts/`; the host config
    // file gains a `hooks` key with sentinel-tagged entries.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);

    let home = dir.join("home");
    let output = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "sync (user mode) should succeed in Phase 2:\n{stderr}"
    );

    // Scripts (entry + helpers) under hooks/scripts/.
    for script in ["init-thoughts.sh", "audit-bash.sh", "_common.sh"] {
        assert!(
            home.join(format!(".claude/hooks/scripts/{script}"))
                .exists(),
            "{script} should land under ~/.claude/hooks/scripts/"
        );
    }

    // settings.json merged with sentinel-tagged hook entries.
    let settings = home.join(".claude/settings.json");
    assert!(settings.exists(), "settings.json should be created");
    let content = std::fs::read_to_string(&settings).expect("read settings.json");
    assert!(
        content.contains("\"_agentspec_id\""),
        "settings.json should contain sentinel, got:\n{content}"
    );
    // Phase 3: command wraps the user script with a per-event shim
    // invocation — `<shim> <user_script>`.
    assert!(
        content.contains(
            "CLAUDE_PLUGIN_ROOT=$HOME/.claude $HOME/.claude/hooks/scripts/_wrappers/user_prompt_submit.sh $HOME/.claude/hooks/scripts/init-thoughts.sh"
        ),
        "command should set CLAUDE_PLUGIN_ROOT inline and invoke per-event shim under $HOME for User mode, got:\n{content}"
    );
}

// ---------------------------------------------------------------------------
// `agentspec sync` manifest-version refusal
// ---------------------------------------------------------------------------

#[test]
fn test_sync_refuses_higher_manifest_version() {
    // Parity with `test_remove_refuses_higher_manifest_version`: a manifest
    // whose `version` exceeds `MANIFEST_VERSION` must abort `sync` instead of
    // being silently rewritten at the older version (which would destroy any
    // future-version-specific fields).
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    // Plant a manifest with version > MANIFEST_VERSION.
    let agents_dir = home.join(".claude/agents");
    std::fs::create_dir_all(&agents_dir).expect("create agents dir");
    std::fs::write(
        agents_dir.join(".agentspec-manifest.json"),
        r#"{"version":999,"files":{}}"#,
    )
    .expect("write forward-incompatible manifest");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    let stderr = String::from_utf8_lossy(&sync.stderr);
    assert!(
        !sync.status.success(),
        "sync should fail on version mismatch, got success:\n{stderr}"
    );
    assert!(
        stderr.contains("version 999") && stderr.contains("upgrade agentspec"),
        "expected version-refusal error message, got:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// Shared helpers used by sync and remove tests
// ---------------------------------------------------------------------------

/// Describes one `[sync.<provider>]` block emitted by `write_sync_config`.
///
/// Use `SyncEntry::new(provider, mode)` for the canonical `mode = "..."`-only
/// form. Chain `.with_prefix("...")`, `.with_content_prefix("...")`,
/// `.with_dir("...")`, `.with_plugin_name("...")`, etc. to add optional
/// per-block fields.
struct SyncEntry<'a> {
    provider: &'a str,
    mode: &'a str,
    prefix: Option<&'a str>,
    content_prefix: Option<&'a str>,
    dir: Option<&'a str>,
    plugin_name: Option<&'a str>,
    plugin_version: Option<&'a str>,
    plugin_description: Option<&'a str>,
    plugin_author: Option<(&'a str, Option<&'a str>)>,
}

impl<'a> SyncEntry<'a> {
    fn new(provider: &'a str, mode: &'a str) -> Self {
        Self {
            provider,
            mode,
            prefix: None,
            content_prefix: None,
            dir: None,
            plugin_name: None,
            plugin_version: None,
            plugin_description: None,
            plugin_author: None,
        }
    }

    fn with_prefix(mut self, prefix: &'a str) -> Self {
        self.prefix = Some(prefix);
        self
    }

    fn with_content_prefix(mut self, content_prefix: &'a str) -> Self {
        self.content_prefix = Some(content_prefix);
        self
    }

    fn with_dir(mut self, dir: &'a str) -> Self {
        self.dir = Some(dir);
        self
    }

    fn with_plugin_name(mut self, name: &'a str) -> Self {
        self.plugin_name = Some(name);
        self
    }

    fn with_plugin_version(mut self, version: &'a str) -> Self {
        self.plugin_version = Some(version);
        self
    }

    fn with_plugin_description(mut self, description: &'a str) -> Self {
        self.plugin_description = Some(description);
        self
    }

    fn with_plugin_author(mut self, name: &'a str, email: Option<&'a str>) -> Self {
        self.plugin_author = Some((name, email));
        self
    }
}

/// Helper: write a minimal agentspec.toml that configures sync for the named providers.
///
/// Each entry is a `SyncEntry`, e.g. `SyncEntry::new(provider, mode).with_prefix("tw")`.
///
/// Deliberately builds the TOML by string interpolation rather than typed
/// serialization (cf. `.claude/rules/design-principles.md`): all callers pass
/// literal-string fixtures or tempdir paths that contain no TOML metacharacters,
/// and the inline shape keeps the helper trivial to read alongside the inline
/// fixtures it replaces. Add a typed struct + `toml::to_string` if a future
/// caller ever needs to inject untrusted input.
fn write_sync_config(dir: &Path, entries: &[SyncEntry<'_>]) {
    use std::fmt::Write as _;
    let mut sections = String::from(
        r#"
[presets.default]
claude = { model = "sonnet" }
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
cursor = { model = "fast" }
"#,
    );
    for entry in entries {
        // Writes to a `String` are infallible.
        let _ = writeln!(
            sections,
            "\n[sync.{}]\nmode = \"{}\"",
            entry.provider, entry.mode
        );
        if let Some(prefix) = entry.prefix {
            let _ = writeln!(sections, "prefix = \"{prefix}\"");
        }
        if let Some(content_prefix) = entry.content_prefix {
            let _ = writeln!(sections, "content-prefix = \"{content_prefix}\"");
        }
        if let Some(dir_val) = entry.dir {
            let _ = writeln!(sections, "dir = \"{dir_val}\"");
        }
        if let Some(name) = entry.plugin_name {
            let _ = writeln!(sections, "plugin-name = \"{name}\"");
        }
        if let Some(version) = entry.plugin_version {
            let _ = writeln!(sections, "plugin-version = \"{version}\"");
        }
        if let Some(description) = entry.plugin_description {
            let _ = writeln!(sections, "plugin-description = \"{description}\"");
        }
        if let Some((name, email)) = entry.plugin_author {
            if let Some(email) = email {
                let _ = writeln!(
                    sections,
                    "plugin-author = {{ name = \"{name}\", email = \"{email}\" }}"
                );
            } else {
                let _ = writeln!(sections, "plugin-author = {{ name = \"{name}\" }}");
            }
        }
    }
    let r = std::fs::write(dir.join("agentspec.toml"), sections);
    assert!(r.is_ok(), "failed to write agentspec.toml: {r:?}");
}

/// Helper: run `agentspec` with HOME set; return the captured Output.
///
/// Returns `io::Result` so spawn failures bubble to the calling test, where
/// `.expect()` is permitted by `clippy.toml`'s `allow-expect-in-tests`.
fn run_agentspec(args: &[&str], dir: &Path, home: &Path) -> std::io::Result<std::process::Output> {
    std::process::Command::new(agentspec())
        .args(args)
        .env("HOME", home)
        .current_dir(dir)
        .output()
}

/// Parses a JSONC file via `jsonc_parser`'s CST and returns the semantic
/// `serde_json::Value`.
///
/// Comments and trivia are stripped — for byte-level fidelity (e.g. comment
/// preservation), assert against the raw file content with `.contains(...)`
/// instead.
///
/// `clippy::expect_used` is denied at crate level and `clippy.toml`'s test
/// exemption only covers `#[test]` functions, not free helpers like this
/// one. A narrow `#[allow]` is more readable than the alternative
/// `assert!(...is_ok())` + `unreachable!()` dance.
#[allow(dead_code, clippy::expect_used)] // used by Phase 3 tests below
fn read_jsonc_normalized(path: &Path) -> serde_json::Value {
    let content = std::fs::read_to_string(path).expect("read jsonc file");
    let root =
        jsonc_parser::cst::CstRootNode::parse(&content, &jsonc_parser::ParseOptions::default())
            .expect("parse jsonc file");
    root.to_serde_value().expect("jsonc to serde value")
}

// ---------------------------------------------------------------------------
// `agentspec remove` tests (Phase 1: CLI scaffold)
// ---------------------------------------------------------------------------

#[test]
fn test_remove_help_lists_subcommand() {
    let output = std::process::Command::new(agentspec())
        .args(["--help"])
        .output()
        .expect("failed to run agentspec --help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "--help failed:\n{stdout}");
    assert!(
        stdout.contains("remove"),
        "expected `remove` in --help output, got:\n{stdout}"
    );
}

#[test]
fn test_remove_with_no_providers_configured_is_clean_exit() {
    // No `[sync.*]` blocks: `agentspec remove` must exit 0 and report
    // "nothing to remove" rather than `bail!`-ing the way `sync` does.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = tmp.path();
    std::fs::write(
        dir.join("agentspec.toml"),
        r#"
[spec]
sources_dir = "spec"
"#,
    )
    .expect("failed to write agentspec.toml");

    let home = dir.join("home");
    let output = std::process::Command::new(agentspec())
        .args(["remove"])
        .env("HOME", &home)
        .current_dir(dir)
        .output()
        .expect("failed to run agentspec remove");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "remove with no providers should exit 0:\n{stderr}"
    );
    assert!(
        stderr.contains("nothing to remove"),
        "expected 'nothing to remove' in stderr, got:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// `agentspec remove` tests (Phase 2: standalone file removal)
// ---------------------------------------------------------------------------

#[test]
fn test_remove_after_sync_user_mode_deletes_all_tracked_files() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    let stderr = String::from_utf8_lossy(&sync.stderr);
    assert!(sync.status.success(), "sync failed:\n{stderr}");
    assert!(
        home.join(".claude/agents/test-agent.md").exists(),
        "sync did not write agent file"
    );
    assert!(
        home.join(".claude/skills/basic-skill/SKILL.md").exists(),
        "sync did not write skill"
    );

    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    let stderr = String::from_utf8_lossy(&remove.stderr);
    assert!(remove.status.success(), "remove failed:\n{stderr}");

    // Tracked files and manifests are gone.
    assert!(
        !home.join(".claude/agents/test-agent.md").exists(),
        "agent file should have been removed"
    );
    assert!(
        !home.join(".claude/agents").exists(),
        "agents kind dir should have been rmdir'd"
    );
    assert!(
        !home.join(".claude/skills").exists(),
        "skills kind dir should have been rmdir'd"
    );
    assert!(
        !home.join(".claude/rules").exists(),
        "rules kind dir should have been rmdir'd"
    );
    // Parent of kind dirs is left alone — remove never touches it.
    assert!(home.join(".claude").exists(), "parent .claude dir survives");
}

#[test]
fn test_remove_after_sync_project_mode_deletes_all_tracked_files() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("claude", "project")]);
    let home = dir.join("home");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(
        sync.status.success(),
        "sync failed:\n{}",
        String::from_utf8_lossy(&sync.stderr)
    );
    // Project mode writes to `<cwd>/.claude/...`
    assert!(
        dir.join(".claude/agents/test-agent.md").exists(),
        "project sync did not write agent"
    );

    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(
        remove.status.success(),
        "remove failed:\n{}",
        String::from_utf8_lossy(&remove.stderr)
    );

    assert!(!dir.join(".claude/agents").exists());
    assert!(!dir.join(".claude/skills").exists());
    assert!(!dir.join(".claude/rules").exists());
}

#[test]
fn test_remove_per_provider_scoping_leaves_other_providers_intact() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(
        &dir,
        &[
            SyncEntry::new("claude", "user"),
            SyncEntry::new("cursor", "user"),
        ],
    );
    let home = dir.join("home");

    let sync = run_agentspec(&["sync"], &dir, &home).expect("agentspec spawn");
    assert!(
        sync.status.success(),
        "sync failed:\n{}",
        String::from_utf8_lossy(&sync.stderr)
    );
    assert!(home.join(".claude/agents/test-agent.md").exists());
    assert!(home.join(".cursor/agents/test-agent.md").exists());

    // Remove only claude — cursor should be untouched.
    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(
        remove.status.success(),
        "remove failed:\n{}",
        String::from_utf8_lossy(&remove.stderr)
    );

    assert!(
        !home.join(".claude/agents").exists(),
        "claude should be cleaned up"
    );
    assert!(
        home.join(".cursor/agents/test-agent.md").exists(),
        "cursor must not be affected by --provider claude"
    );
}

#[test]
fn test_remove_without_prior_sync_is_no_op() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    // No sync first — every dest dir is missing.
    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    let stderr = String::from_utf8_lossy(&remove.stderr);
    assert!(remove.status.success(), "remove failed:\n{stderr}");
    // The pipeline runs without error; nothing to report (no manifests existed).
}

#[test]
fn test_remove_idempotent() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    let remove1 =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(
        remove1.status.success(),
        "first remove failed:\n{}",
        String::from_utf8_lossy(&remove1.stderr)
    );

    let remove2 =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(
        remove2.status.success(),
        "second remove (idempotent) failed:\n{}",
        String::from_utf8_lossy(&remove2.stderr)
    );
}

#[test]
fn test_remove_dry_run_writes_nothing() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());
    let agent_path = home.join(".claude/agents/test-agent.md");
    assert!(agent_path.exists());

    let remove = run_agentspec(
        &["remove", "--provider", "claude", "--dry-run"],
        &dir,
        &home,
    )
    .expect("agentspec spawn");
    let stderr = String::from_utf8_lossy(&remove.stderr);
    assert!(remove.status.success(), "dry-run remove failed:\n{stderr}");
    // Every non-blank stderr line must be tagged with `[dry-run] ` so piped
    // logs are unambiguous about which lines reflect dry-run vs. live action.
    for line in stderr.lines() {
        if line.is_empty() {
            continue;
        }
        assert!(
            line.starts_with("[dry-run] "),
            "every non-blank dry-run stderr line must start with '[dry-run] ', got line: {line:?}\nfull stderr:\n{stderr}"
        );
    }
    // Files survive a dry run.
    assert!(agent_path.exists(), "dry-run remove must not delete files");
    assert!(
        home.join(".claude/agents/.agentspec-manifest.json")
            .exists(),
        "dry-run must not delete manifests"
    );
}

#[test]
fn test_remove_dry_run_predicts_dest_dir_rmdir() {
    // Pins the dry-run preview behavior for dest-dir teardown: when only
    // tracked content lives under the dest dir, dry-run must report "would
    // rmdir dest dir"; when an unmanaged file is present it must not.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    // Clean dest dir: only manifest + tracked files. Dry-run should predict rmdir.
    let remove = run_agentspec(
        &["remove", "--provider", "claude", "--dry-run"],
        &dir,
        &home,
    )
    .expect("agentspec spawn");
    let stderr = String::from_utf8_lossy(&remove.stderr);
    assert!(remove.status.success(), "dry-run failed:\n{stderr}");
    assert!(
        stderr.contains("would rmdir dest dir"),
        "dry-run should predict dest-dir rmdir when only tracked content remains, got:\n{stderr}"
    );

    // Drop an unmanaged file into agents dest dir. Dry-run must no longer
    // claim "would rmdir dest dir" for that destination.
    let user_file = home.join(".claude/agents/user-authored.md");
    std::fs::write(&user_file, "user content\n").expect("failed to write user file");

    let remove = run_agentspec(
        &["remove", "--provider", "claude", "--dry-run"],
        &dir,
        &home,
    )
    .expect("agentspec spawn");
    let stderr = String::from_utf8_lossy(&remove.stderr);
    assert!(remove.status.success(), "dry-run failed:\n{stderr}");
    // The agents-line should report "would remove N file(s) + manifest" without
    // the dest-dir rmdir clause.
    let agents_line = stderr
        .lines()
        .find(|l| l.contains("agents") && l.contains("would remove"))
        .unwrap_or_else(|| panic!("expected an agents 'would remove' line, got:\n{stderr}"));
    assert!(
        !agents_line.contains("would rmdir"),
        "agents line must not predict rmdir while unmanaged file is present, got:\n{agents_line}"
    );
}

#[test]
fn test_remove_tolerates_missing_manifest() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    // Hand-delete the agents manifest (simulating a partial cleanup).
    let manifest = home.join(".claude/agents/.agentspec-manifest.json");
    std::fs::remove_file(&manifest).expect("failed to delete manifest");

    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(
        remove.status.success(),
        "remove with missing manifest should succeed:\n{}",
        String::from_utf8_lossy(&remove.stderr)
    );
    // The agents/ directory + files survive (manifest is the source of truth).
    assert!(
        home.join(".claude/agents/test-agent.md").exists(),
        "files in a manifest-less dir are left alone"
    );
}

#[test]
fn test_remove_tolerates_pre_deleted_tracked_file() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    // Hand-delete one tracked file before remove.
    let agent = home.join(".claude/agents/test-agent.md");
    std::fs::remove_file(&agent).expect("failed to pre-delete tracked file");

    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    let stderr = String::from_utf8_lossy(&remove.stderr);
    assert!(
        remove.status.success(),
        "remove with pre-deleted file should succeed:\n{stderr}"
    );
    assert!(
        stderr.contains("warning") && stderr.contains("test-agent.md"),
        "expected warning about absent file, got:\n{stderr}"
    );
    // The dir is otherwise cleaned up.
    assert!(!home.join(".claude/agents").exists());
}

#[test]
fn test_remove_leaves_unmanaged_files_in_dest_dir() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    // Drop a user-authored file into the agents dest dir.
    let user_file = home.join(".claude/agents/user-authored.md");
    std::fs::write(&user_file, "user content\n").expect("failed to write user file");

    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(
        remove.status.success(),
        "remove failed:\n{}",
        String::from_utf8_lossy(&remove.stderr)
    );
    assert!(user_file.exists(), "user-authored file must survive");
    // The dir is not rmdir'd because the user file makes it non-empty.
    assert!(
        home.join(".claude/agents").exists(),
        "dest dir must remain (rmdir would have failed with NotEmpty)"
    );
    // Tracked files are gone.
    assert!(!home.join(".claude/agents/test-agent.md").exists());
    assert!(
        !home
            .join(".claude/agents/.agentspec-manifest.json")
            .exists(),
        "manifest is removed even when dir survives"
    );
}

#[test]
fn test_remove_refuses_higher_manifest_version() {
    // Parity with `test_sync_refuses_higher_manifest_version`: both call sites
    // share the strict-by-default `Manifest::load`, so a forward-incompatible
    // manifest is rejected end-to-end on the remove path too.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    // Plant a manifest with version > MANIFEST_VERSION.
    let agents_dir = home.join(".claude/agents");
    std::fs::create_dir_all(&agents_dir).expect("create agents dir");
    std::fs::write(
        agents_dir.join(".agentspec-manifest.json"),
        r#"{"version":999,"files":{}}"#,
    )
    .expect("write forward-incompatible manifest");

    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    let stderr = String::from_utf8_lossy(&remove.stderr);
    assert!(
        !remove.status.success(),
        "remove should fail on version mismatch, got success:\n{stderr}"
    );
    assert!(
        stderr.contains("version 999") && stderr.contains("upgrade agentspec"),
        "expected version-refusal error message, got:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// `agentspec remove` tests (Phase 3: Claude/Cursor settings tidy)
// ---------------------------------------------------------------------------

#[test]
fn test_remove_strips_claude_owned_entries_from_settings_json() {
    // Seed a user-authored top-level key (`permissions`) so the host file
    // survives the remove cleanup — otherwise the new delete-on-empty
    // behavior removes it and the strip assertion becomes vacuous.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");
    let settings = home.join(".claude/settings.json");
    std::fs::create_dir_all(settings.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        &settings,
        "{\n  \"permissions\": { \"allow\": [\"Read\"] }\n}\n",
    )
    .expect("seed user settings.json");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(
        sync.status.success(),
        "sync failed:\n{}",
        String::from_utf8_lossy(&sync.stderr)
    );
    assert!(settings.exists(), "settings.json should exist after sync");
    let pre = std::fs::read_to_string(&settings).expect("read settings.json");
    assert!(
        pre.contains("\"_agentspec_id\""),
        "sync should write sentinel"
    );

    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(
        remove.status.success(),
        "remove failed:\n{}",
        String::from_utf8_lossy(&remove.stderr)
    );

    assert!(
        settings.exists(),
        "settings.json must survive when user-authored top-level keys remain"
    );
    let post = std::fs::read_to_string(&settings).expect("read settings.json");
    assert!(
        !post.contains("\"_agentspec_id\""),
        "sentinels must be gone, got:\n{post}"
    );
    assert!(
        post.contains("\"permissions\""),
        "user-authored permissions must round-trip, got:\n{post}"
    );
}

#[test]
fn test_remove_strips_cursor_owned_entries_from_hooks_json() {
    // Seed a user-authored top-level key (`customKey`) — beyond the
    // version-only Cursor carve-out — so the host file survives remove.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(&dir, &[SyncEntry::new("cursor", "user")]);
    let home = dir.join("home");
    let hooks = home.join(".cursor/hooks.json");
    std::fs::create_dir_all(hooks.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        &hooks,
        "{\n  \"version\": 1,\n  \"customKey\": \"value\"\n}\n",
    )
    .expect("seed user hooks.json");

    let sync =
        run_agentspec(&["sync", "--provider", "cursor"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    assert!(hooks.exists());
    let pre = std::fs::read_to_string(&hooks).expect("read hooks.json");
    assert!(pre.contains("\"_agentspec_id\""));

    let remove =
        run_agentspec(&["remove", "--provider", "cursor"], &dir, &home).expect("agentspec spawn");
    assert!(remove.status.success());

    assert!(
        hooks.exists(),
        "hooks.json must survive when user-authored top-level keys remain"
    );
    let post = std::fs::read_to_string(&hooks).expect("read hooks.json");
    assert!(
        !post.contains("\"_agentspec_id\""),
        "sentinels must be gone, got:\n{post}"
    );
    assert!(
        post.contains("\"customKey\""),
        "user-authored top-level key must round-trip, got:\n{post}"
    );
}

#[test]
fn test_remove_preserves_user_authored_entries_in_settings_json() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    // Pre-populate settings.json with a user-authored entry plus a non-hooks key.
    let settings = home.join(".claude/settings.json");
    std::fs::create_dir_all(settings.parent().expect("parent")).expect("mkdir");
    let initial = r#"{
  "model": "opus",
  "hooks": {
    "SessionStart": [
      { "matcher": "*", "hooks": [{ "type": "command", "command": "user-script.sh" }] }
    ]
  }
}
"#;
    std::fs::write(&settings, initial).expect("write initial settings.json");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(remove.status.success());

    let parsed = read_jsonc_normalized(&settings);
    let model = parsed.get("model").and_then(|v| v.as_str());
    assert_eq!(model, Some("opus"), "model must survive: {parsed:?}");

    let user_cmd = parsed
        .pointer("/hooks/SessionStart/0/hooks/0/command")
        .and_then(|v| v.as_str());
    assert_eq!(
        user_cmd,
        Some("user-script.sh"),
        "user-authored command must survive: {parsed:?}"
    );
}

#[test]
fn test_remove_drops_empty_event_arrays_in_settings_json() {
    // After remove, any event array that becomes empty (because all its
    // entries were agentspec-owned) is dropped — leaving no `SessionStart`
    // key behind. Seed a user-authored top-level key so the host file
    // survives the new delete-on-empty cleanup.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");
    let settings = home.join(".claude/settings.json");
    std::fs::create_dir_all(settings.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        &settings,
        "{\n  \"permissions\": { \"allow\": [\"Read\"] }\n}\n",
    )
    .expect("seed user settings.json");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(remove.status.success());

    let parsed = read_jsonc_normalized(&settings);
    // No SessionStart key should remain.
    let session_start = parsed.pointer("/hooks/SessionStart");
    assert!(
        session_start.is_none(),
        "SessionStart should not survive remove, got: {parsed:?}"
    );
}

#[test]
fn test_remove_deletes_settings_when_only_agentspec_content_was_present() {
    // settings.json starts empty (no user content), sync adds agentspec
    // hooks, remove must delete the host file outright (the prior contract
    // — "drop the hooks key, leave settings.json as `{}`" — was inverted
    // for TODO.md #20). The .claude parent directory is rmdir'd as well
    // because all kind dirs and the host file are gone.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(remove.status.success());

    let settings = home.join(".claude/settings.json");
    assert!(
        !settings.exists(),
        "settings.json should be deleted when only agentspec content was present"
    );
    assert!(
        !home.join(".claude").exists(),
        ".claude should be rmdir'd when settings.json is the last thing in it"
    );
}

#[test]
fn test_remove_preserves_top_level_hooks_key_when_user_entries_remain() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    let settings = home.join(".claude/settings.json");
    std::fs::create_dir_all(settings.parent().expect("parent")).expect("mkdir");
    let initial = r#"{
  "hooks": {
    "SessionStart": [
      { "matcher": "*", "hooks": [{ "type": "command", "command": "user-script.sh" }] }
    ]
  }
}
"#;
    std::fs::write(&settings, initial).expect("write initial");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(remove.status.success());

    let parsed = read_jsonc_normalized(&settings);
    assert!(
        parsed.pointer("/hooks/SessionStart/0").is_some(),
        "user entry under SessionStart should survive, got: {parsed:?}"
    );
}

#[test]
fn test_remove_dry_run_reports_user_entries_remaining() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    let settings = home.join(".claude/settings.json");
    std::fs::create_dir_all(settings.parent().expect("parent")).expect("mkdir");
    // Two user entries.
    let initial = r#"{
  "hooks": {
    "SessionStart": [
      { "matcher": "*", "hooks": [
        { "type": "command", "command": "u1.sh" },
        { "type": "command", "command": "u2.sh" }
      ]}
    ]
  }
}
"#;
    std::fs::write(&settings, initial).expect("write initial");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    let remove = run_agentspec(
        &["remove", "--provider", "claude", "--dry-run"],
        &dir,
        &home,
    )
    .expect("agentspec spawn");
    let stderr = String::from_utf8_lossy(&remove.stderr);
    assert!(remove.status.success(), "dry-run failed:\n{stderr}");
    assert!(
        stderr.contains("2 user-authored entries remain"),
        "expected user-entries-remaining count, got:\n{stderr}"
    );
    assert!(
        stderr.contains("settings.json"),
        "expected host path in stderr, got:\n{stderr}"
    );
}

#[test]
fn test_remove_handles_missing_settings_json() {
    // `agentspec remove` against a config dir that has no settings.json yet
    // (e.g. user never synced hooks) must succeed silently.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(
        remove.status.success(),
        "remove with no settings.json should succeed:\n{}",
        String::from_utf8_lossy(&remove.stderr)
    );
    assert!(
        !home.join(".claude/settings.json").exists(),
        "remove must not create settings.json"
    );
}

#[test]
fn test_remove_preserves_jsonc_comments_in_settings_json() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    let settings = home.join(".claude/settings.json");
    std::fs::create_dir_all(settings.parent().expect("parent")).expect("mkdir");
    let initial = r#"{
  // user line comment
  /* block comment */
  "hooks": {
    "SessionStart": [
      { "matcher": "*", "hooks": [{ "type": "command", "command": "u.sh" }] }
    ]
  }
}
"#;
    std::fs::write(&settings, initial).expect("write initial");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(remove.status.success());

    let post = std::fs::read_to_string(&settings).expect("read settings.json");
    assert!(
        post.contains("// user line comment"),
        "line comment should survive, got:\n{post}"
    );
    assert!(
        post.contains("/* block comment */"),
        "block comment should survive, got:\n{post}"
    );
}

#[test]
fn test_remove_empirical_check_jsonc_parser_last_element_comma() {
    // Empirical check from the plan: a user entry followed by an agentspec
    // entry as the last element. The strong assertion is that
    // remove(sync(initial)) is semantically equivalent to `initial`. The
    // weaker `\n\n\n` backstop catches trivia mishandling around the
    // removed last element.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    let settings = home.join(".claude/settings.json");
    std::fs::create_dir_all(settings.parent().expect("parent")).expect("mkdir");
    let initial = r#"{
  "hooks": {
    "SessionStart": [
      { "matcher": "*", "hooks": [{ "type": "command", "command": "user.sh" }] }
    ]
  }
}
"#;
    std::fs::write(&settings, initial).expect("write initial");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(remove.status.success());

    // Strong assertion: round-trip restores the user-only state.
    let initial_path = tmp.path().join("settings.initial.json");
    std::fs::write(&initial_path, initial).expect("write initial copy");
    let initial_value = read_jsonc_normalized(&initial_path);
    let post_value = read_jsonc_normalized(&settings);
    assert_eq!(
        post_value, initial_value,
        "round-trip should restore the user-only state"
    );

    // Backstop: no stray blank lines from trivia mishandling on the last element.
    let post = std::fs::read_to_string(&settings).expect("read");
    assert!(
        !post.contains("\n\n\n"),
        "stray blank line(s) after remove, got:\n{post}"
    );
}

#[test]
fn test_remove_dry_run_suppresses_zero_count_summary() {
    // With no pre-existing user entries, dry-run remove must NOT emit a
    // "0 user-authored entries remain" line — the print is gated on
    // `count > 0` for both live and dry-run paths to avoid noise on the
    // common fresh-config path.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    let remove = run_agentspec(
        &["remove", "--provider", "claude", "--dry-run"],
        &dir,
        &home,
    )
    .expect("agentspec spawn");
    let stderr = String::from_utf8_lossy(&remove.stderr);
    assert!(remove.status.success(), "dry-run failed:\n{stderr}");
    assert!(
        !stderr.contains("user-authored entry remain")
            && !stderr.contains("user-authored entries remain"),
        "dry-run should suppress 0-count summary, got:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// `agentspec remove` tests (Phase 4: OpenCode instructions tidy)
// ---------------------------------------------------------------------------

#[test]
fn test_remove_strips_agentspec_instructions_from_opencode_json() {
    // Seed a user-authored top-level key (`model`) so the host file survives
    // the new delete-on-empty cleanup — otherwise the strip assertions on
    // `instructions[]` shape would be vacuous.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("opencode", "user")]);
    let home = dir.join("home");
    let opencode = home.join(".config/opencode/opencode.json");
    std::fs::create_dir_all(opencode.parent().expect("parent")).expect("mkdir");
    std::fs::write(&opencode, "{\n  \"model\": \"haiku\"\n}\n").expect("seed user opencode.json");

    let sync =
        run_agentspec(&["sync", "--provider", "opencode"], &dir, &home).expect("agentspec spawn");
    assert!(
        sync.status.success(),
        "sync failed:\n{}",
        String::from_utf8_lossy(&sync.stderr)
    );
    assert!(opencode.exists(), "sync should preserve opencode.json");

    let pre_value = read_jsonc_normalized(&opencode);
    let pre_instructions = pre_value
        .get("instructions")
        .and_then(|v| v.as_array())
        .expect("sync should populate instructions[]");
    assert!(
        !pre_instructions.is_empty(),
        "sync should leave at least one rule path"
    );

    let remove =
        run_agentspec(&["remove", "--provider", "opencode"], &dir, &home).expect("agentspec spawn");
    assert!(
        remove.status.success(),
        "remove failed:\n{}",
        String::from_utf8_lossy(&remove.stderr)
    );

    assert!(
        opencode.exists(),
        "remove must not delete opencode.json when user-authored keys remain"
    );
    let post_value = read_jsonc_normalized(&opencode);
    let rules_root = home.join(".config/opencode/rules");
    let rules_root_str = rules_root.to_string_lossy().into_owned();
    let leftover_under_rules = post_value
        .get("instructions")
        .and_then(|v| v.as_array())
        .is_some_and(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .any(|s| s.starts_with(&rules_root_str))
        });
    assert!(
        !leftover_under_rules,
        "no instructions[] entry should remain under {rules_root_str}, got: {post_value:?}"
    );
    // Tighter guarantee: with no user-authored instructions, the key should be
    // absent (or the array empty) — not simply "no leftover under rules".
    let post_instructions = post_value.get("instructions");
    let absent_or_empty = post_instructions.is_none()
        || post_instructions
            .and_then(serde_json::Value::as_array)
            .is_some_and(Vec::is_empty);
    assert!(
        absent_or_empty,
        "instructions should be absent or empty after remove, got: {post_value:?}"
    );
    // User-authored `model` must round-trip.
    assert_eq!(
        post_value.get("model").and_then(|v| v.as_str()),
        Some("haiku"),
        "user-authored model must round-trip, got: {post_value:?}"
    );
}

#[test]
fn test_remove_deletes_opencode_when_only_agentspec_content_was_present() {
    // opencode.json starts empty, sync adds agentspec instructions, remove
    // must delete the host file outright (the prior contract — "drop the
    // instructions key, leave opencode.json as `{}`" — was inverted for
    // TODO.md #20). The .config/opencode parent is rmdir'd as well.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("opencode", "user")]);
    let home = dir.join("home");

    let sync =
        run_agentspec(&["sync", "--provider", "opencode"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    let remove =
        run_agentspec(&["remove", "--provider", "opencode"], &dir, &home).expect("agentspec spawn");
    assert!(remove.status.success());

    let opencode = home.join(".config/opencode/opencode.json");
    assert!(
        !opencode.exists(),
        "opencode.json should be deleted when only agentspec content was present"
    );
    assert!(
        !home.join(".config/opencode").exists(),
        ".config/opencode should be rmdir'd when opencode.json is the last thing in it"
    );
}

#[test]
fn test_remove_preserves_user_authored_opencode_instructions() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("opencode", "user")]);
    let home = dir.join("home");

    let opencode = home.join(".config/opencode/opencode.json");
    std::fs::create_dir_all(opencode.parent().expect("parent")).expect("mkdir");
    let initial = r#"{
  "model": "haiku",
  "instructions": ["~/notes/personal.md"]
}
"#;
    std::fs::write(&opencode, initial).expect("write initial");

    let sync =
        run_agentspec(&["sync", "--provider", "opencode"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    let remove =
        run_agentspec(&["remove", "--provider", "opencode"], &dir, &home).expect("agentspec spawn");
    assert!(remove.status.success());

    let parsed = read_jsonc_normalized(&opencode);
    let model = parsed.get("model").and_then(|v| v.as_str());
    assert_eq!(model, Some("haiku"), "model must survive: {parsed:?}");
    let instructions = parsed
        .get("instructions")
        .and_then(|v| v.as_array())
        .expect("user instructions[] must survive");
    let strs: Vec<&str> = instructions.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(
        strs,
        vec!["~/notes/personal.md"],
        "user-authored instruction must survive verbatim, got: {parsed:?}"
    );
}

#[test]
fn test_remove_dry_run_reports_opencode_user_entries_remaining() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("opencode", "user")]);
    let home = dir.join("home");

    let opencode = home.join(".config/opencode/opencode.json");
    std::fs::create_dir_all(opencode.parent().expect("parent")).expect("mkdir");
    let initial = r#"{
  "instructions": ["~/notes/u1.md"]
}
"#;
    std::fs::write(&opencode, initial).expect("write initial");

    let sync =
        run_agentspec(&["sync", "--provider", "opencode"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    let remove = run_agentspec(
        &["remove", "--provider", "opencode", "--dry-run"],
        &dir,
        &home,
    )
    .expect("agentspec spawn");
    let stderr = String::from_utf8_lossy(&remove.stderr);
    assert!(remove.status.success(), "dry-run failed:\n{stderr}");
    assert!(
        stderr.contains("1 user-authored entry remain"),
        "expected user-entries-remaining count, got:\n{stderr}"
    );
    assert!(
        stderr.contains("opencode.json"),
        "expected host path in stderr, got:\n{stderr}"
    );
}

#[test]
fn test_remove_keeps_settings_when_user_authored_keys_present() {
    // Pre-authored permissions key + agentspec hooks. After remove, the
    // permissions key survives, the hooks block is gone, and the parent
    // directory is left in place.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");
    let settings = home.join(".claude/settings.json");
    std::fs::create_dir_all(settings.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        &settings,
        "{\n  \"permissions\": { \"allow\": [\"Read\", \"Bash\"] }\n}\n",
    )
    .expect("seed user settings.json");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());
    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(remove.status.success());

    assert!(
        settings.exists(),
        "settings.json must survive when user-authored top-level keys remain"
    );
    assert!(
        home.join(".claude").exists(),
        ".claude parent must survive while settings.json is still present"
    );
    let parsed = read_jsonc_normalized(&settings);
    assert!(
        parsed.get("hooks").is_none(),
        "hooks key should be dropped when no entries remain, got: {parsed:?}"
    );
    let allow = parsed
        .pointer("/permissions/allow")
        .and_then(|v| v.as_array());
    assert!(
        allow.is_some_and(|a| a.iter().any(|v| v.as_str() == Some("Read"))),
        "user permissions must round-trip, got: {parsed:?}"
    );
}

#[test]
fn test_remove_keeps_cursor_hooks_when_user_event_arrays_present() {
    // User has a hand-authored hook entry (no `_agentspec_id`) under a
    // different event from agentspec's. Sync adds agentspec entries; remove
    // strips them but the user's event array survives — file stays.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(&dir, &[SyncEntry::new("cursor", "user")]);
    let home = dir.join("home");
    let hooks = home.join(".cursor/hooks.json");
    std::fs::create_dir_all(hooks.parent().expect("parent")).expect("mkdir");
    // Hand-authored entry under PreToolUse (agentspec fixture installs a
    // SessionStart hook — separate event), so the user's event array is
    // disjoint from agentspec's.
    std::fs::write(
        &hooks,
        r#"{
  "version": 1,
  "hooks": {
    "PreToolUse": [
      { "type": "command", "command": "user-pretool.sh", "matcher": "Bash" }
    ]
  }
}
"#,
    )
    .expect("seed user hooks.json");

    let sync =
        run_agentspec(&["sync", "--provider", "cursor"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());
    let remove =
        run_agentspec(&["remove", "--provider", "cursor"], &dir, &home).expect("agentspec spawn");
    assert!(remove.status.success());

    assert!(
        hooks.exists(),
        "hooks.json must survive when user hook entries remain"
    );
    assert!(
        home.join(".cursor").exists(),
        ".cursor parent must survive while hooks.json is still present"
    );
    let post = std::fs::read_to_string(&hooks).expect("read hooks.json");
    assert!(
        post.contains("user-pretool.sh"),
        "user hook command must round-trip, got:\n{post}"
    );
    assert!(
        !post.contains("\"_agentspec_id\""),
        "agentspec sentinels must be gone, got:\n{post}"
    );
}

#[test]
fn test_remove_does_not_delete_when_no_agentspec_content_was_present() {
    // Hand-authored settings.json without ever running sync — `removed_owned`
    // is 0, so the delete-on-empty branch must not fire even though the file
    // (after a no-op tidy) is structurally minimal. Pins the `removed_owned > 0`
    // guard end-to-end.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");
    let settings = home.join(".claude/settings.json");
    std::fs::create_dir_all(settings.parent().expect("parent")).expect("mkdir");
    let initial = "{\n  \"permissions\": { \"allow\": [\"Read\"] }\n}\n";
    std::fs::write(&settings, initial).expect("seed user settings.json");

    // No sync — go straight to remove.
    let remove =
        run_agentspec(&["remove", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(remove.status.success());

    assert!(
        settings.exists(),
        "settings.json must survive when no agentspec content was present"
    );
    let post = std::fs::read_to_string(&settings).expect("read settings.json");
    assert_eq!(
        post, initial,
        "no-op remove must round-trip the file byte-identical"
    );
    assert!(
        home.join(".claude").exists(),
        ".claude parent must not be touched"
    );
}

#[test]
fn test_remove_dry_run_does_not_delete_host_file() {
    // A full sync followed by `agentspec remove --dry-run` must leave the
    // host file (and its parent) untouched, even though the predicate would
    // delete the file under a live run.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(&dir, &[SyncEntry::new("claude", "user")]);
    let home = dir.join("home");

    let sync =
        run_agentspec(&["sync", "--provider", "claude"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    let settings = home.join(".claude/settings.json");
    let pre = std::fs::read_to_string(&settings).expect("read settings.json");

    let remove = run_agentspec(
        &["remove", "--provider", "claude", "--dry-run"],
        &dir,
        &home,
    )
    .expect("agentspec spawn");
    assert!(
        remove.status.success(),
        "dry-run remove failed:\n{}",
        String::from_utf8_lossy(&remove.stderr)
    );

    assert!(settings.exists(), "dry-run must not delete the host file");
    assert!(
        home.join(".claude").exists(),
        "dry-run must not rmdir the parent"
    );
    let post = std::fs::read_to_string(&settings).expect("read settings.json");
    assert_eq!(post, pre, "dry-run must not modify the host file");
}

#[test]
fn test_remove_opencode_handles_missing_config_file() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("opencode", "user")]);
    let home = dir.join("home");

    // No prior sync → no opencode.json.
    let remove =
        run_agentspec(&["remove", "--provider", "opencode"], &dir, &home).expect("agentspec spawn");
    assert!(
        remove.status.success(),
        "remove with no opencode.json should succeed:\n{}",
        String::from_utf8_lossy(&remove.stderr)
    );
    assert!(
        !home.join(".config/opencode/opencode.json").exists(),
        "remove must not create opencode.json"
    );
}

#[test]
fn test_remove_deletes_opencode_host_file_when_only_agentspec_entries_present() {
    // Sync writes the only entries (no user content); remove must DELETE
    // the host file rather than leaving it as a `{}` residue. The new
    // contract (TODO.md #20) inverts the prior "host file must survive
    // even when empty" guarantee — a file containing only agentspec content
    // is informationally equivalent to no file, and is cleaned up.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    write_sync_config(&dir, &[SyncEntry::new("opencode", "user")]);
    let home = dir.join("home");

    let sync =
        run_agentspec(&["sync", "--provider", "opencode"], &dir, &home).expect("agentspec spawn");
    assert!(sync.status.success());

    let remove =
        run_agentspec(&["remove", "--provider", "opencode"], &dir, &home).expect("agentspec spawn");
    assert!(remove.status.success());

    let opencode = home.join(".config/opencode/opencode.json");
    assert!(
        !opencode.exists(),
        "host file should be deleted when only agentspec entries were present"
    );
    assert!(
        !home.join(".config/opencode").exists(),
        ".config/opencode parent should be rmdir'd"
    );
}

// ---------------------------------------------------------------------------
// `agentspec remove` tests (Phase 5: cross-cutting round-trip)
// ---------------------------------------------------------------------------

#[test]
#[allow(clippy::too_many_lines)] // narrative end-to-end test; extracting subroutines obscures it
fn test_full_round_trip_all_providers() {
    // End-to-end: pre-populate each provider's host config file
    // (settings.json / hooks.json / opencode.json) with one user-authored
    // entry, sync all three providers, then remove all three. Every host
    // file must survive with semantic equality to its initial contents;
    // every dest dir, every manifest must be gone. Sync's spec-file dest
    // dirs (agents/, skills/, rules/, hooks/) start empty so no collision
    // arises — `--force` isn't needed. The contract for unmanaged files
    // *inside* dest dirs is exercised separately by Phase 2's
    // `test_remove_leaves_unmanaged_files_in_dest_dir`.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    write_sync_config(
        &dir,
        &[
            SyncEntry::new("claude", "user"),
            SyncEntry::new("cursor", "user"),
            SyncEntry::new("opencode", "user"),
        ],
    );
    let home = dir.join("home");

    // Seed each host file with one user-authored entry. Use file paths the
    // adapters won't touch:
    //   - Claude: a SessionStart entry under a unique matcher
    //   - Cursor: a SessionStart entry tagged with no _agentspec_id
    //   - OpenCode: an instruction outside the rules dest dir
    let claude_settings = home.join(".claude/settings.json");
    let cursor_hooks = home.join(".cursor/hooks.json");
    let opencode_config = home.join(".config/opencode/opencode.json");
    for parent in [
        claude_settings.parent(),
        cursor_hooks.parent(),
        opencode_config.parent(),
    ]
    .into_iter()
    .flatten()
    {
        std::fs::create_dir_all(parent).expect("mkdir host parent");
    }

    let claude_initial = serde_json::json!({
        "hooks": {
            "SessionStart": [
                { "matcher": "user", "hooks": [{ "type": "command", "command": "u.sh" }] }
            ]
        }
    });
    let cursor_initial = serde_json::json!({
        "version": 1,
        "hooks": {
            "SessionStart": [{ "type": "command", "command": "user.sh" }]
        }
    });
    let opencode_initial = serde_json::json!({
        "instructions": ["~/notes/personal.md"]
    });
    std::fs::write(
        &claude_settings,
        serde_json::to_string_pretty(&claude_initial).expect("ser"),
    )
    .expect("seed claude");
    std::fs::write(
        &cursor_hooks,
        serde_json::to_string_pretty(&cursor_initial).expect("ser"),
    )
    .expect("seed cursor");
    std::fs::write(
        &opencode_config,
        serde_json::to_string_pretty(&opencode_initial).expect("ser"),
    )
    .expect("seed opencode");

    // Sync all configured providers.
    let sync = run_agentspec(&["sync"], &dir, &home).expect("agentspec spawn");
    assert!(
        sync.status.success(),
        "sync failed:\n{}",
        String::from_utf8_lossy(&sync.stderr)
    );

    // Sanity: each provider got both user content + agentspec content.
    // Walk the parsed value rather than relying on byte-level `.contains()`,
    // so a future formatter change doesn't silently break the assertion and
    // the merge layer's grouping is verified — the user's matcher group must
    // still appear at `SessionStart[0]`, distinct from the agentspec group.
    let claude_synced = read_jsonc_normalized(&claude_settings);
    let claude_sessions = claude_synced
        .pointer("/hooks/SessionStart")
        .and_then(|v| v.as_array())
        .expect("SessionStart array after sync");
    let user_group_command = claude_sessions.iter().find_map(|g| {
        let m = g.get("matcher").and_then(|v| v.as_str())?;
        (m == "user")
            .then(|| g.pointer("/hooks/0/command").and_then(|v| v.as_str()))
            .flatten()
    });
    assert_eq!(
        user_group_command,
        Some("u.sh"),
        "user matcher group must survive sync verbatim"
    );
    // agentspec hooks land under whatever event the spec declares — the
    // fixture's `init-thoughts` is `user_prompt_submit`, not `SessionStart`,
    // so walk every event under `/hooks`.
    let claude_hooks_obj = claude_synced
        .pointer("/hooks")
        .and_then(|v| v.as_object())
        .expect("Claude hooks object after sync");
    let agentspec_present = claude_hooks_obj.values().any(|event_arr| {
        event_arr.as_array().is_some_and(|groups| {
            groups.iter().any(|g| {
                g.pointer("/hooks")
                    .and_then(|h| h.as_array())
                    .is_some_and(|hs| hs.iter().any(|h| h.get("_agentspec_id").is_some()))
            })
        })
    });
    assert!(
        agentspec_present,
        "agentspec entry must be present somewhere in Claude hooks after sync"
    );

    let cursor_synced = read_jsonc_normalized(&cursor_hooks);
    let cursor_sessions = cursor_synced
        .pointer("/hooks/SessionStart")
        .and_then(|v| v.as_array())
        .expect("Cursor SessionStart array after sync");
    let cursor_user_present = cursor_sessions
        .iter()
        .any(|e| e.get("command").and_then(|v| v.as_str()) == Some("user.sh"));
    assert!(cursor_user_present, "user entry must survive Cursor sync");

    // Cursor's hooks shape is flatter (no matcher-group wrapper). Walk every
    // event for an agentspec-tagged entry — `init-thoughts` lives under
    // `UserPromptSubmit`, not `SessionStart`.
    let cursor_hooks_obj = cursor_synced
        .pointer("/hooks")
        .and_then(|v| v.as_object())
        .expect("Cursor hooks object after sync");
    let cursor_agentspec_present = cursor_hooks_obj.values().any(|event_arr| {
        event_arr
            .as_array()
            .is_some_and(|entries| entries.iter().any(|e| e.get("_agentspec_id").is_some()))
    });
    assert!(
        cursor_agentspec_present,
        "agentspec entry must be present somewhere in Cursor hooks after sync"
    );

    let opencode_synced = read_jsonc_normalized(&opencode_config);
    let synced_paths: Vec<&str> = opencode_synced
        .get("instructions")
        .and_then(|v| v.as_array())
        .expect("instructions array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(synced_paths.iter().any(|p| p == &"~/notes/personal.md"));
    assert!(synced_paths.iter().any(|p| p.contains("/rules/")));

    // Reverse the sync.
    let remove = run_agentspec(&["remove"], &dir, &home).expect("agentspec spawn");
    assert!(
        remove.status.success(),
        "remove failed:\n{}",
        String::from_utf8_lossy(&remove.stderr)
    );

    // Every manifest-tracked dest dir is gone.
    for kind_dir in [
        ".claude/agents",
        ".claude/skills",
        ".claude/rules",
        ".claude/hooks",
        ".cursor/agents",
        ".cursor/skills",
        ".cursor/rules",
        ".cursor/hooks",
        ".config/opencode/agents",
        ".config/opencode/commands",
        ".config/opencode/rules",
        ".config/opencode/skills",
    ] {
        let p = home.join(kind_dir);
        assert!(!p.exists(), "kind dir should be gone: {}", p.display());
    }

    // Every host file survives with semantic equality to its initial state.
    let claude_post = read_jsonc_normalized(&claude_settings);
    assert_eq!(
        claude_post, claude_initial,
        "claude settings.json must round-trip"
    );
    let cursor_post = read_jsonc_normalized(&cursor_hooks);
    assert_eq!(
        cursor_post, cursor_initial,
        "cursor hooks.json must round-trip"
    );
    let opencode_post = read_jsonc_normalized(&opencode_config);
    assert_eq!(
        opencode_post, opencode_initial,
        "opencode.json must round-trip"
    );
}

// ---------------------------------------------------------------------------
// Phase 4: shim manifest round-trip + cross-provider portability warnings
// ---------------------------------------------------------------------------

/// Replace the fixture's `hooks.toml` with custom content for tests that
/// need an event other than the standard `user_prompt_submit`/`pre_tool_use`
/// pair installed by `install_hook_fixture`.
///
/// Uses the `assert!(…is_ok…)` idiom rather than `.expect()` for the same
/// reason `install_hook_fixture` does: `clippy.toml`'s
/// `allow-expect-in-tests` exemption only covers `#[test]` fns, not free
/// helpers.
fn write_hooks_toml(dir: &Path, toml: &str) {
    let hooks_dir = dir.join("spec/hooks");
    let r = std::fs::create_dir_all(&hooks_dir);
    assert!(r.is_ok(), "mkdir hooks: {r:?}");
    let r = std::fs::write(hooks_dir.join("hooks.toml"), toml);
    assert!(r.is_ok(), "write hooks.toml: {r:?}");
}

/// Install a single `session_start` hook with a placeholder user script.
/// Used by the session-start asymmetry warning tests.
fn install_session_start_hook_fixture(dir: &Path) {
    let scripts_dir = dir.join("spec/hooks/scripts");
    let r = std::fs::create_dir_all(&scripts_dir);
    assert!(r.is_ok(), "create scripts dir: {r:?}");
    write_hooks_toml(
        dir,
        r#"
[hooks.startup]
events = ["session_start"]
script = "scripts/startup.sh"
"#,
    );
    let r = std::fs::write(scripts_dir.join("startup.sh"), "#!/bin/sh\nexit 0\n");
    assert!(r.is_ok(), "write startup.sh: {r:?}");
    set_script_permissions(&scripts_dir);
}

/// Extract just the compile-diagnostic lines from stderr, in order.
///
/// Filters out the `compiled N files for M providers` and `wrote N files to …`
/// lines that bracket them, so tests can assert on order and cardinality of
/// the diagnostic block alone.
fn diagnostic_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|line| !line.starts_with("compiled ") && !line.starts_with("wrote "))
        .collect()
}

#[test]
fn test_sync_remove_round_trip_cleans_shim_files() {
    // Phase 4 manifest validation: shim files (the per-event `_wrappers/`
    // scripts) round-trip cleanly through `agentspec sync` → `remove`.
    // After sync they exist; after remove they're gone, and the
    // `_wrappers/` directory is rmdir'd because it's empty.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    let dest = dir.join("plugin-claude");
    write_sync_config(
        &dir,
        &[SyncEntry::new("claude", "plugin")
            .with_dir(dest.to_str().expect("dest path utf-8"))
            .with_plugin_name("tw")],
    );
    let home = dir.join("home");

    let sync = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("agentspec sync spawn");
    assert!(
        sync.status.success(),
        "sync should succeed: {}",
        String::from_utf8_lossy(&sync.stderr)
    );

    // After sync: both shim files exist on disk.
    let wrappers_dir = dest.join("hooks/scripts/_wrappers");
    assert!(
        wrappers_dir.join("pre_tool_use.sh").exists(),
        "pre_tool_use shim should land after sync"
    );
    assert!(
        wrappers_dir.join("user_prompt_submit.sh").exists(),
        "user_prompt_submit shim should land after sync"
    );

    // Sanity check: manifest exists and references one of the shim paths.
    let manifest_path = dest.join("hooks/.agentspec-manifest.json");
    assert!(
        manifest_path.exists(),
        "hooks manifest should exist after sync"
    );
    let manifest_body = std::fs::read_to_string(&manifest_path).expect("read manifest");
    assert!(
        manifest_body.contains("_wrappers/pre_tool_use.sh"),
        "manifest should track pre_tool_use shim, got:\n{manifest_body}"
    );
    assert!(
        manifest_body.contains("_wrappers/user_prompt_submit.sh"),
        "manifest should track user_prompt_submit shim, got:\n{manifest_body}"
    );

    let remove = std::process::Command::new(agentspec())
        .args(["remove", "--provider", "claude"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("agentspec remove spawn");
    assert!(
        remove.status.success(),
        "remove should succeed: {}",
        String::from_utf8_lossy(&remove.stderr)
    );

    // After remove: shim files gone and `_wrappers/` directory pruned.
    assert!(
        !wrappers_dir.exists(),
        "_wrappers/ directory should be rmdir'd once empty (no surviving children)"
    );
}

#[test]
fn test_sync_orphan_cleanup_removes_stale_shim() {
    // Phase 4 orphan handling: an initial sync emits both shims; mutating
    // `hooks.toml` to remove the `pre_tool_use` hook and syncing again
    // must delete the now-orphaned `_wrappers/pre_tool_use.sh` shim from
    // disk (the manifest-diff sweep recognises it's no longer in the
    // emitted file set).
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    let dest = dir.join("plugin-claude");
    write_sync_config(
        &dir,
        &[SyncEntry::new("claude", "plugin")
            .with_dir(dest.to_str().expect("dest path utf-8"))
            .with_plugin_name("tw")],
    );
    let home = dir.join("home");

    // First sync — both shims land.
    let first = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("agentspec sync spawn");
    assert!(first.status.success(), "first sync should succeed");
    let wrappers_dir = dest.join("hooks/scripts/_wrappers");
    assert!(wrappers_dir.join("pre_tool_use.sh").exists());
    assert!(wrappers_dir.join("user_prompt_submit.sh").exists());

    // Mutate hooks.toml to drop the pre_tool_use entry.
    write_hooks_toml(
        &dir,
        r#"
[hooks.init-thoughts]
events = ["user_prompt_submit"]
script = "scripts/init-thoughts.sh"
"#,
    );

    let second = std::process::Command::new(agentspec())
        .args(["sync", "--provider", "claude"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("agentspec sync spawn");
    assert!(
        second.status.success(),
        "second sync should succeed: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    // Surviving shim still present; orphaned shim is gone.
    assert!(
        wrappers_dir.join("user_prompt_submit.sh").exists(),
        "user_prompt_submit shim must survive the second sync"
    );
    assert!(
        !wrappers_dir.join("pre_tool_use.sh").exists(),
        "pre_tool_use shim must be cleaned by the manifest-diff sweep on second sync"
    );
}

#[test]
fn test_compile_emits_cursor_partial_output_warning_when_cursor_targeted() {
    // Cursor partial-output warning fires whenever Cursor is in the active
    // provider list and any hook spec exists. The standard fixture has
    // both pre_tool_use and user_prompt_submit hooks, so any Cursor-active
    // compile should surface the warning on stderr.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);

    let output = std::process::Command::new(agentspec())
        .args(["compile", "--provider", "cursor"])
        .current_dir(&dir)
        .output()
        .expect("agentspec compile spawn");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");
    assert!(
        stderr.contains("Cursor does not surface a hook's canonical `user_facing_message`"),
        "expected Cursor partial-output limitation on stderr, got:\n{stderr}"
    );
}

#[test]
fn test_compile_does_not_emit_cursor_warning_when_only_claude_targeted() {
    // Negative case for the Cursor partial-output warning: Claude-only
    // compile must NOT surface a Cursor-related warning. Regression guard
    // against future logic that fires the warning regardless of the
    // active provider set.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);

    let output = std::process::Command::new(agentspec())
        .args(["compile", "--provider", "claude"])
        .current_dir(&dir)
        .output()
        .expect("agentspec compile spawn");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");
    assert!(
        !stderr.contains("does not surface a hook's canonical"),
        "Claude-only compile must not surface Cursor warning, got:\n{stderr}"
    );
}

#[test]
fn test_compile_emits_session_start_asymmetry_warning_for_cross_provider_fixture() {
    // session_start asymmetry warning fires only when BOTH Claude AND
    // Cursor are targeted by a session_start hook (cross-provider parity
    // concern). Default compile run (all configured providers) with a
    // session_start fixture should surface the warning.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_session_start_hook_fixture(&dir);

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("agentspec compile spawn");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");
    assert!(
        stderr.contains("session_start asymmetry"),
        "expected session_start asymmetry warning on stderr, got:\n{stderr}"
    );
}

#[test]
fn test_compile_does_not_emit_session_start_warning_when_only_cursor_targeted() {
    // Regression guard for the cross-provider gate: a session_start hook
    // compiled only for Cursor must NOT surface the asymmetry warning —
    // single-provider users don't have a parity expectation to violate.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_session_start_hook_fixture(&dir);

    let output = std::process::Command::new(agentspec())
        .args(["compile", "--provider", "cursor"])
        .current_dir(&dir)
        .output()
        .expect("agentspec compile spawn");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");
    assert!(
        !stderr.contains("session_start asymmetry"),
        "Cursor-only compile must not surface session_start asymmetry warning, got:\n{stderr}"
    );
}

#[test]
fn test_compile_diagnostic_block_order_and_cardinality() {
    // Pins the exact order and count of the compile report. Loss ordering is
    // `(provider, setting, kind, spec_id)` from the `BTreeSet` the subtraction
    // collects into, so it is stable against spec-file edits and adapter
    // iteration order alike.
    //
    // Three fixture facts this depends on: `compile` with no `--provider`
    // targets every `Provider::VARIANTS`; `spec/rules/react-components.md`
    // carries a `paths:` key; and three skills name `preset: default`, which
    // configures a model for all three providers.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_session_start_hook_fixture(&dir);

    let output = std::process::Command::new(agentspec())
        .args(["compile", "--verbose"])
        .current_dir(&dir)
        .output()
        .expect("agentspec compile spawn");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    let lines = diagnostic_lines(&stderr);
    let expected = [
        "not delivered:",
        "cursor: 3 specs lost `model`",
        "skill/agent-invocable-skill",
        "skill/dual-invocable-skill",
        "skill/scripted-skill",
        "cursor: 1 spec lost `tools`",
        "skill/agent-invocable-skill",
        "opencode: 1 spec lost `content` — opencode emits no hook",
        "hook/startup",
        "opencode: 2 specs lost `model`",
        "skill/agent-invocable-skill",
        "skill/dual-invocable-skill",
        "opencode: 2 specs lost `variant`",
        "skill/agent-invocable-skill",
        "skill/dual-invocable-skill",
        "opencode: 1 spec lost `tools`",
        "skill/agent-invocable-skill",
        "opencode: 1 spec lost `paths`",
        "rule/react-components",
        "provider limitations:",
        "Cursor does not surface a hook's canonical `user_facing_message`",
        "session_start asymmetry",
        "11 losses, 2 provider limitations",
    ];
    assert_eq!(
        lines.len(),
        expected.len(),
        "diagnostic line count changed, expected:\n{expected:#?}\ngot:\n{lines:#?}"
    );
    for (idx, marker) in expected.iter().enumerate() {
        assert!(
            lines[idx].contains(marker),
            "diagnostic line {idx} should contain {marker:?}, got: {:?}",
            lines[idx]
        );
    }
}

#[test]
fn test_compile_reports_opencode_skill_preset_loss() {
    // The handed-forward skills condition: a skill names a preset configuring
    // execution that no file OpenCode emits for it can carry. No provider-level
    // capability accessor can express this — `agent-invocable-skill` and a
    // user-invocable skill land on opposite sides of the same accessor — which
    // is why it needed the per-emitted-kind subtraction rather than a
    // `DegradationKind`.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let output = std::process::Command::new(agentspec())
        .args(["compile", "--provider", "opencode", "--verbose"])
        .current_dir(&dir)
        .output()
        .expect("agentspec compile spawn");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    for setting in ["`model`", "`variant`", "`tools`"] {
        let named = stderr.lines().any(|line| line.contains(setting))
            && stderr.contains("skill/agent-invocable-skill");
        assert!(
            named,
            "expected an OpenCode loss naming {setting} for skill/agent-invocable-skill, got:\n{stderr}"
        );
    }
    assert!(
        stderr.contains("no opencode skills file carries `variant`"),
        "expected the derived explanation to name the skills file kind, got:\n{stderr}"
    );
}

#[test]
fn test_compile_reports_cursor_tools_loss() {
    // Cursor reads no tool restriction on any file kind — its documented
    // subagent fields are `name`, `description`, `model`, `readonly`, and
    // `is_background`, and subagents inherit every tool from the parent. This
    // is a silent, undocumented drop before the loss report and the largest
    // single loss it surfaces on a real library.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let output = std::process::Command::new(agentspec())
        .args(["compile", "--provider", "cursor", "--verbose"])
        .current_dir(&dir)
        .output()
        .expect("agentspec compile spawn");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    assert!(
        stderr.contains("lost `tools`") && stderr.contains("skill/agent-invocable-skill"),
        "expected a Cursor `tools` loss naming skill/agent-invocable-skill, got:\n{stderr}"
    );
}

#[test]
fn test_compile_reports_dual_invocable_skill_skill_file_loss() {
    // Per-emitted-kind matching is what stops one output file from masking
    // another. `dual-invocable-skill` emits an OpenCode command file carrying
    // `model` and `variant` and a skill file carrying neither; only the skill
    // file's loss is real, and only per-kind matching reports it. Matching on
    // `(spec, setting)` would see the command file's delivery and stay silent.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let output = std::process::Command::new(agentspec())
        .args(["compile", "--provider", "opencode", "--verbose"])
        .current_dir(&dir)
        .output()
        .expect("agentspec compile spawn");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    assert!(
        stderr.contains("no opencode skills file carries `model`")
            && stderr.contains("skill/dual-invocable-skill"),
        "expected a skills-kind `model` loss naming the dual-invocable skill, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("commands file carries"),
        "the command file carries both values, so no commands-kind loss may be reported:\n{stderr}"
    );
}

#[test]
fn test_compile_skipped_hook_listing_order_and_count() {
    // Three hooks, none emitted for OpenCode. Every one of them loses its
    // body, so the group is categorical and collapses to a count line plus a
    // listing under `--verbose`. Ordering is by `spec_id` from the loss
    // `BTreeSet`, so it is alphabetical and stable against `hooks.toml` edits.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);

    let output = std::process::Command::new(agentspec())
        .args(["compile", "--provider", "opencode", "--verbose"])
        .current_dir(&dir)
        .output()
        .expect("agentspec compile spawn");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    let body_lines: Vec<&str> = diagnostic_lines(&stderr)
        .into_iter()
        .skip_while(|line| !line.contains("lost `content`"))
        .take(4)
        .collect();
    let expected = [
        "opencode: 3 specs lost `content`",
        "hook/audit-bash",
        "hook/init-thoughts",
        "hook/subagent-gate",
    ];
    assert_eq!(
        body_lines.len(),
        expected.len(),
        "expected a counted line plus three subjects, got:\n{body_lines:#?}"
    );
    for (idx, marker) in expected.iter().enumerate() {
        assert!(
            body_lines[idx].contains(marker),
            "line {idx} should contain {marker:?}, got: {:?}",
            body_lines[idx]
        );
    }
}

#[test]
fn test_compile_path_scoped_loss_is_one_categorical_line() {
    // Every path-scoped rule loses `paths` on OpenCode, so the group is
    // categorical and renders one counted line however many rules there are.
    // Cardinality is derived from the intent set rather than declared: if
    // OpenCode ever carried `paths` for some rules and not others, the same
    // code would render per-spec lines instead.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    for id in ["vue-components", "svelte-components"] {
        let body = format!(
            "---\nid: {id}\ndescription: More conventions\npaths:\n  - \"src/{id}/**/*.ts\"\n---\n\n# {id}\n\nBody.\n"
        );
        let r = std::fs::write(dir.join(format!("spec/rules/{id}.md")), body);
        assert!(r.is_ok(), "write {id} rule: {r:?}");
    }

    let output = std::process::Command::new(agentspec())
        .args(["compile", "--provider", "opencode"])
        .current_dir(&dir)
        .output()
        .expect("agentspec compile spawn");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    let paths_lines: Vec<&str> = diagnostic_lines(&stderr)
        .into_iter()
        .filter(|line| line.contains("lost `paths`"))
        .collect();
    assert_eq!(
        paths_lines.len(),
        1,
        "three path-scoped rules must collapse to one counted line, got:\n{paths_lines:#?}"
    );
    assert!(
        paths_lines[0].contains("3 specs lost `paths`"),
        "expected a count of 3, got: {:?}",
        paths_lines[0]
    );
}

#[test]
fn test_compile_emits_no_warnings_for_non_hook_fixture() {
    // Sanity check: a compile without hook specs surfaces no hook-related
    // warnings. With no hook spec to drop, no adapter pushes a
    // hook body loss or `PartialOutputImpl` limitation, and the
    // cross-provider parity gate has no `session_start` hook to fire on.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    // No hook fixture installed.

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("agentspec compile spawn");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");
    assert!(
        !stderr.contains("does not surface a hook's canonical"),
        "non-hook fixture must not surface PartialOutputImpl warning, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("session_start asymmetry"),
        "non-hook fixture must not surface SessionStartAsymmetry warning, got:\n{stderr}"
    );
}

#[test]
#[cfg(unix)]
fn test_compile_resolves_symlinked_supporting_file() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    add_symlink_to_fixture(
        &dir,
        "spec/skills/scripted-skill/scripts/from-helper.sh",
        "helper.sh",
    );
    set_script_permissions(&dir.join("spec/skills/scripted-skill/scripts"));

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    let generated = dir.join("generated/claude/skills/scripted-skill/scripts/from-helper.sh");
    assert!(generated.exists(), "symlinked file not in compiled output");
    assert!(
        !generated
            .symlink_metadata()
            .expect("stat")
            .file_type()
            .is_symlink(),
        "compiled output must be a regular file, not a symlink"
    );

    let original = std::fs::read(dir.join("spec/skills/scripted-skill/scripts/helper.sh"))
        .expect("read original");
    let resolved = std::fs::read(&generated).expect("read generated");
    assert_eq!(original, resolved, "resolved content must match target");
}

#[test]
#[cfg(unix)]
fn test_compile_resolves_symlinked_hook_script() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);

    add_symlink_to_fixture(
        &dir,
        "spec/hooks/scripts/from-skill.sh",
        "../../skills/scripted-skill/scripts/helper.sh",
    );

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    let generated = dir.join("generated/claude/hooks/scripts/from-skill.sh");
    assert!(
        generated.exists(),
        "symlinked hook script not in compiled output"
    );
    assert!(
        !generated
            .symlink_metadata()
            .expect("stat")
            .file_type()
            .is_symlink(),
        "compiled output must be a regular file, not a symlink"
    );

    let original = std::fs::read(dir.join("spec/skills/scripted-skill/scripts/helper.sh"))
        .expect("read original");
    let resolved = std::fs::read(&generated).expect("read generated");
    assert_eq!(original, resolved, "resolved content must match target");
}

#[test]
fn test_validate_surfaces_config_errors() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = setup(&tmp);

    let toml_path = dir.join("agentspec.toml");
    let existing = std::fs::read_to_string(&toml_path).expect("read existing toml");
    let appended =
        format!("{existing}\n[sync.claude]\nmode = \"plugin\"\nplugin-name = \"test\"\n");
    std::fs::write(&toml_path, appended).expect("write toml");

    let home = tmp.path().join("fake-home");
    std::fs::create_dir_all(&home).expect("create fake home");

    let output = run_agentspec(&["validate"], &dir, &home).expect("run agentspec");
    assert!(
        !output.status.success(),
        "agentspec validate should exit non-zero for misconfigured sync target"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no `dir` configured"),
        "stderr should contain the config validation error, got: {stderr}"
    );
}

#[test]
fn test_hook_test_success_path() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);

    let output = std::process::Command::new(agentspec())
        .args(["hook", "test", "audit-bash", "--event", "pre_tool_use"])
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec hook test");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "hook test failed:\n{stderr}");
    assert!(
        stderr.contains("Provider Input"),
        "stderr missing 'Provider Input': {stderr}"
    );
    assert!(
        stderr.contains("Canonical Input"),
        "stderr missing 'Canonical Input': {stderr}"
    );
    assert!(
        stderr.contains("Script Output"),
        "stderr missing 'Script Output': {stderr}"
    );
    assert!(
        stderr.contains("Exit Code"),
        "stderr missing 'Exit Code': {stderr}"
    );
}

#[test]
fn test_hook_test_forwards_args_to_script() {
    // The one thing about `hook test` that actually changed for this
    // feature: the entry's `args` reach the spawned shim's argv, and the
    // two new stderr sections describe what was sent. Everything else
    // about `args` is exercised at the library level (compile output,
    // `hook_command_anchor` quoting) — this pins the CLI-level behavior
    // those tests can't reach.
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let hooks_dir = dir.join("spec/hooks");
    let scripts_dir = hooks_dir.join("scripts");
    std::fs::create_dir_all(&scripts_dir).expect("create hooks dir");
    std::fs::write(
        hooks_dir.join("hooks.toml"),
        r#"
[hooks.audit-bash-strict]
events = ["pre_tool_use"]
matcher = "shell"
script = "scripts/audit-bash.sh"
args = ["--strict", "two words"]
"#,
    )
    .expect("write hooks.toml");
    std::fs::write(
        scripts_dir.join("audit-bash.sh"),
        // Stdout must be empty or valid canonical JSON (the shim rejects
        // anything else), so the observed-argv marker goes to stderr.
        "#!/bin/sh\ncat > /dev/null\nprintf 'received: %s / argc=%s' \"$1\" \"$#\" >&2\n",
    )
    .expect("write audit-bash.sh");
    set_script_permissions(&scripts_dir);

    let output = std::process::Command::new(agentspec())
        .args([
            "hook",
            "test",
            "audit-bash-strict",
            "--event",
            "pre_tool_use",
        ])
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec hook test");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "hook test failed:\n{stderr}");

    assert!(
        stderr.contains("Provider Registration"),
        "stderr missing 'Provider Registration': {stderr}"
    );
    assert!(
        stderr.contains("'--strict' 'two words'"),
        "Provider Registration should show quoted args: {stderr}"
    );
    assert!(
        stderr.contains("Script Argv"),
        "stderr missing 'Script Argv': {stderr}"
    );
    assert!(
        stderr.contains(r#"$1: "--strict""#) && stderr.contains(r#"$2: "two words""#),
        "Script Argv should list each argument by position: {stderr}"
    );
    assert!(
        stderr.contains("received: --strict / argc=2"),
        "script should have observed the forwarded args on its own argv: {stderr}"
    );
}

#[test]
fn test_hook_test_nonexistent_hook_errors() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);

    let output = std::process::Command::new(agentspec())
        .args([
            "hook",
            "test",
            "nonexistent-hook",
            "--event",
            "pre_tool_use",
        ])
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec hook test");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("nonexistent-hook"),
        "error should mention the missing hook ID: {stderr}"
    );
}

#[test]
fn test_hook_test_multi_event_no_flag_errors() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let hooks_dir = dir.join("spec/hooks");
    let scripts_dir = hooks_dir.join("scripts");
    let r = std::fs::create_dir_all(&scripts_dir);
    assert!(r.is_ok(), "create hooks dir: {r:?}");
    let r = std::fs::write(
        hooks_dir.join("hooks.toml"),
        r#"
[hooks.multi-event]
events = ["pre_tool_use", "post_tool_use"]
script = "scripts/multi.sh"
"#,
    );
    assert!(r.is_ok(), "write hooks.toml: {r:?}");
    let r = std::fs::write(scripts_dir.join("multi.sh"), "#!/bin/sh\nexit 0\n");
    assert!(r.is_ok(), "write multi.sh: {r:?}");
    set_script_permissions(&scripts_dir);

    let output = std::process::Command::new(agentspec())
        .args(["hook", "test", "multi-event"])
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec hook test");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("multiple events"),
        "error should mention multiple events: {stderr}"
    );
    assert!(
        stderr.contains("pre_tool_use"),
        "error should list available events: {stderr}"
    );
}

#[test]
fn test_hook_test_single_event_no_flag_uses_it() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);

    let output = std::process::Command::new(agentspec())
        .args(["hook", "test", "audit-bash"])
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec hook test");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "single-event hook should auto-select: {stderr}"
    );
    assert!(
        stderr.contains("pre_tool_use"),
        "should use the only event: {stderr}"
    );
}

#[test]
fn test_hook_test_opencode_provider_errors() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);

    let output = std::process::Command::new(agentspec())
        .args([
            "hook",
            "test",
            "audit-bash",
            "--event",
            "pre_tool_use",
            "--provider",
            "opencode",
        ])
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec hook test");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid value") || stderr.contains("opencode"),
        "should reject opencode provider: {stderr}"
    );
}

#[test]
fn test_hook_test_payload_inline() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);

    let custom_payload = r#"{"session_id":"custom-sess","cwd":"/tmp","tool_name":"Bash","tool_input":{"command":"echo hello"}}"#;
    let output = std::process::Command::new(agentspec())
        .args([
            "hook",
            "test",
            "audit-bash",
            "--event",
            "pre_tool_use",
            "--payload",
            custom_payload,
        ])
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec hook test");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "hook test with payload failed:\n{stderr}"
    );
    assert!(
        stderr.contains("custom-sess"),
        "output should contain the custom payload: {stderr}"
    );
}

#[test]
fn test_hook_test_payload_file() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);

    let payload_path = tmp.path().join("payload.json");
    let r = std::fs::write(
        &payload_path,
        r#"{"session_id":"file-sess","cwd":"/tmp","tool_name":"Bash","tool_input":{"command":"ls"}}"#,
    );
    assert!(r.is_ok(), "write payload file: {r:?}");

    let output = std::process::Command::new(agentspec())
        .args([
            "hook",
            "test",
            "audit-bash",
            "--event",
            "pre_tool_use",
            "--payload-file",
            &payload_path.to_string_lossy(),
        ])
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec hook test");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "hook test with payload-file failed:\n{stderr}"
    );
    assert!(
        stderr.contains("file-sess"),
        "output should contain the file payload: {stderr}"
    );
}

#[test]
fn test_compile_resolves_extra_include_dir() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let extra_dir = dir.join("shared-fragments");
    std::fs::create_dir_all(&extra_dir).expect("create extra dir");
    std::fs::write(
        extra_dir.join("extra-note.md"),
        "Content from extra fragment.",
    )
    .expect("write extra fragment");

    let toml_path = dir.join("agentspec.toml");
    let toml_content = std::fs::read_to_string(&toml_path).expect("read toml");
    let patched = toml_content.replace(
        "[spec]\nsources_dir = \"spec\"",
        "[spec]\nsources_dir = \"spec\"\nextra_include_dirs = [{ name = \"shared\", path = \"shared-fragments\" }]",
    );
    std::fs::write(&toml_path, &patched).expect("write patched toml");

    let skill_dir = dir.join("spec/skills/extra-user");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nid: extra-user\ndescription: Uses extra fragment\nuser_invocable: true\nagent_invocable: false\n---\n\n{% include \"shared/extra-note.md\" %}\n",
    )
    .expect("write skill spec");

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    let compiled = std::fs::read_to_string(dir.join("generated/claude/skills/extra-user/SKILL.md"))
        .expect("read compiled skill");
    assert!(
        compiled.contains("Content from extra fragment."),
        "extra fragment not resolved in compiled output: {compiled}"
    );
}

#[test]
fn test_compile_extra_include_dir_duplicate_names_errors() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let extra_a = dir.join("extra-a");
    let extra_b = dir.join("extra-b");
    std::fs::create_dir_all(&extra_a).expect("create extra-a dir");
    std::fs::create_dir_all(&extra_b).expect("create extra-b dir");

    let toml_path = dir.join("agentspec.toml");
    let toml_content = std::fs::read_to_string(&toml_path).expect("read toml");
    let patched = toml_content.replace(
        "[spec]\nsources_dir = \"spec\"",
        "[spec]\nsources_dir = \"spec\"\nextra_include_dirs = [\n  { name = \"shared\", path = \"extra-a\" },\n  { name = \"shared\", path = \"extra-b\" },\n]",
    );
    std::fs::write(&toml_path, &patched).expect("write patched toml");

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    assert!(
        !output.status.success(),
        "compile should fail on duplicate names"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("duplicate"),
        "stderr should mention duplicate: {stderr}"
    );
}

#[test]
fn test_compile_resolves_colocated_include() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let skill_dir = dir.join("spec/skills/colocated-test");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nid: colocated-test\ndescription: Colocated include test\nuser_invocable: true\nagent_invocable: false\n---\n\n{% include \"./detail.md\" %}\n",
    )
    .expect("write skill spec");
    std::fs::write(skill_dir.join("detail.md"), "Colocated detail content.").expect("write detail");

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    let compiled =
        std::fs::read_to_string(dir.join("generated/claude/skills/colocated-test/SKILL.md"))
            .expect("read compiled skill");
    assert!(
        compiled.contains("Colocated detail content."),
        "colocated content not resolved: {compiled}"
    );
}

#[test]
fn test_compile_resolves_colocated_include_full_path() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let skill_dir = dir.join("spec/skills/colocated-test");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nid: colocated-test\ndescription: Full-path colocated include\nuser_invocable: true\nagent_invocable: false\n---\n\n{% include \"skills/colocated-test/detail.md\" %}\n",
    )
    .expect("write skill spec");
    std::fs::write(skill_dir.join("detail.md"), "Full path detail content.").expect("write detail");

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    let compiled =
        std::fs::read_to_string(dir.join("generated/claude/skills/colocated-test/SKILL.md"))
            .expect("read compiled skill");
    assert!(
        compiled.contains("Full path detail content."),
        "full-path colocated content not resolved: {compiled}"
    );
}

#[test]
fn test_compile_extra_include_dir_name_collision_with_spec_tree() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let extra = dir.join("extra-skills");
    std::fs::create_dir_all(&extra).expect("create extra dir");

    let toml_path = dir.join("agentspec.toml");
    let toml_content = std::fs::read_to_string(&toml_path).expect("read toml");
    let patched = toml_content.replace(
        "[spec]\nsources_dir = \"spec\"",
        "[spec]\nsources_dir = \"spec\"\nextra_include_dirs = [{ name = \"skills\", path = \"extra-skills\" }]",
    );
    std::fs::write(&toml_path, &patched).expect("write patched toml");

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    assert!(
        !output.status.success(),
        "compile should fail on name collision with spec tree"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("collides"),
        "stderr should mention collision: {stderr}"
    );
}

#[test]
fn test_config_flag_loads_specified_file() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let toml_path = dir.join("agentspec.toml");

    let output = std::process::Command::new(agentspec())
        .arg("--config")
        .arg(&toml_path)
        .arg("validate")
        .current_dir(tmp.path())
        .output()
        .expect("failed to run agentspec --config validate");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "--config validate failed:\n{stderr}"
    );
    assert!(stderr.contains("validation complete"), "stderr: {stderr}");
}

#[test]
fn test_config_flag_missing_file_errors() {
    let output = std::process::Command::new(agentspec())
        .arg("--config")
        .arg("/tmp/nonexistent-agentspec.toml")
        .arg("validate")
        .output()
        .expect("failed to run agentspec --config validate");

    assert!(
        !output.status.success(),
        "--config with missing file should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to read"),
        "stderr should mention failed read: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// `agentspec prune` tests
// ---------------------------------------------------------------------------

#[test]
fn test_prune_help_lists_subcommand() {
    let output = std::process::Command::new(agentspec())
        .args(["prune", "--help"])
        .output()
        .expect("failed to run agentspec prune --help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("--dry-run"));
    assert!(stdout.contains("--provider"));
    assert!(stdout.contains("--verbose"));
}

#[test]
fn test_prune_with_no_orphaned_entries_reports_nothing() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = tmp.path();
    std::fs::write(
        dir.join("agentspec.toml"),
        "[spec]\nsources_dir = \"spec\"\n",
    )
    .expect("write config");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("mkdir home");

    let output = run_agentspec(&["prune"], dir, &home).expect("agentspec spawn");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "prune should exit 0:\n{stderr}");
    assert!(
        stderr.contains("nothing to prune"),
        "expected 'nothing to prune' in stderr, got:\n{stderr}"
    );
}

#[test]
fn test_prune_strips_orphaned_agentspec_entries_from_settings_json() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = tmp.path();
    std::fs::write(
        dir.join("agentspec.toml"),
        "[spec]\nsources_dir = \"spec\"\n",
    )
    .expect("write config");
    let home = dir.join("home");
    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).expect("mkdir .claude");

    // Plant a settings.json with orphaned agentspec entries
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          { "type": "command", "command": "echo orphan", "_agentspec_id": "orphan-hook" }
        ]
      }
    ]
  }
}"#,
    )
    .expect("write settings.json");

    let output =
        run_agentspec(&["prune", "--provider", "claude"], dir, &home).expect("agentspec spawn");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "prune should exit 0:\n{stderr}");

    // The file had only agentspec entries — after prune, the delete-on-empty
    // behavior removes the now-empty file entirely.
    assert!(
        !claude_dir.join("settings.json").exists(),
        "settings.json should be deleted after pruning all entries"
    );
}

#[test]
fn test_prune_dry_run_does_not_modify_files() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = tmp.path();
    std::fs::write(
        dir.join("agentspec.toml"),
        "[spec]\nsources_dir = \"spec\"\n",
    )
    .expect("write config");
    let home = dir.join("home");
    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).expect("mkdir .claude");

    let original = r#"{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          { "type": "command", "command": "echo orphan", "_agentspec_id": "orphan-hook" }
        ]
      }
    ]
  }
}"#;
    std::fs::write(claude_dir.join("settings.json"), original).expect("write settings.json");

    let output = run_agentspec(&["prune", "--dry-run"], dir, &home).expect("agentspec spawn");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "prune --dry-run should exit 0:\n{stderr}"
    );

    let after = std::fs::read_to_string(claude_dir.join("settings.json"))
        .expect("read settings.json after dry-run");
    assert_eq!(after, original, "dry-run should not modify the file");
}

#[test]
fn test_prune_with_provider_flag_only_checks_that_provider() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = tmp.path();
    std::fs::write(
        dir.join("agentspec.toml"),
        "[spec]\nsources_dir = \"spec\"\n",
    )
    .expect("write config");
    let home = dir.join("home");

    // Create orphaned entries for both Claude and Cursor
    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).expect("mkdir .claude");
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{ "hooks": { "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "echo", "_agentspec_id": "a" }] }] } }"#,
    )
    .expect("write claude settings");

    let cursor_dir = home.join(".cursor");
    std::fs::create_dir_all(&cursor_dir).expect("mkdir .cursor");
    std::fs::write(
        cursor_dir.join("hooks.json"),
        r#"{ "hooks": { "PreToolUse": [{ "type": "command", "command": "echo", "_agentspec_id": "b" }] } }"#,
    )
    .expect("write cursor hooks");

    // Prune only cursor
    let output =
        run_agentspec(&["prune", "--provider", "cursor"], dir, &home).expect("agentspec spawn");
    assert!(output.status.success());

    // Claude should be untouched
    let claude_content =
        std::fs::read_to_string(claude_dir.join("settings.json")).expect("read claude settings");
    assert!(
        claude_content.contains("_agentspec_id"),
        "claude settings should be untouched"
    );

    // Cursor should be pruned — file only had agentspec entries, so
    // delete-on-empty removes it entirely
    assert!(
        !cursor_dir.join("hooks.json").exists(),
        "cursor hooks.json should be deleted after pruning all entries"
    );
}

/// Overwrite the per-test copy's `agentspec.toml`. The shared fixture must stay
/// valid for every other test, so preset-shape scenarios are installed into the
/// `TempDir` after `setup()` rather than committed — the same reasoning as
/// `install_hook_fixture`.
fn install_agentspec_toml(dir: &Path, body: &str) {
    let r = std::fs::write(dir.join("agentspec.toml"), body);
    assert!(r.is_ok(), "write agentspec.toml: {r:?}");
}

/// Point the fixture's agent at a preset by name.
fn install_agent_using_preset(dir: &Path, preset_name: &str) {
    let r = std::fs::write(
        dir.join("spec/agents/test-agent.md"),
        format!(
            "---\n\
             id: test-agent\n\
             description: A test agent for fixture testing\n\
             execution:\n  preset: {preset_name}\n\
             ---\n\n\
             # Test Agent\n\n\
             Agent instructions here.\n"
        ),
    );
    assert!(r.is_ok(), "write test-agent.md: {r:?}");
}

/// The bracket ban has to reach `compile`, not just `validate`. The precedent it
/// deliberately departs from — `SyncTargetConfig::validate_for_provider` — is
/// only wired into the `validate` command, so a preset gate copied literally
/// from it would let `compile` compose a double bracket unchecked.
#[test]
fn test_compile_rejects_bracketed_cursor_model() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    install_agentspec_toml(
        &dir,
        r#"
[spec]
sources_dir = "spec"

[compile]
output_dir = "generated"

[presets.default]
cursor = { model = "claude-opus-5[effort=high]" }
"#,
    );

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "compile should have failed:\n{stderr}"
    );
    assert!(
        stderr.contains("presets.default.cursor"),
        "stderr should name the offending preset block: {stderr}"
    );
    assert!(
        stderr.contains("bare model id"),
        "stderr should explain the constraint: {stderr}"
    );
}

#[test]
fn test_compile_composes_cursor_model_options() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    install_agentspec_toml(
        &dir,
        r#"
[spec]
sources_dir = "spec"

[compile]
output_dir = "generated"

[presets.default]
cursor = { model = "claude-opus-5", effort = "high", fast = false, context = "300k", params = { optimize_for = "cost" } }
"#,
    );
    install_agent_using_preset(&dir, "default");

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    let content = std::fs::read_to_string(dir.join("generated/cursor/agents/test-agent.md"))
        .expect("failed to read cursor agent");
    assert!(
        content.contains(
            "model: claude-opus-5[effort=high,fast=false,context=300k,optimize_for=cost]"
        ),
        "composed model line missing from:\n{content}"
    );
}

/// The gate's justification is that it fires on every command that loads specs,
/// not only `compile`. `validate` is the command a user reaches for first.
#[test]
fn test_validate_rejects_bracketed_cursor_model() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    install_agentspec_toml(
        &dir,
        r#"
[spec]
sources_dir = "spec"

[compile]
output_dir = "generated"

[presets.default]
cursor = { model = "claude-opus-5[effort=high]" }
"#,
    );

    let output = std::process::Command::new(agentspec())
        .arg("validate")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec validate");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "validate should have failed:\n{stderr}"
    );
    assert!(
        stderr.contains("presets.default.cursor"),
        "stderr should name the offending preset block: {stderr}"
    );
}

/// Claude's half is pinned byte-exactly in `adapters/claude.rs`, but the
/// `effort:` key reaching a real generated file through the binary was not
/// exercised — unlike Cursor's, which has three end-to-end cases.
#[test]
fn test_compile_emits_claude_effort() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    install_agentspec_toml(
        &dir,
        r#"
[spec]
sources_dir = "spec"

[compile]
output_dir = "generated"

[presets.default]
claude = { model = "opus", effort = "high" }
"#,
    );
    install_agent_using_preset(&dir, "default");

    let output = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "compile failed:\n{stderr}");

    let content = std::fs::read_to_string(dir.join("generated/claude/agents/test-agent.md"))
        .expect("failed to read claude agent");
    // Whole lines, not substrings: `contains("model: opus")` would also pass
    // for `model: opus-4.5`, and an `effort:` emitted anywhere in the file.
    assert!(
        content.contains("\nmodel: opus\n"),
        "expected an exact `model: opus` line in:\n{content}"
    );
    assert!(
        content.contains("\neffort: high\n"),
        "expected an exact `effort: high` line in:\n{content}"
    );
}
