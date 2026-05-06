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

    // Cursor: skills and agents
    assert!(
        dir.join("generated/cursor/skills/basic-skill/SKILL.md")
            .exists(),
        "missing cursor basic-skill"
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
fn test_sync_prefix() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    std::fs::write(
        dir.join("agentspec.toml"),
        r#"
[presets.default]
claude = { model = "sonnet" }
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
cursor = { model = "fast" }

[sync.claude]
mode = "user"
prefix = "tw"
"#,
    )
    .expect("failed to write agentspec.toml");

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

    std::fs::write(
        dir.join("agentspec.toml"),
        r#"
[presets.default]
claude = { model = "sonnet" }
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
cursor = { model = "fast" }

[sync.opencode]
mode = "user"
prefix = "tw"
"#,
    )
    .expect("failed to write agentspec.toml");

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

    std::fs::write(
        dir.join("agentspec.toml"),
        r#"
[presets.default]
claude = { model = "sonnet" }
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
cursor = { model = "fast" }

[sync.cursor]
mode = "user"
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
fn test_sync_provider_unconfigured_with_dest_allowed() {
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

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sync should succeed:\n{stderr}");
    // --dest should route files to the specified directory.
    assert!(
        dest.join("agents").exists(),
        "agents dir should be created under --dest"
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
    std::fs::write(
        dir.join("agentspec.toml"),
        r#"
[presets.default]
claude = { model = "sonnet" }
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
cursor = { model = "fast" }

[sync.claude]
mode = "user"
prefix = "original"
"#,
    )
    .expect("failed to write agentspec.toml");

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
    std::fs::write(
        dir.join("agentspec.toml"),
        r#"
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
    std::fs::write(
        dir.join("agentspec.toml"),
        r#"
[presets.default]
claude = { model = "sonnet" }
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
cursor = { model = "fast" }

[sync.claude]
mode = "user"
prefix = "tw"
"#,
    )
    .expect("failed to write agentspec.toml");

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
    std::fs::write(
        dir.join("agentspec.toml"),
        r#"
[presets.default]
claude = { model = "sonnet" }
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
cursor = { model = "fast" }

[sync.opencode]
mode = "user"
prefix = "tw"
"#,
    )
    .expect("failed to write agentspec.toml");

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
    std::fs::write(
        dir.join("agentspec.toml"),
        r#"
[presets.default]
claude = { model = "sonnet" }
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
cursor = { model = "fast" }

[sync.claude]
mode = "user"
content-prefix = "tw:"
"#,
    )
    .expect("failed to write agentspec.toml");

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
    std::fs::write(
        dir.join("agentspec.toml"),
        r#"
[presets.default]
claude = { model = "sonnet" }
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
cursor = { model = "fast" }

[sync.claude]
mode = "user"
prefix = "tw"
content-prefix = "tw:"
"#,
    )
    .expect("failed to write agentspec.toml");

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
    std::fs::write(
        dir.join("agentspec.toml"),
        r#"
[presets.default]
claude = { model = "sonnet" }
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
cursor = { model = "fast" }

[sync.claude]
mode = "user"
content-prefix = "original:"
"#,
    )
    .expect("failed to write agentspec.toml");

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
    // write_ignore_config omits sync config — sync --dry-run requires one.
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
event = "user_prompt_submit"
script = "scripts/init-thoughts.sh"
description = "Seed THOUGHTS_DIR context at the start of each turn"

[hooks.audit-bash]
event = "pre_tool_use"
matcher = "Bash"
script = "scripts/audit-bash.sh"
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

    // Both Claude and Cursor receive the same script content (Cursor aliases
    // ${CLAUDE_PLUGIN_ROOT} natively at plugin scope).
    for provider in ["claude", "cursor"] {
        let json = dir.join(format!("generated/{provider}/hooks/hooks.json"));
        assert!(json.exists(), "{provider}: hooks.json should be emitted");
        let content = std::fs::read_to_string(&json).expect("failed to read hooks.json");
        assert!(
            content.contains("init-thoughts"),
            "{provider}: hooks.json should contain init-thoughts agentspec_id, got:\n{content}"
        );
        assert!(
            content.contains("${CLAUDE_PLUGIN_ROOT}/hooks/scripts/init-thoughts.sh"),
            "{provider}: hooks.json should anchor scripts under CLAUDE_PLUGIN_ROOT"
        );

        // Both entry scripts AND the `_common.sh` helper that
        // `init-thoughts.sh` sources must land at the destination — otherwise
        // the entry script fails at runtime trying to source a missing file.
        for script in ["init-thoughts.sh", "audit-bash.sh", "_common.sh"] {
            let path = dir.join(format!("generated/{provider}/hooks/scripts/{script}"));
            assert!(path.exists(), "{provider}: {script} should be emitted");
        }
    }

    // OpenCode does not receive hook output in v1.
    assert!(
        !dir.join("generated/opencode/hooks").exists(),
        "OpenCode should not receive hooks/ directory in v1"
    );
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
event = "session_start"
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
    assert!(
        cursor_json.contains("\"matcher\": \"Bash\""),
        "Cursor should place matcher per-entry, got:\n{cursor_json}"
    );
}

#[test]
fn test_compile_opencode_with_hooks_prints_skip_warning() {
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
        stderr.contains("opencode: skipped"),
        "expected per-provider skip summary, got:\n{stderr}"
    );
}

#[test]
fn test_compile_opencode_verbose_lists_skipped_hooks() {
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
        stderr.contains("opencode: skipped hook init-thoughts"),
        "expected per-spec listing under --verbose, got:\n{stderr}"
    );
    assert!(
        stderr.contains("opencode: skipped hook audit-bash"),
        "expected per-spec listing under --verbose, got:\n{stderr}"
    );
}

#[test]
fn test_sync_claude_path_mode_writes_hooks() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    install_hook_fixture(&dir);
    let dest = dir.join("plugin-claude");
    std::fs::write(
        dir.join("agentspec.toml"),
        format!(
            r#"
[presets.default]
claude = {{ model = "sonnet" }}
opencode = {{ model = "anthropic/claude-sonnet-4-5", variant = "high" }}
cursor = {{ model = "fast" }}

[sync.claude]
mode = "path"
dir = "{}"
"#,
            dest.display()
        ),
    )
    .expect("failed to write agentspec.toml");

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
        "sync (path mode) should succeed:\n{stderr}"
    );

    assert!(
        dest.join("hooks/hooks.json").exists(),
        "hooks.json should land under the plugin path destination"
    );
    assert!(
        dest.join("hooks/scripts/init-thoughts.sh").exists(),
        "hook script should land under the plugin path destination"
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
    std::fs::write(
        dir.join("agentspec.toml"),
        r#"
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
    assert!(
        content.contains(
            "CLAUDE_PLUGIN_ROOT=$HOME/.claude $HOME/.claude/hooks/scripts/init-thoughts.sh"
        ),
        "command should set CLAUDE_PLUGIN_ROOT inline and anchor under $HOME for User mode, got:\n{content}"
    );
}
