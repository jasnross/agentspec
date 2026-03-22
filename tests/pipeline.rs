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
        .args([
            "-r",
            fixture_dir().to_str().expect("fixture path is valid utf-8"),
            dest.to_str().expect("tmp path is valid utf-8"),
        ])
        .status()
        .expect("failed to copy fixture");
    assert!(status.success(), "cp fixture failed");
    set_script_permissions(&dest);
    dest
}

#[cfg(unix)]
fn set_script_permissions(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
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
    assert!(stderr.contains("loaded 3 specs"), "stderr: {stderr}");
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
fn test_validate_strict_no_warnings() {
    let output = std::process::Command::new(agentspec())
        .args(["validate", "--strict"])
        .current_dir(fixture_dir())
        .output()
        .expect("failed to run agentspec validate --strict");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "validate --strict failed:\n{stderr}"
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
fn test_compile_with_profile_overrides_model() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let dir = setup(&tmp);

    let output = std::process::Command::new(agentspec())
        .args(["compile", "--profile", "test"])
        .current_dir(&dir)
        .output()
        .expect("failed to run agentspec compile --profile test");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "compile --profile test failed:\n{stderr}"
    );

    // scripted-skill is user_invocable → OpenCode emits commands/<id>.md
    // With --profile test, the [profiles.test.default] overlay replaces the
    // OpenCode model from "anthropic/claude-sonnet-4-5" to "openai/gpt-4o".
    let cmd_path = dir.join("generated/opencode/commands/scripted-skill.md");
    assert!(cmd_path.exists(), "scripted-skill command not generated");

    let content = std::fs::read_to_string(&cmd_path).expect("failed to read command file");
    assert!(
        content.contains("openai/gpt-4o"),
        "profile model override not applied: {content}"
    );
    assert!(
        !content.contains("anthropic/claude-sonnet-4-5"),
        "base model should be replaced by profile: {content}"
    );
}
