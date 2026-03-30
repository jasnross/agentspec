//! Integration tests using a self-contained fixture under `tests/fixtures/agent-config/`.
//! These tests always run — no external dotfiles checkout required.

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
    assert!(stderr.contains("loaded 5 specs"), "stderr: {stderr}");
    assert!(
        stderr.contains("schema validation passed"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("semantic validation passed"),
        "stderr: {stderr}"
    );
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

    // Codex and Cursor: skills only — no agents dir
    assert!(
        dir.join("generated/codex/skills/basic-skill/SKILL.md")
            .exists(),
        "missing codex basic-skill"
    );
    assert!(
        dir.join("generated/cursor/skills/basic-skill/SKILL.md")
            .exists(),
        "missing cursor basic-skill"
    );
    assert!(
        !dir.join("generated/codex/agents").exists(),
        "codex should not have agents/"
    );
    assert!(
        !dir.join("generated/cursor/agents").exists(),
        "cursor should not have agents/"
    );

    // Fragment was resolved: basic-skill body should contain the included text
    let basic_skill_path = dir.join("generated/claude/skills/basic-skill/SKILL.md");
    let content = std::fs::read_to_string(&basic_skill_path).expect("failed to read basic-skill");
    assert!(
        content.contains("shared via fragment"),
        "fragment not resolved in basic-skill body"
    );

    // --- Rules ---

    // Claude: unconditional rule has no frontmatter; path-scoped rule has paths: in frontmatter
    let claude_general = dir.join("generated/claude/rules/general-guidance.md");
    assert!(
        claude_general.exists(),
        "missing claude general-guidance rule"
    );
    let claude_general_content =
        std::fs::read_to_string(&claude_general).expect("failed to read claude general rule");
    assert!(
        !claude_general_content.starts_with("---"),
        "unconditional claude rule should have no frontmatter"
    );

    let claude_api = dir.join("generated/claude/rules/api-design.md");
    assert!(claude_api.exists(), "missing claude api-design rule");
    let claude_api_content =
        std::fs::read_to_string(&claude_api).expect("failed to read claude api rule");
    assert!(
        claude_api_content.contains("paths:"),
        "path-scoped claude rule should have paths in frontmatter"
    );
    assert!(
        claude_api_content.contains("src/api/**"),
        "claude api rule should contain the glob pattern"
    );

    // Cursor: .mdc extension; alwaysApply vs globs
    let cursor_general = dir.join("generated/cursor/rules/general-guidance.mdc");
    assert!(
        cursor_general.exists(),
        "missing cursor general-guidance rule"
    );
    let cursor_general_content =
        std::fs::read_to_string(&cursor_general).expect("failed to read cursor general rule");
    assert!(
        cursor_general_content.contains("alwaysApply: true"),
        "unconditional cursor rule should have alwaysApply"
    );

    let cursor_api = dir.join("generated/cursor/rules/api-design.mdc");
    assert!(cursor_api.exists(), "missing cursor api-design rule");
    let cursor_api_content =
        std::fs::read_to_string(&cursor_api).expect("failed to read cursor api rule");
    assert!(
        cursor_api_content.contains("globs:"),
        "path-scoped cursor rule should have globs"
    );
    assert!(
        !cursor_api_content.contains("alwaysApply"),
        "path-scoped cursor rule should not have alwaysApply"
    );

    // Codex: plain body, no frontmatter
    let codex_general = dir.join("generated/codex/rules/general-guidance.md");
    assert!(
        codex_general.exists(),
        "missing codex general-guidance rule"
    );
    let codex_general_content =
        std::fs::read_to_string(&codex_general).expect("failed to read codex general rule");
    assert!(
        !codex_general_content.starts_with("---"),
        "codex rule should have no frontmatter"
    );

    let codex_api = dir.join("generated/codex/rules/api-design.md");
    assert!(codex_api.exists(), "missing codex api-design rule");
    let codex_api_content =
        std::fs::read_to_string(&codex_api).expect("failed to read codex api rule");
    assert!(
        !codex_api_content.starts_with("---"),
        "codex rule should have no frontmatter"
    );

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
fn test_check_passes_after_compile() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let compile = std::process::Command::new(agentspec())
        .arg("compile")
        .current_dir(&dir)
        .output()
        .expect("failed to run compile");
    assert!(
        compile.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let check = std::process::Command::new(agentspec())
        .arg("check")
        .current_dir(&dir)
        .output()
        .expect("failed to run check");
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(
        check.status.success(),
        "check failed after compile:\n{stderr}"
    );
    assert!(
        stderr.contains("check passed"),
        "expected 'check passed': {stderr}"
    );
}

#[test]
fn test_sync_prefix_strip_name_conflict_errors() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = tmp.path();
    std::fs::write(
        dir.join("agentspec.toml"),
        "[sync.claude]\nprefix = \"tw\"\nstrip_name = true\n",
    )
    .expect("failed to write agentspec.toml");

    let output = std::process::Command::new(agentspec())
        .args(["sync", "--no-compile", "--provider", "claude"])
        .current_dir(dir)
        .output()
        .expect("failed to run agentspec sync");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "sync should fail:\n{stderr}");
    assert!(
        stderr.contains("`prefix` and `strip_name` are mutually exclusive"),
        "stderr: {stderr}"
    );
}

