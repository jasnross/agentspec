"""Every jq subprocess the harness starts.

jq keeps the verdict, the record, and the view: projections, `wait_for` filters, version
expressions, custom checks, the `expected == observed` comparison, record assembly and
view assembly all run here, with the flags the bash harness used. A probe author runs the
same jq at their shell, so a manifest expression means one thing everywhere.

No other module calls jq; `test_jq_boundary.py` enforces that.
"""

import shutil
import subprocess
from collections.abc import Mapping, Sequence
from pathlib import Path

JQ = "jq"


class ToolsMissing(Exception):
    def __init__(self, tools: Sequence[str]) -> None:
        super().__init__(" ".join(tools))
        self.tools = tuple(tools)


class JqError(Exception):
    """jq exited nonzero. `stdout` holds whatever it printed before failing."""

    def __init__(self, returncode: int, stderr: str, stdout: str) -> None:
        super().__init__(stderr)
        self.returncode = returncode
        self.stderr = stderr
        self.stdout = stdout


def require_jq() -> None:
    if shutil.which(JQ) is None:
        raise ToolsMissing([JQ])


def evaluate(
    expr: str,
    input_path: Path | None = None,
    *,
    stdin: str | None = None,
    null_input: bool = False,
    slurp: bool = False,
    compact: bool = False,
    raw: bool = False,
    args: Mapping[str, str] | None = None,
    argjson: Mapping[str, str] | None = None,
    slurpfiles: Mapping[str, Path] | None = None,
) -> str:
    """Run jq over <input_path> (or <stdin>) and return stdout; raise `JqError` on failure."""
    argv = _argv(
        expr,
        input_path,
        null_input=null_input,
        slurp=slurp,
        compact=compact,
        raw=raw,
        args=args,
        argjson=argjson,
        slurpfiles=slurpfiles,
    )
    return _run(argv, stdin)


def test(
    expr: str,
    input_path: Path | None = None,
    *,
    stdin: str | None = None,
    null_input: bool = False,
    slurp: bool = False,
    args: Mapping[str, str] | None = None,
    argjson: Mapping[str, str] | None = None,
    slurpfiles: Mapping[str, Path] | None = None,
) -> bool:
    """`jq -e … >/dev/null 2>&1`: true exactly when jq exits 0.

    Any nonzero exit is false, including a runtime error, as with the bash idiom.
    """
    argv = _argv(
        expr,
        input_path,
        null_input=null_input,
        slurp=slurp,
        compact=False,
        raw=False,
        args=args,
        argjson=argjson,
        slurpfiles=slurpfiles,
    )
    argv.insert(1, "-e")
    completed = subprocess.run(
        argv,
        input=stdin,
        stdin=None if stdin is not None else subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        text=True,
        encoding="utf-8",
        check=False,
    )
    return completed.returncode == 0


def compile_error(expr: str) -> str | None:
    """jq's diagnostic when <expr> does not compile, or `None` when it does."""
    try:
        # The newline keeps a trailing `#` comment in <expr> from swallowing the `)`.
        _run([JQ, "-n", f"empty | ({expr}\n)"], None)
    except JqError as e:
        return e.stderr
    return None


def equal(expected_text: str, observed_text: str) -> bool:
    """Structural comparison, never a string one: immune to key order and whitespace."""
    return test("$a == $b", null_input=True, argjson={"a": expected_text, "b": observed_text})


def assemble_record(
    manifest: Path,
    shape: str,
    observed_text: str,
    status: str,
    date: str,
    tool_version: str,
) -> str:
    """The seven-key record, pretty-printed by jq, with no trailing newline.

    Every value that came from the manifest is read by jq from the manifest file, so a
    number keeps the literal its author wrote.
    """
    program = """
        $m[0] as $m
        | {
            schema_version: $m.schema_version,
            provider: $m.provider,
            status: $status,
            depth: $m.depth,
            date: $date,
            tool_version: (if $tool_version == "" then null else $tool_version end),
            assertion: ({($shape): $m.assertion[$shape], expected: $m.assertion.expected}
                        + {observed: $observed})
          }
    """
    out = evaluate(
        program,
        null_input=True,
        args={"shape": shape, "status": status, "date": date, "tool_version": tool_version},
        argjson={"observed": observed_text},
        slurpfiles={"m": manifest},
    )
    return out.rstrip("\n")


def assemble_arm_view_claude(ws: Path, arms: Sequence[str], out: Path) -> None:
    """`{<arm>: [<request bodies>], …}`, folded one arm at a time from `{}`."""
    view = evaluate("{}", null_input=True).rstrip("\n")
    for arm in arms:
        requests = sorted((ws / arm / "sink").glob("*.request.json"))
        bodies = _run([JQ, "-s", ".", *map(str, requests)], None).rstrip("\n")
        view = evaluate(".[$a] = $v", stdin=view, args={"a": arm}, argjson={"v": bodies}).rstrip(
            "\n"
        )
    out.write_text(view + "\n", encoding="utf-8")


def assemble_arm_view_opencode(ws: Path, arms: Sequence[str], out: Path) -> None:
    """Slurp each arm into `<arm>/requests.json`, then join them under their arm names."""
    for arm in arms:
        requests = sorted((ws / arm / "sink").glob("*.request.json"))
        bodies = _run([JQ, "-s", ".", *map(str, requests)], None)
        (ws / arm / "requests.json").write_text(bodies, encoding="utf-8")
    literal = "{" + ", ".join(f"{arm}: ${arm}[0]" for arm in arms) + "}"
    view = evaluate(
        literal, null_input=True, slurpfiles={arm: ws / arm / "requests.json" for arm in arms}
    )
    out.write_text(view, encoding="utf-8")


def append_responses(view: Path, arm: str, response_files: Sequence[Path]) -> None:
    """Append an arm's response bodies to its view entry, replacing the view atomically."""
    concatenated = "".join(path.read_text(encoding="utf-8") for path in response_files)
    updated = evaluate(
        ".[$a] += $r",
        view,
        stdin=concatenated,
        args={"a": arm},
        slurpfiles={"r": Path("/dev/stdin")},
    )
    staged = view.with_name("view.next.json")
    staged.write_text(updated, encoding="utf-8")
    staged.replace(view)


def slurp_view(payloads: Path, out: Path) -> None:
    out.write_text(_run([JQ, "-s", ".", str(payloads)], None), encoding="utf-8")


def _argv(
    expr: str,
    input_path: Path | None,
    *,
    null_input: bool,
    slurp: bool,
    compact: bool,
    raw: bool,
    args: Mapping[str, str] | None,
    argjson: Mapping[str, str] | None,
    slurpfiles: Mapping[str, Path] | None,
) -> list[str]:
    argv = [JQ]
    for enabled, flag in ((null_input, "-n"), (slurp, "-s"), (compact, "-c"), (raw, "-r")):
        if enabled:
            argv.append(flag)
    for name, value in (args or {}).items():
        argv += ["--arg", name, value]
    for name, value in (argjson or {}).items():
        argv += ["--argjson", name, value]
    for name, path in (slurpfiles or {}).items():
        argv += ["--slurpfile", name, str(path)]
    argv.append(expr)
    if input_path is not None:
        argv.append(str(input_path))
    return argv


def _run(argv: list[str], stdin: str | None) -> str:
    completed = subprocess.run(
        argv,
        input=stdin,
        stdin=None if stdin is not None else subprocess.DEVNULL,
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    if completed.returncode != 0:
        raise JqError(completed.returncode, completed.stderr, completed.stdout)
    return completed.stdout
