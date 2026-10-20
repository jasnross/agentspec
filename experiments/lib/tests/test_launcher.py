"""Coverage for `experiments/probe`'s environment step: it syncs `uv.lock` into
`experiments/.venv` and re-executes itself there, refusing what it cannot prepare before any
harness module is imported.
"""

import os
import shutil
import subprocess
import sys
from pathlib import Path

from tests.conftest import EXPERIMENTS_DIR, Result


def _launch(
    argv: list[str], env: dict[str, str], cwd: Path | None = None, timeout: float = 120
) -> Result:
    completed = subprocess.run(
        argv,
        env=env,
        cwd=cwd,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=timeout,
        check=False,
    )
    return Result(completed.returncode, completed.stdout, completed.stderr)


def _unsynced_environment(**overrides: str) -> dict[str, str]:
    """This process's environment as a caller outside the harness environment sees it.

    pytest runs under `uv run`, which puts the harness environment's `bin/` first on `PATH`;
    left there, the launcher's `python3` shebang would start inside the environment.
    """
    venv_bin = (Path(sys.prefix) / "bin").resolve()
    path = os.pathsep.join(
        entry
        for entry in os.environ.get("PATH", "").split(os.pathsep)
        if entry and Path(entry).resolve() != venv_bin
    )
    env = {**os.environ, "PATH": path, **overrides}
    for name in ("_AGENTSPEC_PROBE_SYNCED", "VIRTUAL_ENV", "UV_PROJECT_ENVIRONMENT"):
        env.pop(name, None)
    return env


def test_a_missing_uv_is_refused_before_any_harness_import(tmp_path: Path) -> None:
    empty = tmp_path / "empty-path"
    empty.mkdir()
    result = _launch(
        [sys.executable, str(EXPERIMENTS_DIR / "probe"), "status"],
        _unsynced_environment(PATH=str(empty)),
    )
    assert result.returncode == 1
    assert result.stderr == "probe: uv is required to run the probe harness; run it under mise\n"


def test_a_stale_lock_is_refused_with_uv_s_message(tmp_path: Path) -> None:
    copy = tmp_path / "experiments"
    copy.mkdir()
    for name in ("probe", "pyproject.toml", "uv.lock"):
        shutil.copy2(EXPERIMENTS_DIR / name, copy / name)
    shutil.copytree(
        EXPERIMENTS_DIR / "lib", copy / "lib", ignore=shutil.ignore_patterns("__pycache__")
    )
    pyproject = copy / "pyproject.toml"
    text = pyproject.read_text(encoding="utf-8")
    stale = text.replace("dependencies = [", 'dependencies = ["annotated-types", ', 1)
    assert stale != text
    pyproject.write_text(stale, encoding="utf-8")

    result = _launch([str(copy / "probe"), "status"], _unsynced_environment())

    assert result.returncode == 1
    assert "needs to be updated" in result.stderr
    assert "probe: could not prepare the harness environment" in result.stderr
    assert "Traceback" not in result.stderr


def test_an_unsynced_caller_is_re_executed_into_the_environment() -> None:
    env = _unsynced_environment()
    interpreter = shutil.which("python3", path=env["PATH"])
    assert interpreter is not None
    assert not Path(interpreter).resolve().is_relative_to(Path(sys.prefix).resolve())

    result = _launch([str(EXPERIMENTS_DIR / "probe"), "status", "--summary"], env)
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("probes: ")

    # The exit code of the re-executed harness passes through `uv run` unchanged.
    usage = _launch([str(EXPERIMENTS_DIR / "probe"), "status", "--no-such-flag"], env)
    assert usage.returncode == 2, usage.stderr


def test_a_marker_from_another_environment_is_refused() -> None:
    result = _launch(
        [str(Path(sys.base_prefix) / "bin" / "python3"), str(EXPERIMENTS_DIR / "probe"), "status"],
        {**os.environ, "_AGENTSPEC_PROBE_SYNCED": "1"},
    )
    assert result.returncode == 1
    assert "the harness environment is" in result.stderr
