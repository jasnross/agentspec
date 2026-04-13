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
