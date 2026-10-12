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
import subprocess
import textwrap
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path

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
    if record is None or record is False:
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