#[test]
fn test_sync_prefix_symlink_conflict_errors() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = tmp.path();
    std::fs::write(
        dir.join("agentspec.toml"),
        "[sync.claude]\nprefix = \"tw\"\nstrategy = \"symlink\"\n",
    )
    .expect("failed to write agentspec.toml");

    let output = std::process::Command::new(agentspec())
        .args(["sync", "--no-compile", "--provider", "claude"])
        .current_dir(dir)
        .output()
        .expect("failed to run agentspec sync");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "sync should fail:\n{stderr}");
    assert!(
        stderr.contains("`prefix` requires `strategy = \"copy\"`"),
        "stderr: {stderr}"
    );
}

#[test]
fn test_sync_opencode_commands_prefix_subdir() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = tmp.path();
    std::fs::write(
        dir.join("agentspec.toml"),
        "[sync.opencode]\nmode = \"user\"\nprefix = \"tw\"\n",
    )
    .expect("failed to write agentspec.toml");
    std::fs::create_dir_all(dir.join("generated/opencode/commands"))
        .expect("failed to create generated commands dir");
    std::fs::write(
        dir.join("generated/opencode/commands/commit.md"),
        "---\nname: commit\n---\n",
    )
    .expect("failed to write generated command");

    let home = dir.join("home");
    let output = std::process::Command::new(agentspec())
        .args(["sync", "--no-compile", "--provider", "opencode"])
        .env("HOME", &home)
        .current_dir(dir)
        .output()
        .expect("failed to run agentspec sync");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sync failed:\n{stderr}");
    assert!(
        stderr.contains(".config/opencode/commands/tw"),
        "stderr: {stderr}"
    );
    assert!(
        home.join(".config/opencode/commands/tw/commit.md").exists(),
        "prefixed opencode command file should exist"
    );
}

#[test]
fn test_sync_no_config_errors_with_guidance() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let home = dir.join("home");

    let output = std::process::Command::new(agentspec())
        .args(["sync", "--no-compile"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --no-compile");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "sync should fail:\n{stderr}");
    assert!(
        stderr.contains("no sync providers are configured"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("--provider claude --mode user|project"),
        "stderr: {stderr}"
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
claude = "sonnet"
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
codex = { model = "gpt-4o" }
cursor = "fast"

[sync.cursor]
mode = "user"
strategy = "symlink"
"#,
    )
    .expect("failed to write agentspec.toml");

    let output = std::process::Command::new(agentspec())
        .args(["sync", "--no-compile"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --no-compile");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sync should succeed:\n{stderr}");
    assert!(stderr.contains("syncing cursor"), "stderr: {stderr}");
    assert!(
        !stderr.contains("syncing claude"),
        "claude should not be synced by default: {stderr}"
    );
    assert!(
        !stderr.contains("syncing opencode"),
        "opencode should not be synced by default: {stderr}"
    );
}

#[test]
fn test_sync_provider_unconfigured_errors_without_dest() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let home = dir.join("home");

    let output = std::process::Command::new(agentspec())
        .args(["sync", "--no-compile", "--provider", "claude"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --provider claude");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "sync should fail:\n{stderr}");
    assert!(
        stderr.contains("sync config for claude is not configured"),
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
            "--no-compile",
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
    assert!(
        stderr.contains("syncing claude (mode=Path"),
        "stderr: {stderr}"
    );
}

#[test]
fn test_sync_provider_unconfigured_with_mode_user_allowed() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let home = dir.join("home");

    let output = std::process::Command::new(agentspec())
        .args([
            "sync",
            "--no-compile",
            "--provider",
            "claude",
            "--mode",
            "user",
        ])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --provider claude --mode user");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sync should succeed:\n{stderr}");
    assert!(
        stderr.contains("syncing claude (mode=User"),
        "stderr: {stderr}"
    );
}

#[test]
fn test_sync_provider_unconfigured_with_mode_project_allowed() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let home = dir.join("home");

    let output = std::process::Command::new(agentspec())
        .args([
            "sync",
            "--no-compile",
            "--provider",
            "claude",
            "--mode",
            "project",
        ])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --provider claude --mode project");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sync should succeed:\n{stderr}");
    assert!(
        stderr.contains("syncing claude (mode=Project"),
        "stderr: {stderr}"
    );
}

#[test]
fn test_sync_no_config_mode_user_without_provider_errors() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);
    let home = dir.join("home");

    let output = std::process::Command::new(agentspec())
        .args(["sync", "--no-compile", "--mode", "user"])
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
    assert!(stderr.contains("explicit provider"), "stderr: {stderr}");
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
claude = "sonnet"
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
codex = { model = "gpt-4o" }
cursor = "fast"

[sync.cursor]
invalid_field = "oops"
"#,
    )
    .expect("failed to write agentspec.toml");

    let output = std::process::Command::new(agentspec())
        .args(["sync", "--no-compile"])
        .env("HOME", &home)
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec sync --no-compile");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "sync should fail:\n{stderr}");
    assert!(stderr.contains("failed to parse"), "stderr: {stderr}");
    assert!(stderr.contains("unknown field"), "stderr: {stderr}");
}
