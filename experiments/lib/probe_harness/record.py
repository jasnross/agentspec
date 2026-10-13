"""Assert and record: the single writer of probe result records.

Every record in the repository is produced here. There is one record shape and no
hand-written exception, which is what lets `probe status` join across packages and what
makes hand-editing a record a defect rather than a workflow.

Usage:
    probe record --manifest <probe.json> --view <view.json>      [--capture <ws>] [--dry-run]
    probe record --manifest <probe.json> --selection <option-id>  --capture <ws>  [--dry-run]
"""

import json
import re
import subprocess
import sys
from dataclasses import dataclass
from datetime import UTC, datetime
from enum import StrEnum
from pathlib import Path

from probe_harness import jq_cli, json_values
from probe_harness.contract import (
    CaptureSource,
    CommandSource,
    Driver,
    Manifest,
    ManifestError,
    NoSource,
    Option,
    OptionsAssertion,
    ProjectionAssertion,
    parse_manifest,
)
from probe_harness.json_values import JsonValue


class Status(StrEnum):
    CONFIRMED = "confirmed"
    REFUTED = "refuted"
    INCONCLUSIVE = "inconclusive"


@dataclass(frozen=True)
class Record:
    """One computed record, not yet written anywhere."""

    manifest: Manifest
    status: Status
    observed: JsonValue
    tool_version: str | None
    # `%Y-%m-%dT%H%M%S` from one UTC clock read; the date is the part before `T`.
    stamp: str

    @property
    def date(self) -> str:
        return self.stamp.split("T", 1)[0]

    def document(self) -> dict[str, JsonValue]:
        """The seven keys, in order."""
        assertion: dict[str, JsonValue]
        match self.manifest.assertion:
            case ProjectionAssertion(projection=projection):
                assertion = {"projection": projection}
            case OptionsAssertion(options=options):
                assertion = {"options": [_option_document(o) for o in options]}
        assertion["expected"] = self.manifest.assertion.expected
        assertion["observed"] = self.observed
        return {
            "schema_version": 1,
            "provider": self.manifest.provider.value,
            "status": self.status.value,
            "depth": self.manifest.depth.value if self.manifest.depth is not None else None,
            "date": self.date,
            "tool_version": self.tool_version,
            "assertion": assertion,
        }

    def text(self) -> str:
        """The record as written, with no trailing newline."""
        return json_values.dumps(self.document())


class RecordRefused(Exception):
    """A refusal. <lines> are the recorder's own words; <detail> is output it passes through."""

    def __init__(self, *lines: str, detail: tuple[str, ...] = ()) -> None:
        super().__init__("\n".join(lines))
        self.lines = lines
        self.detail = detail


def compute(
    manifest: Manifest,
    *,
    view: Path | None = None,
    selection: str | None = None,
    capture: Path | None = None,
) -> Record:
    """Gate the invocation, compare, and return the record; raise `RecordRefused` otherwise.

    Prints nothing and writes nothing. Assumes jq is installed: `main` checks for it, and
    every family's preflight requires it.
    """
    # Status is always computed from the comparison, because a caller that could assert a
    # status could assert one the evidence does not support.
    if view is not None and selection is not None:
        raise RecordRefused("--view and --selection are mutually exclusive")
    if view is None and selection is None:
        raise RecordRefused("exactly one of --view or --selection is required")

    # The invocation mode must match the assertion shape. Mismatched, a projection over
    # an options manifest resolves to `null` and records `refuted`, and `refuted` is the
    # strong signal: an operator wiring error would raise a false alarm.
    if view is not None and not isinstance(manifest.assertion, ProjectionAssertion):
        raise RecordRefused(
            "--view needs a projection assertion; this manifest declares options "
            f"(use --selection): {manifest.path}"
        )
    if selection is not None and not isinstance(manifest.assertion, OptionsAssertion):
        raise RecordRefused(
            "--selection needs an options assertion; this manifest declares a projection "
            f"(use --view): {manifest.path}"
        )

    if manifest.driver == Driver.MANUAL and capture is None:
        raise RecordRefused('driver "manual" requires --capture')

    # The non-empty check reads the file the *provider* wrote, never `--view`, which the
    # runner writes moments before recording. A hook that never fired is what it catches.
    if capture is not None:
        payloads = capture / "capture" / "payloads.jsonl"
        if not _non_empty(payloads):
            raise RecordRefused(f"capture payloads absent or empty: {payloads}")

    option_status = ""
    observed: JsonValue
    if selection is not None:
        observed, option_status = _select(manifest, selection)
    else:
        assert view is not None
        observed = _project(manifest, view)

    if option_status:
        # A `couldnt-tell` option declares `inconclusive`, so a tired operator picking it
        # cannot manufacture a pass.
        status = Status(option_status)
    elif json_values.equal(manifest.assertion.expected, observed):
        status = Status.CONFIRMED
    else:
        status = Status.REFUTED

    tool_version = _tool_version(manifest, capture)

    # One UTC clock read feeds both the record's `date` and the filename. The time
    # component stops a same-day re-run at the same version from overwriting its
    # predecessor.
    stamp = datetime.now(UTC).strftime("%Y-%m-%dT%H%M%S")
    return Record(manifest, status, observed, tool_version or None, stamp)


