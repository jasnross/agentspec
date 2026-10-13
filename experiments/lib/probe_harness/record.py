"""Assert and record: the single writer of probe result records.

Every record in the repository is produced here. There is one record shape and no
hand-written exception, which is what lets `probe status` join across packages and what
makes hand-editing a record a defect rather than a workflow.

Usage:
    probe record --manifest <probe.json> --view <view.json>      [--capture <ws>] [--dry-run]
    probe record --manifest <probe.json> --selection <option-id>  --capture <ws>  [--dry-run]
"""

import json
import os
import re
import subprocess
import sys
from datetime import UTC, datetime
from pathlib import Path

from probe_harness import jq_cli
from probe_harness.contract import (
    CaptureSource,
    CommandSource,
    Driver,
    Manifest,
    ManifestError,
    NoSource,
    OptionsAssertion,
    ProjectionAssertion,
    parse_manifest,
)


class RecordRefused(Exception):
    """A refusal, printed as `record: <message>` with exit 1."""


def record(
    manifest_path: str | Path,
    *,
    view: str | Path | None = None,
    selection: str | None = None,
    capture: str | Path | None = None,
    dry_run: bool = False,
) -> int:
    """Gate, compare, and write (or print) one record. Returns the exit code.

    Paths are kept as given, so every message names a path exactly as the caller spelled it.
    """
    try:
        jq_cli.require_jq()
    except jq_cli.ToolsMissing as e:
        _missing_tools(e)
        return 1
    try:
        return _record(
            os.fspath(manifest_path),
            os.fspath(view) if view is not None else None,
            selection,
            os.fspath(capture) if capture is not None else None,
            dry_run,
        )
    except RecordRefused as e:
        print(f"record: {e}", file=sys.stderr)
        return 1
    except jq_cli.JqError as e:
        # An unguarded jq failure ends the run with jq's own diagnostic and status, as
        # it did under `set -e`.
        sys.stderr.write(e.stderr)
        return e.returncode


def main(argv: list[str]) -> int:
    try:
        jq_cli.require_jq()
    except jq_cli.ToolsMissing as e:
        _missing_tools(e)
        return 1

    values: dict[str, str] = {}
    dry_run = False
    rest = list(argv)
    while rest:
        flag = rest.pop(0)
        if flag == "--dry-run":
            dry_run = True
        elif flag in ("--manifest", "--view", "--selection", "--capture"):
            if not rest:
                print(f"record: {flag} requires a value", file=sys.stderr)
                return 1
            values[flag] = rest.pop(0)
        else:
            print(f"record: unknown argument: {flag}", file=sys.stderr)
            return 1

    # An empty value is the same as an absent one, as with the shell's `[ -n … ]`.
    manifest = values.get("--manifest", "")
    if not manifest:
        print("record: --manifest is required", file=sys.stderr)
        return 1
    view = values.get("--view", "")
    capture = values.get("--capture", "")
    return record(
        manifest,
        view=view or None,
        selection=values.get("--selection", "") or None,
        capture=capture or None,
        dry_run=dry_run,
    )


def _record(
    manifest_path: str,
    view: str | None,
    selection: str | None,
    capture: str | None,
    dry_run: bool,
) -> int:
    try:
        manifest = parse_manifest(manifest_path)
    except ManifestError as e:
        raise RecordRefused(str(e)) from None

    # Status is always computed from the comparison, because a caller that could assert a
    # status could assert one the evidence does not support.
    if view is not None and selection is not None:
        raise RecordRefused("--view and --selection are mutually exclusive")
    if view is None and selection is None:
        raise RecordRefused("exactly one of --view or --selection is required")

    # The invocation mode must match the assertion shape. Mismatched, a projection over
    # an options manifest resolves to `null` and records `refuted`, and `refuted` is the
    # strong signal: an operator wiring error would raise a false alarm.
    shape = "projection" if isinstance(manifest.assertion, ProjectionAssertion) else "options"
    if view is not None and shape != "projection":
        raise RecordRefused(
            "--view needs a projection assertion; this manifest declares options "
            f"(use --selection): {manifest_path}"
        )
    if selection is not None and shape != "options":
        raise RecordRefused(
            "--selection needs an options assertion; this manifest declares a projection "
            f"(use --view): {manifest_path}"
        )

    if manifest.driver == Driver.MANUAL and capture is None:
        raise RecordRefused('driver "manual" requires --capture')

    # The non-empty check reads the file the *provider* wrote, never `--view`, which the
    # runner writes moments before recording. A hook that never fired is what it catches.
    if capture is not None:
        payloads = f"{capture}/capture/payloads.jsonl"
        if not _non_empty(Path(payloads)):
            raise RecordRefused(f"capture payloads absent or empty: {payloads}")

    expected = jq_cli.evaluate(".assertion.expected", Path(manifest_path), compact=True).rstrip(
        "\n"
    )

    option_status = ""
    if selection is not None:
        observed, option_status = _select(manifest, selection)
    else:
        assert view is not None
        observed = _project(manifest, view)

    if option_status:
        # A `couldnt-tell` option declares `inconclusive`, so a tired operator picking it
        # cannot manufacture a pass.
        status = option_status
    elif jq_cli.equal(expected, observed):
        status = "confirmed"
    else:
        status = "refuted"

    tool_version = _tool_version(manifest, capture)

    # One UTC clock read feeds both the record's `date` and the filename. The time
    # component stops a same-day re-run at the same version from overwriting its
    # predecessor.
    stamped = datetime.now(UTC).strftime("%Y-%m-%dT%H%M%S")
    record_date = stamped.split("T", 1)[0]

    text = jq_cli.assemble_record(
        Path(manifest_path), shape, observed, status, record_date, tool_version
    )

    if dry_run:
        # Not a record: what one would contain, printed so a probe's wiring can be checked
        # before a live or billed run produces one. Exits 0 whatever the comparison
        # computed, since a discriminator run is expected to refute.
        sys.stdout.write(text + "\n")
        sys.stdout.flush()
        print(
            f"record: dry run — {status} — observed {observed} (no file written)", file=sys.stderr
        )
        return 0

    results_dir = _logical_directory(Path(manifest_path).parent) / "results"
    slug = _slug(tool_version or "unknown")
    record_path = results_dir / f"{stamped}-{manifest.provider}-{slug}.json"
    results_dir.mkdir(parents=True, exist_ok=True)
    record_path.write_text(text + "\n", encoding="utf-8")

    sys.stdout.write(f"{record_path}\n")
    sys.stdout.flush()
    print(f"record: {status} — observed {observed}", file=sys.stderr)
    return 0


