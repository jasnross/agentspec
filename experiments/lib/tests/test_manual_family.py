"""The manual family: refusals that cost no session, and a dry run that writes no record."""

import json
import os
import shutil
import subprocess
import time
from pathlib import Path
from typing import Any

from tests.conftest import EXPERIMENTS_DIR, Cli

DUMP_HOOK = EXPERIMENTS_DIR / "cursor-subagent-effort" / "fixtures" / "capture" / "dump-hook.sh"

MANUAL_PROBE_PY = """\
from probe_harness.families.manual import ManualProbe

PROBE = ManualProbe(instructions="Open the workspace and run the session.")
"""


def _put_manual(tree: Path, name: str, *, wait_for: str | None) -> Path:
    package = tree / name
    capture = package / "fixtures" / "capture"
    capture.mkdir(parents=True)
    shutil.copy2(DUMP_HOOK, capture / "dump-hook.sh")
    manifest: dict[str, Any] = {
        "schema_version": 1,
        "provider": "cursor",
        "driver": "manual",
        "depth": None,
        "question": "q",
        "version_source": {"kind": "none"},
        "assertion": {"projection": ".[0].x", "expected": 1},
    }
    if wait_for is not None:
        manifest["wait_for"] = wait_for
    (package / "probe.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    (package / "probe.py").write_text(MANUAL_PROBE_PY, encoding="utf-8")
    return package


def _workspaces(name: str) -> list[Path]:
    return sorted(Path(os.environ["TMPDIR"]).glob(f"probe-{name}.*"))


def test_a_manual_manifest_without_wait_for_fails_before_the_workspace_is_ready(
    cli: Cli, tree: Path
) -> None:
    _put_manual(tree, "manual-stub", wait_for=None)

    result = cli("run", "manual-stub")

    assert result.returncode == 1, result.output
    assert "manifest declares no wait_for filter" in result.output
    assert "Workspace ready" not in result.output
    assert _workspaces("manual-stub") == []


def test_a_dry_run_with_a_matching_capture_writes_no_record(tree: Path) -> None:
    package = _put_manual(tree, "manual-stub", wait_for="length > 0")

    process = subprocess.Popen(
        [str(EXPERIMENTS_DIR / "probe"), "run", "manual-stub"],
        env={**os.environ, "PROBE_DRY_RUN": "1", "PROBE_TIMEOUT_SECONDS": "60"},
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
    )
    try:
        # Seed the capture as the provider's hook would, once Arrange has created it. The
        # payload lands whole via a rename, so the poll never reads a partial line.
        capture = _await_capture_dir("manual-stub", deadline=time.monotonic() + 30)
        staged = capture / "payloads.jsonl.tmp"
        staged.write_text('{"x":1}\n', encoding="utf-8")
        staged.replace(capture / "payloads.jsonl")

        stdout, stderr = process.communicate(timeout=60)
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate()

    assert process.returncode == 0, stdout + stderr
    assert "Workspace ready" in stderr
    assert "record: dry run" in stderr
    assert json.loads(stdout)["status"] == "confirmed"
    results = package / "results"
    assert not results.exists() or not any(results.iterdir())


def _await_capture_dir(name: str, *, deadline: float) -> Path:
    while time.monotonic() < deadline:
        for ws in _workspaces(name):
            # The capture script is templated in after `capture/` is created; once it is
            # present, Arrange has finished with the directory.
            if (ws / "capture" / "dump-hook.sh").is_file():
                return ws / "capture"
        time.sleep(0.1)
    raise AssertionError(f"no workspace with a capture directory appeared for {name}")
