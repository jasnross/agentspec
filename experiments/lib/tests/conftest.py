"""Shared fixtures for the probe harness suite.

Every test that runs the harness runs the real `experiments/probe` script as a subprocess,
so it exercises the version guard, the imports, and the exit codes an operator sees. Trees
a test builds live under its own temporary directory: a record the recorder writes is
byte-for-byte indistinguishable from a real measurement, so no test may write into a
tracked `results/` directory.
"""

import json
import os
import re
import shutil
import subprocess
import textwrap
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import pytest
from probe_harness.contract import Depth

EXPERIMENTS_DIR = Path(__file__).resolve().parents[2]


# Exactly these seven keys. No more, no fewer.
RECORD_KEYS = ["assertion", "date", "depth", "provider", "schema_version", "status", "tool_version"]


# Six fields removed during planning. A schema is only as narrow as what rejects additions,
# so each is asserted absent rather than merely unused.
RECORD_FORBIDDEN_KEYS = ["probe", "kind", "driver", "capture", "finding", "blocked_reason"]


# A synthetic package's `probe.py`: its Act prints and exits with the chosen code, so its
# Judge is never reached. Filled by `str.format` with `name` and `code`.
STUB_PROBE_PY = textwrap.dedent("""\
    import sys
    from pathlib import Path
    from probe_harness.families.base import CustomProbe, JudgeResult

    def _act(ws: Path) -> None:
        print("ran {name}")
        sys.exit({code})

    PROBE = CustomProbe(required_tools=(), arrange=lambda ws, variant: None, act=_act,
                        judge=lambda ws: JudgeResult(view=ws / "view.json", selection=None))
""")


# The manual-driver counterpart: a custom probe cannot run under a manifest that needs a
# capture, so a "manual" stub is a manual probe that prints and exits from its own preflight
# step, before any workspace or wait.
STUB_MANUAL_PROBE_PY = textwrap.dedent("""\
    import sys
    from probe_harness.families.base import ProbeFunctions
    from probe_harness.families.manual import ManualProbe

    def _preflight() -> None:
        print("ran {name}")
        sys.exit({code})

    PROBE = ManualProbe(instructions="", functions=ProbeFunctions(preflight=_preflight))
""")


@dataclass(frozen=True)
class Result:
    returncode: int
    stdout: str
    stderr: str

    @property
    def output(self) -> str:
        """stdout then stderr, for assertions that do not care which stream a line used."""
        return self.stdout + self.stderr


type Cli = Callable[..., Result]


