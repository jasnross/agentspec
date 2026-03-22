//! Integration tests that run against the real dotfiles spec directory.
//! These tests are skipped if the spec directory doesn't exist.

use std::path::PathBuf;

// Re-export from the library — since agentspec is a binary crate, we test
// through the modules directly by including them. For integration tests
// against the real spec dir, we'll use a subprocess approach instead.

fn agent_config_dir() -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("agent-config");
    if path.is_dir() { Some(path) } else { None }
}

#[test]
fn test_load_specs_from_dotfiles() {
    let Some(base_dir) = agent_config_dir() else {
        eprintln!("skipping: agent-config directory not found");
        return;
    };

    // Use a subprocess to run the binary with a test flag, or directly test
    // by invoking the parse functions. Since this is a bin crate, we test
    // by running the binary and checking output. For now, we verify the
    // spec directory structure directly.
    let agents_dir = base_dir.join("spec/agents");
    let skills_dir = base_dir.join("spec/skills");

    // Count agent specs
    let agent_count = std::fs::read_dir(&agents_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
        .count();
    assert_eq!(
        agent_count, 8,
        "expected 8 agent specs (update if agents added/removed)"
    );

    // Count skill specs
    let skill_count = std::fs::read_dir(&skills_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .count();
    assert_eq!(
        skill_count, 27,
        "expected 27 skill specs (update if skills added/removed)"
    );

    // Verify gh-safe has supporting file in scripts/ subdirectory
    let gh_safe_script = skills_dir
        .join("gh-safe")
        .join("scripts")
        .join("gh-safe.sh");
    assert!(
        gh_safe_script.exists(),
        "expected gh-safe/scripts/gh-safe.sh to exist"
    );

    // Verify gh-safe.sh is executable
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(&gh_safe_script).unwrap();
    assert!(
        metadata.permissions().mode() & 0o111 != 0,
        "gh-safe.sh should be executable"
    );
}

#[test]
fn test_validate_against_dotfiles() {
    let Some(base_dir) = agent_config_dir() else {
        eprintln!("skipping: agent-config directory not found");
        return;
    };

    // Run `agentspec validate` against the real dotfiles spec directory.
    // This exercises schema validation, normalization, and semantic checks.
    let binary = env!("CARGO_BIN_EXE_agentspec");
    let output = std::process::Command::new(binary)
        .arg("validate")
        .current_dir(&base_dir)
        .output()
        .expect("failed to run agentspec");

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "agentspec validate failed against dotfiles:\n{}",
        stderr
    );

    // Verify key pipeline stages ran
    assert!(stderr.contains("loaded 35 specs"), "stderr: {}", stderr);
    assert!(
        stderr.contains("schema validation passed"),
        "stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("semantic validation passed"),
        "stderr: {}",
        stderr
    );
    // During migration, some fragments use Handlebars syntax and produce warnings
    assert!(stderr.contains("validation complete"), "stderr: {}", stderr);
}

#[test]
fn test_validate_strict_passes() {
    let Some(base_dir) = agent_config_dir() else {
        eprintln!("skipping: agent-config directory not found");
        return;
    };

    // All fragments use MiniJinja syntax, so --strict should pass with zero warnings.
    let binary = env!("CARGO_BIN_EXE_agentspec");
    let output = std::process::Command::new(binary)
        .args(["validate", "--strict"])
        .current_dir(&base_dir)
        .output()
        .expect("failed to run agentspec");

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "agentspec validate --strict failed:\n{}",
        stderr
    );
    assert!(stderr.contains("validation complete"), "stderr: {}", stderr);
}

#[test]
fn test_compile_against_dotfiles() {
    let Some(base_dir) = agent_config_dir() else {
        eprintln!("skipping: agent-config directory not found");
        return;
    };

    // Run `agentspec compile` against the real dotfiles spec directory.
    // This exercises the full pipeline through adapter dispatch.
    let binary = env!("CARGO_BIN_EXE_agentspec");
    let output = std::process::Command::new(binary)
        .arg("compile")
        .current_dir(&base_dir)
        .output()
        .expect("failed to run agentspec");

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "agentspec compile failed against dotfiles:\n{}",
        stderr
    );

    // Verify compilation ran
    assert!(stderr.contains("loaded 35 specs"), "stderr: {}", stderr);
    assert!(stderr.contains("compiled"), "stderr: {}", stderr);
    assert!(stderr.contains("4 provider(s)"), "stderr: {}", stderr);
    assert!(
        stderr.contains("wrote"),
        "expected wrote message: {}",
        stderr
    );
}

#[test]
fn test_check_passes_after_compile() {
    let Some(base_dir) = agent_config_dir() else {
        eprintln!("skipping: agent-config directory not found");
        return;
    };

    // Compile first to a temp copy, then check
    let tmp = tempfile::TempDir::new().unwrap();
    let test_dir = tmp.path().join("agent-config");
    // Copy the spec, mappings, and generated dirs
    let status = std::process::Command::new("cp")
        .args(["-r", base_dir.to_str().unwrap(), test_dir.to_str().unwrap()])
        .status()
        .expect("failed to copy");
    assert!(status.success(), "cp failed");

    let binary = env!("CARGO_BIN_EXE_agentspec");

    // Compile
    let compile_output = std::process::Command::new(binary)
        .arg("compile")
        .current_dir(&test_dir)
        .output()
        .expect("failed to run compile");
    assert!(
        compile_output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    // Check should pass immediately after compile
    let check_output = std::process::Command::new(binary)
        .arg("check")
        .current_dir(&test_dir)
        .output()
        .expect("failed to run check");
    let stderr = String::from_utf8_lossy(&check_output.stderr);
    assert!(
        check_output.status.success(),
        "check failed after compile:\n{}",
        stderr
    );
    assert!(
        stderr.contains("check passed"),
        "expected check passed: {}",
        stderr
    );
}
