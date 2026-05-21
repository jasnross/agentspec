use std::io::{IsTerminal, Write};
use std::process::{Command, Stdio};

use agentspec::hooks_canonical::shim_template::shim_script;
use agentspec::hooks_canonical::{CanonicalInput, ProviderName, provider_fixture};
use agentspec::spec::{HookEvent, Spec};
use agentspec::specs::{SpecDirs, ValidatedSpecs};
use anyhow::{Context, Result, bail};

use crate::cli::HookTestArgs;

pub fn run_hook_test(
    args: &HookTestArgs,
    dirs: &SpecDirs,
    validated: &ValidatedSpecs,
) -> Result<()> {
    let hook = find_hook(validated, &args.hook_id)?;
    let event = resolve_event(hook, args)?;
    let provider = args.provider;
    let payload = resolve_payload(args, provider, event)?;

    let shim_content = shim_script(provider, event);
    let shim_path = write_temp_executable(&shim_content, "shim.sh")?;

    let script_rel = &hook.frontmatter.script;
    let script_path = dirs.hooks.join(script_rel);
    if !script_path.exists() {
        bail!(
            "hook script not found: {} (resolved to {})",
            script_rel.display(),
            script_path.display(),
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(&script_path)
            .with_context(|| format!("reading metadata for {}", script_path.display()))?;
        if meta.permissions().mode() & 0o111 == 0 {
            bail!(
                "hook script is not executable: {} (run `chmod +x {}`)",
                script_path.display(),
                script_path.display(),
            );
        }
    }

    eprintln!(
        "\n\u{2500}\u{2500} Provider Input ({}, {}) \u{2500}\u{2500}",
        provider.wire_name(),
        event.snake_case()
    );
    eprintln!("{payload}");

    if let Ok(canonical) = CanonicalInput::from_provider_stdin(provider, &payload, event) {
        let pretty = serde_json::to_string_pretty(&canonical).unwrap_or_else(|_| payload.clone());
        eprintln!(
            "\n\u{2500}\u{2500} Canonical Input (what your script receives on stdin) \u{2500}\u{2500}"
        );
        eprintln!("{pretty}");
    }

    let mut child = Command::new("sh")
        .arg(shim_path.path())
        .arg(&script_path)
        .arg(&args.hook_id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn shim subprocess")?;

    {
        let mut child_stdin = child.stdin.take().context("failed to open shim stdin")?;
        child_stdin
            .write_all(payload.as_bytes())
            .context("failed to write payload to shim stdin")?;
    }

    let output = child
        .wait_with_output()
        .context("failed to wait for shim")?;
    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    eprintln!("\n\u{2500}\u{2500} Script Output \u{2500}\u{2500}");
    eprintln!(
        "stdout: {}",
        if stdout.is_empty() {
            "(empty)"
        } else {
            stdout.trim()
        }
    );
    if stderr.is_empty() {
        eprintln!("stderr: (empty)");
    } else {
        eprintln!("stderr: {}", stderr.trim());
    }

    if !stdout.is_empty() {
        eprintln!(
            "\n\u{2500}\u{2500} Provider Output (what the provider would receive) \u{2500}\u{2500}"
        );
        eprintln!("{}", stdout.trim());
    }

    eprintln!("\n\u{2500}\u{2500} Exit Code: {exit_code} \u{2500}\u{2500}");

    if exit_code != 0 {
        bail!("hook exited with code {exit_code}");
    }

    Ok(())
}

fn find_hook<'a>(
    validated: &'a ValidatedSpecs,
    hook_id: &str,
) -> Result<&'a agentspec::spec::HookSpec> {
    let mut available_ids: Vec<&str> = Vec::new();
    for spec in validated.specs() {
        if let Spec::Hook(h) = spec {
            if h.frontmatter.id == hook_id {
                return Ok(h);
            }
            available_ids.push(&h.frontmatter.id);
        }
    }
    if available_ids.is_empty() {
        bail!("no hooks found in spec/hooks/hooks.toml");
    }
    bail!(
        "hook '{hook_id}' not found. Available hooks: {}",
        available_ids.join(", "),
    );
}

fn resolve_event(hook: &agentspec::spec::HookSpec, args: &HookTestArgs) -> Result<HookEvent> {
    if let Some(event) = args.event {
        if !hook.frontmatter.events.contains(&event) {
            bail!(
                "hook '{}' is not registered for event '{}'. Registered events: {}",
                hook.frontmatter.id,
                event.snake_case(),
                hook.frontmatter
                    .events
                    .iter()
                    .map(|e| e.snake_case())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        return Ok(event);
    }

    match hook.frontmatter.events.as_slice() {
        [single] => Ok(*single),
        events => {
            debug_assert!(
                !events.is_empty(),
                "empty events should be caught at load time"
            );
            bail!(
                "hook '{}' handles multiple events; specify one with --event. Available: {}",
                hook.frontmatter.id,
                events
                    .iter()
                    .map(|e| e.snake_case())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
    }
}

fn resolve_payload(
    args: &HookTestArgs,
    provider: ProviderName,
    event: HookEvent,
) -> Result<String> {
    if let Some(path) = &args.payload_file {
        return std::fs::read_to_string(path)
            .with_context(|| format!("reading payload file: {}", path.display()));
    }

    if let Some(inline) = &args.payload {
        return Ok(inline.clone());
    }

    if !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
            .context("reading payload from stdin")?;
        if !buf.is_empty() {
            return Ok(buf);
        }
    }

    eprintln!(
        "note: no payload provided; using built-in fixture for {}/{}",
        provider.wire_name(),
        event.snake_case(),
    );
    Ok(provider_fixture(provider, event).to_string())
}

fn write_temp_executable(content: &str, suffix: &str) -> Result<tempfile::NamedTempFile> {
    let mut tmp = tempfile::Builder::new()
        .prefix("agentspec-")
        .suffix(suffix)
        .tempfile()
        .context("creating temp file for shim")?;
    tmp.write_all(content.as_bytes())
        .context("writing shim to temp file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = tmp.as_file().metadata()?.permissions();
        perms.set_mode(0o755);
        tmp.as_file().set_permissions(perms)?;
    }
    Ok(tmp)
}