def write(record: Record) -> Path:
    """Write <record> to `results/<stamp>-<provider>-<version>.json` beside its manifest."""
    results_dir = record.manifest.path.parent / "results"
    slug = _slug(record.tool_version or "unknown")
    path = results_dir / f"{record.stamp}-{record.manifest.provider}-{slug}.json"
    text = record.text()
    results_dir.mkdir(parents=True, exist_ok=True)
    path.write_text(text + "\n", encoding="utf-8")
    return path


def report(record: Record, written: Path | None) -> None:
    """Print a computed record's outcome: the written path, or under a dry run the record."""
    if written is None:
        # Not a record: what one would contain, printed so a probe's wiring can be checked
        # before a live or billed run produces one.
        sys.stdout.write(record.text() + "\n")
        sys.stdout.flush()
        print(
            f"record: dry run — {record.status} — observed {_compact(record.observed)} "
            "(no file written)",
            file=sys.stderr,
        )
        return
    sys.stdout.write(f"{written}\n")
    sys.stdout.flush()
    print(f"record: {record.status} — observed {_compact(record.observed)}", file=sys.stderr)


def main(argv: list[str]) -> int:
    """`probe record`: parse the arguments, compute, and write (or print) one record."""
    try:
        jq_cli.require_jq()
    except jq_cli.ToolsMissing as e:
        print(f"probe: missing required tool(s): {' '.join(e.tools)}", file=sys.stderr)
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

    # An empty value is the same as an absent one.
    manifest_path = values.get("--manifest", "")
    if not manifest_path:
        print("record: --manifest is required", file=sys.stderr)
        return 1
    view = values.get("--view", "")
    capture = values.get("--capture", "")

    try:
        manifest = parse_manifest(manifest_path)
        record = compute(
            manifest,
            view=Path(view) if view else None,
            selection=values.get("--selection", "") or None,
            capture=Path(capture) if capture else None,
        )
        report(record, None if dry_run else write(record))
    except ManifestError as e:
        print(f"record: {e}", file=sys.stderr)
        return 1
    except RecordRefused as e:
        for line in e.lines:
            print(f"record: {line}", file=sys.stderr)
        sys.stderr.writelines(f"{line}\n" for line in e.detail)
        return 1
    except jq_cli.JqError as e:
        sys.stderr.write(e.stderr)
        return 1
    return 0


def _select(manifest: Manifest, selection: str) -> tuple[str, str]:
    """The selected option id and the option's declared status."""
    assert isinstance(manifest.assertion, OptionsAssertion)
    option = next((o for o in manifest.assertion.options if o.id == selection), None)
    if option is None:
        raise RecordRefused(f'selection "{selection}" is not one of the manifest\'s option ids')
    return selection, option.status or ""


def _project(manifest: Manifest, view: Path) -> JsonValue:
    """The projection's single value over <view>; `null` when it yields none."""
    assert isinstance(manifest.assertion, ProjectionAssertion)
    if not view.is_file():
        raise RecordRefused(f"view not found: {view}")
    projection = manifest.assertion.projection
    try:
        output = jq_cli.evaluate(projection, view, compact=True)
    except jq_cli.JqError as e:
        raise RecordRefused(
            f"projection failed: {projection}", detail=tuple(e.stderr.splitlines())
        ) from None
    try:
        values = json_values.load_stream(output)
    except ValueError as e:
        raise RecordRefused(f"projection failed: {projection}", detail=(str(e),)) from None
    # A projection emitting more than one value cannot be compared structurally against
    # a single `expected`.
    if len(values) > 1:
        raise RecordRefused(
            f"projection must yield exactly one JSON value, but yielded several: {projection}"
        )
    return values[0] if values else None


def _tool_version(manifest: Manifest, capture: Path | None) -> str:
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
                    expr, capture / "capture" / "payloads.jsonl", slurp=True, raw=True
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


def _slug(version: str) -> str:
    """Every character outside `A-Za-z0-9._-` becomes `_`."""
    return re.sub(r"[^A-Za-z0-9._-]", "_", version)


def _non_empty(path: Path) -> bool:
    try:
        return path.is_file() and path.stat().st_size > 0
    except OSError:
        return False


def _option_document(option: Option) -> dict[str, JsonValue]:
    document: dict[str, JsonValue] = {"id": option.id, "text": option.text}
    if option.status is not None:
        document["status"] = option.status
    return document


def _compact(value: JsonValue) -> str:
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False)
