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
fn test_sync_provider_unconfigured_with_dest_requires_plugin_name() {
    // Per the plan: `--dest <path>` implies `mode = "plugin"`. Plugin mode
    // requires `plugin-name`. Since v1 ships no `--plugin-name` CLI flag,
    // running `agentspec sync --provider X --dest <path>` without a TOML
    // `plugin-name` errors with the validation message.
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
        "sync --dest without plugin-name must error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("plugin-name"),
        "error should mention plugin-name, got:\n{stderr}"
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
            .with_plugin_author("Jason")],
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
    assert!(
        content.contains("CLAUDE_PLUGIN_ROOT=${CLAUDE_PROJECT_DIR}/.claude ${CLAUDE_PROJECT_DIR}/.claude/hooks/scripts/init-thoughts.sh"),
        "command should set CLAUDE_PLUGIN_ROOT inline and anchor under \\${{CLAUDE_PROJECT_DIR}} for Project mode, got:\n{content}"
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
    assert!(
        content.contains(
            "CLAUDE_PLUGIN_ROOT=$HOME/.claude $HOME/.claude/hooks/scripts/init-thoughts.sh"
        ),
        "command should set CLAUDE_PLUGIN_ROOT inline and anchor under $HOME for User mode, got:\n{content}"
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
    plugin_author: Option<&'a str>,
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

    fn with_plugin_author(mut self, author: &'a str) -> Self {
        self.plugin_author = Some(author);
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
        if let Some(author) = entry.plugin_author {
            let _ = writeln!(sections, "plugin-author = \"{author}\"");
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
