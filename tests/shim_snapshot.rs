//! Snapshot tests for the shim code-generator.
//!
//! For every `(ProviderName, HookEvent)` pair, assert that
//! `shim_script(provider, event)` produces byte-identical output to a
//! committed golden file under `tests/snapshots/shim/<provider>_<event>.sh`.
//! Snapshots make codegen drift visible in PR diffs — if the generated
//! shim changes (intentionally or otherwise), the diff against the golden
//! files surfaces the change for review.
//!
//! ## Updating snapshots
//!
//! Set `AGENTSPEC_UPDATE_SHIM_SNAPSHOTS=1` and re-run:
//!
//! ```sh
//! AGENTSPEC_UPDATE_SHIM_SNAPSHOTS=1 cargo test --test shim_snapshot
//! ```
//!
//! The test will write the current generated output to the golden files
//! instead of comparing. Commit the resulting changes after reviewing the
//! diff.

use agentspec::hooks_canonical::ProviderName;
use agentspec::hooks_canonical::shim_template::shim_script;
use agentspec::spec::HookEvent;

fn snapshot_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join("shim")
}

fn snapshot_path(provider: ProviderName, event: HookEvent) -> std::path::PathBuf {
    snapshot_dir().join(format!(
        "{}_{}.sh",
        provider.wire_name(),
        event.snake_case()
    ))
}

fn all_pairs() -> Vec<(ProviderName, HookEvent)> {
    let events = [
        HookEvent::PreToolUse,
        HookEvent::PostToolUse,
        HookEvent::PostToolUseFailure,
        HookEvent::SessionStart,
        HookEvent::SessionEnd,
        HookEvent::Stop,
        HookEvent::PreCompact,
        HookEvent::SubagentStart,
        HookEvent::SubagentStop,
        HookEvent::UserPromptSubmit,
    ];
    let mut v = Vec::with_capacity(20);
    for p in [ProviderName::Claude, ProviderName::Cursor] {
        for e in events {
            v.push((p, e));
        }
    }
    v
}

fn update_mode() -> bool {
    std::env::var("AGENTSPEC_UPDATE_SHIM_SNAPSHOTS")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

#[test]
fn all_twenty_shims_match_committed_golden_output() {
    std::fs::create_dir_all(snapshot_dir()).expect("create snapshot dir");
    let update = update_mode();
    let mut failures: Vec<String> = Vec::new();
    for (provider, event) in all_pairs() {
        let generated = shim_script(provider, event);
        let path = snapshot_path(provider, event);
        if update {
            std::fs::write(&path, &generated).expect("write snapshot");
            continue;
        }
        let expected = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!(
                    "missing snapshot file {}: {e} (re-run with AGENTSPEC_UPDATE_SHIM_SNAPSHOTS=1)",
                    path.display(),
                ));
                continue;
            }
        };
        if expected != generated {
            failures.push(format!(
                "shim drift for {}/{}: snapshot does not match generated output (re-run with AGENTSPEC_UPDATE_SHIM_SNAPSHOTS=1 after reviewing the change)",
                provider.wire_name(),
                event.snake_case(),
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} snapshot failure(s):\n  - {}",
        failures.len(),
        failures.join("\n  - "),
    );
}