@pytest.fixture(autouse=True)
def _isolated_tmpdir(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """Workspaces a test creates land under its own temporary directory."""
    tmpdir = tmp_path / "tmpdir"
    tmpdir.mkdir()
    monkeypatch.setenv("TMPDIR", str(tmpdir))
    # pytest runs under `uv run --locked`, so the harness environment is already synced and
    # the launcher may skip straight to the harness; the launcher's own tests unset this.
    monkeypatch.setenv("_AGENTSPEC_PROBE_SYNCED", "1")
    for name in (
        "PROBE_DRY_RUN",
        "PROBE_FIXTURE",
        "PROBE_TIMEOUT_SECONDS",
        "PROBE_AUTHORIZE_DRIVERS",
        "PROBE_EXPERIMENTS_DIR",
    ):
        monkeypatch.delenv(name, raising=False)
    return tmpdir


@pytest.fixture
def experiments_dir() -> Path:
    return EXPERIMENTS_DIR


@pytest.fixture
def cli() -> Cli:
    """Run `experiments/probe <args…>`, returning its exit code and both streams."""

    def run(
        *args: str | os.PathLike[str],
        env: Mapping[str, str] | None = None,
        cwd: Path | None = None,
        input: str | None = None,
        timeout: float = 120,
    ) -> Result:
        completed = subprocess.run(
            [str(EXPERIMENTS_DIR / "probe"), *map(str, args)],
            env={**os.environ, **(env or {})},
            cwd=cwd,
            input=input,
            stdin=None if input is not None else subprocess.DEVNULL,
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=timeout,
            check=False,
        )
        return Result(completed.returncode, completed.stdout, completed.stderr)

    return run


@pytest.fixture
def tree(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """An empty experiments tree that `probe` reads through `PROBE_EXPERIMENTS_DIR`."""
    root = tmp_path / "experiments"
    root.mkdir()
    monkeypatch.setenv("PROBE_EXPERIMENTS_DIR", str(root))
    return root


@pytest.fixture
def put_package(tree: Path) -> Callable[..., Path]:
    """A package declaring <driver>, whose stub `probe.py` prints and exits with <code>."""

    def put(name: str, driver: str, code: int = 0) -> Path:
        package = tree / name
        package.mkdir(parents=True, exist_ok=True)
        manifest = {
            "schema_version": 1,
            "provider": "opencode",
            "driver": driver,
            "depth": None,
            "question": "q",
            "version_source": {"kind": "none"},
            "assertion": {"projection": ".", "expected": 1},
        }
        (package / "probe.json").write_text(json.dumps(manifest, indent=2) + "\n")
        stub = STUB_MANUAL_PROBE_PY if driver == "manual" else STUB_PROBE_PY
        (package / "probe.py").write_text(stub.format(name=name, code=code))
        return package

    return put


def record_problems(path: Path) -> list[str]:
    """Every way the record at <path> breaks the record contract; empty when it holds."""
    try:
        record = json.loads(path.read_text(encoding="utf-8"))
    except ValueError:
        return [f"not valid JSON: {path}"]
    if not isinstance(record, dict):
        return [f"key set mismatch in {path}: not an object"]

    if sorted(record) != RECORD_KEYS:
        return [f"key set mismatch in {path}\n  want: {RECORD_KEYS}\n  got:  {sorted(record)}"]
    if any(key in record for key in RECORD_FORBIDDEN_KEYS):
        return [f"forbidden key present in {path}: {sorted(record)}"]
    if record["status"] not in ("confirmed", "refuted", "inconclusive"):
        return [f"status not one of confirmed/refuted/inconclusive in {path}"]
    # The recorder gates this at the manifest, but a record is a file on disk and this check
    # is what makes hand-editing one a detected defect.
    depth = record["depth"]
    if depth is not None and not (isinstance(depth, str) and depth in {d.value for d in Depth}):
        return [f"depth not one of resolved-config/outbound-request/null in {path}"]
    assertion = record["assertion"]
    if not (
        isinstance(assertion, dict)
        and "expected" in assertion
        and "observed" in assertion
        and ("projection" in assertion or "options" in assertion)
    ):
        return [f"assertion missing expected/observed/(projection|options) in {path}"]
    date = record["date"]
    if not (isinstance(date, str) and re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}", date)):
        return [f"date is not YYYY-MM-DD in {path}"]
    return []


@pytest.fixture
def record_wellformed() -> Callable[[Path], None]:
    """Assert one record file satisfies the record contract."""

    def check(path: Path) -> None:
        problems = record_problems(path)
        assert not problems, "\n".join(problems)

    return check


MARKER = "AGENTSPEC-PROBE-MARKER-TEST"


def write_view(path: Path, arms: dict[str, list[Any]]) -> Path:
    """An arm-keyed view: each arm maps to its array of request bodies."""
    path.write_text(json.dumps(arms, indent=2) + "\n", encoding="utf-8")
    return path


def deferred_request(
    *tools: str, marker: str = MARKER, model: str = "pinned-model"
) -> dict[str, Any]:
    """A subagent request under tool search.

    `ToolSearch` in `tools[]`, and each MCP tool named on its own line in the
    deferred-tools text block.
    """
    text = "\n".join(["The following deferred tools are now available via ToolSearch:", *tools])
    return {
        "model": model,
        "system": marker,
        "tools": [{"name": "Read"}, {"name": "ToolSearch"}],
        "messages": [{"role": "user", "content": [{"type": "text", "text": text}]}],
    }


def direct_request(*tools: str, marker: str = MARKER) -> dict[str, Any]:
    """The same subagent with tool search off: every MCP tool sits in `tools[]`."""
    return {
        "model": "pinned-model",
        "system": marker,
        "tools": [{"name": "Read"}, *({"name": t} for t in tools)],
    }


def compact(value: Any) -> str:
    """`jq -c` text for <value>."""
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False)


def make_capture_workspace(ws: Path, payloads: str | None = None) -> Path:
    (ws / "capture").mkdir(parents=True)
    if payloads is not None:
        (ws / "capture" / "payloads.jsonl").write_text(payloads, encoding="utf-8")
    return ws


CURSOR_PACKAGE_NAME = "cursor-subagent-effort"


@pytest.fixture
def cursor_package(tmp_path: Path, experiments_dir: Path) -> Path:
    """A copy of the Cursor package with no `results/`, inside the test's own tree."""
    copy = tmp_path / "experiments" / CURSOR_PACKAGE_NAME
    shutil.copytree(experiments_dir / CURSOR_PACKAGE_NAME, copy, symlinks=True)
    shutil.rmtree(copy / "results", ignore_errors=True)
    return copy


OPENCODE_DEBUG_STUB = textwrap.dedent("""\
    #!/usr/bin/env bash
    if [ "$1" = "--version" ]; then echo "9.9.9"; exit 0; fi
    if [ -n "${STUB_HOME_FILE:-}" ]; then printf '%s' "$HOME" > "$STUB_HOME_FILE"; fi
    if [ -n "${STUB_CALL_FILE:-}" ]; then
        if [ -p /dev/stdout ]; then kind=pipe; else kind=file; fi
        printf '%s\\n%s\\n' "$PWD" "$kind" > "$STUB_CALL_FILE"
    fi
    if [ -n "${STUB_ENV_FILE:-}" ]; then env > "$STUB_ENV_FILE"; fi
    echo "stub stderr" >&2
    if [ -n "${STUB_ORACLE:-}" ]; then cat "$STUB_ORACLE"; fi
    exit "${STUB_EXIT:-0}"
""")


AGENT_VIEW = {
    "name": "probe",
    "model": {"providerID": "anthropic", "modelID": "claude-sonnet-4-5"},
    "variant": "high",
}


@pytest.fixture
def opencode_debug_stub(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """`OPENCODE_DEBUG_STUB`, installed as `opencode` first on `PATH`.

    The stub answers `--version`, and for `debug …` prints the file named by `STUB_ORACLE`
    (nothing when unset) and exits `STUB_EXIT`. Side files let a test see how it was run:
    `STUB_HOME_FILE` receives its `$HOME`, `STUB_CALL_FILE` its working directory and whether
    its stdout was a pipe, and `STUB_ENV_FILE` its whole environment.
    """
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    stub = bin_dir / "opencode"
    stub.write_text(OPENCODE_DEBUG_STUB, encoding="utf-8")
    stub.chmod(0o755)
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")
    return bin_dir


def copy_package(experiments_dir: Path, tree: Path, name: str) -> Path:
    package = tree / name
    shutil.copytree(experiments_dir / name, package, symlinks=True)
    shutil.rmtree(package / "results", ignore_errors=True)
    return package


def oracle_file(tmp_path: Path, content: str) -> str:
    path = tmp_path / "oracle.json"
    path.write_text(content, encoding="utf-8")
    return str(path)


def unattended_manifest(**overrides: Any) -> dict[str, Any]:
    manifest: dict[str, Any] = {
        "schema_version": 1,
        "provider": "opencode",
        "driver": "unattended",
        "depth": None,
        "question": "q",
        "version_source": {"kind": "none"},
        "assertion": {"projection": ".", "expected": 1},
    }
    manifest.update(overrides)
    return manifest


STUBS = EXPERIMENTS_DIR / "lib" / "stubs"