def _select(manifest: Manifest, selection: str) -> tuple[str, str]:
    """The observed text and the option's declared status for a selected option id."""
    assert isinstance(manifest.assertion, OptionsAssertion)
    option = next((o for o in manifest.assertion.options if o.id == selection), None)
    if option is None:
        raise RecordRefused(f'selection "{selection}" is not one of the manifest\'s option ids')
    return json.dumps(selection, ensure_ascii=False), option.status or ""


def _project(manifest: Manifest, view: str) -> str:
    """The projection's single value over <view>, as compact JSON text."""
    assert isinstance(manifest.assertion, ProjectionAssertion)
    if not Path(view).is_file():
        raise RecordRefused(f"view not found: {view}")
    projection = manifest.assertion.projection
    try:
        observed = jq_cli.evaluate(projection, Path(view), compact=True).rstrip("\n")
    except jq_cli.JqError as e:
        sys.stderr.write(e.stderr)
        raise RecordRefused(f"projection failed: {projection}") from None
    if not observed:
        observed = "null"
    # A projection emitting more than one value cannot be compared structurally against
    # a single `expected`.
    if "\n" in observed:
        raise RecordRefused(
            f"projection must yield exactly one JSON value, but yielded several: {projection}"
        )
    return observed


def _tool_version(manifest: Manifest, capture: str | None) -> str:
    version: str
    match manifest.version_source:
        case CommandSource(argv=argv):
            version = run_version_command(argv, timeout=None)
        case CaptureSource(jq=expr):
            if capture is None:
                raise RecordRefused('version_source.kind "capture" requires --capture')
            # Slurped, as every declared capture expression is array-shaped.
            try:
                out = jq_cli.evaluate(
                    expr, Path(capture, "capture", "payloads.jsonl"), slurp=True, raw=True
                )
            except jq_cli.JqError as e:
                out = e.stdout
            version = out.rstrip("\n")
        case NoSource():
            version = ""
    return "" if version == "null" else version


def run_version_command(argv: tuple[str, ...], *, timeout: float | None) -> str:
    """Line 1 of a version command's stdout, or empty when it cannot run or times out.

    Executed directly rather than through a shell: a manifest is a data file reviewed as
    data. stdin is closed so a command that reads it cannot block.
    """
    try:
        completed = subprocess.run(
            list(argv),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=timeout,
            check=False,
        )
    except OSError, subprocess.TimeoutExpired:
        return ""
    return completed.stdout.decode("utf-8", errors="replace").split("\n", 1)[0]


def _logical_directory(directory: Path) -> Path:
    """<directory> made absolute and normalized against the logical working directory.

    The shell's `cd … && pwd` kept symlinks in the path, so resolving would turn `/var/…`
    into `/private/var/…` on macOS. `$PWD` is that logical path whenever it still names the
    working directory.
    """
    base = os.environ.get("PWD", "")
    try:
        logical = bool(base) and Path(base).is_absolute() and Path(base).samefile(".")
    except OSError:
        logical = False
    root = Path(base) if logical else Path.cwd()
    return Path(os.path.normpath(root / directory))


def _slug(version: str) -> str:
    """Every character outside `A-Za-z0-9._-` becomes `_`."""
    return re.sub(r"[^A-Za-z0-9._-]", "_", version)


def _non_empty(path: Path) -> bool:
    try:
        return path.is_file() and path.stat().st_size > 0
    except OSError:
        return False


def _missing_tools(e: jq_cli.ToolsMissing) -> None:
    print(f"probe: missing required tool(s): {' '.join(e.tools)}", file=sys.stderr)
