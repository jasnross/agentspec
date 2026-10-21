"""Every jq subprocess the harness starts.

jq evaluates the expressions a probe author wrote, and nothing else: a manifest's
projection, `wait_for` filter and `version_source.jq`, and a definition's custom checks.
The author runs the same jq at their shell, so an expression means one thing everywhere.
The harness reads and writes JSON itself, in `json_values.py`.

No other module calls jq; `test_repo_jq_boundary.py` enforces that.
"""

import subprocess
from collections.abc import Mapping
from pathlib import Path

JQ = "jq"


class JqError(Exception):
    """jq exited nonzero. `stdout` holds whatever it printed before failing."""

    def __init__(self, returncode: int, stderr: str, stdout: str) -> None:
        super().__init__(stderr)
        self.returncode = returncode
        self.stderr = stderr
        self.stdout = stdout


def evaluate(
    expr: str,
    input_path: Path,
    *,
    slurp: bool = False,
    compact: bool = False,
    raw: bool = False,
    args: Mapping[str, str] | None = None,
) -> str:
    """Run jq over <input_path> and return stdout; raise `JqError` on failure."""
    return _run(_argv(expr, input_path, slurp=slurp, compact=compact, raw=raw, args=args))


def test(
    expr: str,
    input_path: Path,
    *,
    slurp: bool = False,
    args: Mapping[str, str] | None = None,
) -> bool:
    """`jq -e … >/dev/null 2>&1`: true exactly when jq exits 0.

    Any nonzero exit is false, including a runtime error.
    """
    argv = _argv(expr, input_path, slurp=slurp, compact=False, raw=False, args=args)
    argv.insert(1, "-e")
    completed = subprocess.run(
        argv,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    return completed.returncode == 0


def compile_error(expr: str) -> str | None:
    """jq's diagnostic when <expr> does not compile, or `None` when it does."""
    try:
        # The newline keeps a trailing `#` comment in <expr> from swallowing the `)`.
        _run([JQ, "-n", f"empty | ({expr}\n)"])
    except JqError as e:
        return e.stderr
    return None


def _argv(
    expr: str,
    input_path: Path,
    *,
    slurp: bool,
    compact: bool,
    raw: bool,
    args: Mapping[str, str] | None,
) -> list[str]:
    argv = [JQ]
    for enabled, flag in ((slurp, "-s"), (compact, "-c"), (raw, "-r")):
        if enabled:
            argv.append(flag)
    for name, value in (args or {}).items():
        argv += ["--arg", name, value]
    return [*argv, expr, str(input_path)]


def _run(argv: list[str]) -> str:
    completed = subprocess.run(
        argv,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    if completed.returncode != 0:
        raise JqError(completed.returncode, completed.stderr, completed.stdout)
    return completed.stdout
