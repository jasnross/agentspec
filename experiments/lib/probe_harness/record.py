"""Assert and record: the single writer of probe result records.

Every record in the repository is produced here. There is one record shape and no
hand-written exception, which is what lets `probe status` join across packages and what
makes hand-editing a record a defect rather than a workflow.

It records a judged workspace: `probe run` and `probe record <package> <workspace>` judge
it through the family's judge, and hand the result here. Nothing here runs a gate, and
nothing here runs a version command except to read one before Act (`run_version_command`).
"""

import json
import re
import subprocess
import sys
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path

from pydantic import MISSING, ValidationError

from probe_harness import console, contract, jq_cli, json_values
from probe_harness.contract import (
    CaptureSource,
    CommandSource,
    Driver,
    Manifest,
    NoSource,
    OptionsAssertion,
    ProjectionAssertion,
    RecordAssertion,
    Status,
)
from probe_harness.json_values import JsonValue

# Bounds a version command, so a wedged one cannot hang a run or `just check`.
VERSION_COMMAND_TIMEOUT_SECONDS = 10


@dataclass(frozen=True)
class JudgeResult:
    """What Judge hands to Record: exactly one of a view or a selected option id."""

    view: Path | None
    selection: str | None


@dataclass(frozen=True)
class ComputedRecord:
    """One computed record, not yet written anywhere, with what writing and reporting need."""

    manifest: Manifest
    # `%Y-%m-%dT%H%M%S` from one UTC clock read; the record's `date` is the part before `T`.
    stamp: str
    record: contract.Record

    @property
    def status(self) -> Status:
        return self.record.status

    @property
    def observed(self) -> JsonValue:
        return self.record.assertion.observed

    @property
    def tool_version(self) -> str | None:
        return self.record.tool_version

    def text(self) -> str:
        """The record as written, with no trailing newline."""
        return json_values.dumps(self.record.model_dump(mode="json"))


class RecordRefused(Exception):
    """A refusal. <lines> are the recorder's own words; <detail> is output it passes through."""

    def __init__(self, *lines: str, detail: tuple[str, ...] = ()) -> None:
        super().__init__("\n".join(lines))
        self.lines = lines
        self.detail = detail


def compute(manifest: Manifest, ws: Path, result: JudgeResult, *, dry_run: bool) -> ComputedRecord:
    """Compare a judged workspace's result and return the record; raise `RecordRefused`.

    Writes nothing. Prints only the dry-run warning for a workspace with no recorded tool
    version. Assumes jq is installed, which every caller has checked.
    """
    # The judge hands over a view for a projection and a selection for an option set; a
    # mismatch would be a wiring error, since a projection over an options manifest
    # resolves to `null` and records a false `refuted`.
    if isinstance(manifest.assertion, ProjectionAssertion):
        assert result.view is not None and result.selection is None
    else:
        assert result.selection is not None and result.view is None

    # The non-empty check reads the file the *provider* wrote, never the view, which the
    # judge writes moments before recording. A hook that never fired is what it catches.
    if manifest.driver == Driver.MANUAL:
        payloads = ws / "capture" / "payloads.jsonl"
        if not _non_empty(payloads):
            raise RecordRefused(f"capture payloads absent or empty: {payloads}")

    # Status is always computed from the comparison, because a caller that could assert a
    # status could assert one the evidence does not support.
    option_status = ""
    observed: JsonValue
    if result.selection is not None:
        observed, option_status = _select(manifest, result.selection)
    else:
        assert result.view is not None
        observed = _project(manifest, result.view)

    if option_status:
        # A `couldnt-tell` option declares `inconclusive`, so a tired operator picking it
        # cannot manufacture a pass.
        status = Status(option_status)
    elif json_values.equal(manifest.assertion.expected, observed):
        status = Status.CONFIRMED
    else:
        status = Status.REFUTED

    tool_version = _tool_version(manifest, ws, dry_run=dry_run)

    # One UTC clock read feeds both the record's `date` and the filename. The time
    # component stops a same-day re-run at the same version from overwriting its
    # predecessor.
    stamp = datetime.now(UTC).strftime("%Y-%m-%dT%H%M%S")
    version = tool_version or None
    assertion: RecordAssertion
    try:
        match manifest.assertion:
            case ProjectionAssertion(projection=projection, expected=expected):
                assertion = RecordAssertion(
                    projection=projection, expected=expected, observed=observed
                )
            case OptionsAssertion(options=options, expected=expected):
                assertion = RecordAssertion(options=options, expected=expected, observed=observed)
        document = contract.Record(
            schema_version=1,
            provider=manifest.provider,
            status=status,
            depth=manifest.depth,
            date=stamp.split("T", 1)[0],
            tool_version=version,
            assertion=assertion,
        )
    except ValidationError as e:
        # pydantic refuses a JSON value nested past about 255 levels, which a projection can
        # still yield. Its location would repeat once per level, so only the messages show.
        raise RecordRefused(
            "the observed value cannot be written as a record",
            detail=tuple(dict.fromkeys(error["msg"] for error in e.errors())),
        ) from None
    return ComputedRecord(manifest, stamp, document)


def write(record: ComputedRecord) -> Path:
    """Write <record> to `results/<stamp>-<provider>-<version>.json` beside its manifest."""
    results_dir = record.manifest.path.parent / "results"
    slug = _slug(record.tool_version or "unknown")
    path = results_dir / f"{record.stamp}-{record.manifest.provider}-{slug}.json"
    text = record.text()
    results_dir.mkdir(parents=True, exist_ok=True)
    path.write_text(text + "\n", encoding="utf-8")
    return path


def report(record: ComputedRecord, written: Path | None) -> None:
    """Print a computed record's outcome: the written path, or under a dry run the record."""
    if written is None:
        # Not a record: what one would contain, printed so a probe's wiring can be checked
        # before a live or billed run produces one.
        sys.stdout.write(record.text() + "\n")
        sys.stdout.flush()
        console.say(
            f"dry run — {record.status} — observed {_compact(record.observed)} (no file written)"
        )
        return
    sys.stdout.write(f"{written}\n")
    sys.stdout.flush()
    console.say(f"{record.status} — observed {_compact(record.observed)}")


def _select(manifest: Manifest, selection: str) -> tuple[str, str]:
    """The selected option id and the option's declared status."""
    assert isinstance(manifest.assertion, OptionsAssertion)
    option = next((o for o in manifest.assertion.options if o.id == selection), None)
    if option is None:
        raise RecordRefused(f'selection "{selection}" is not one of the manifest\'s option ids')
    return selection, "" if option.status is MISSING else option.status


def _project(manifest: Manifest, view: Path) -> JsonValue:
    """The projection's single value over <view>; `null` when it yields none."""
    assert isinstance(manifest.assertion, ProjectionAssertion)
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


def _tool_version(manifest: Manifest, ws: Path, *, dry_run: bool) -> str:
    version: str
    match manifest.version_source:
        case CommandSource():
            # The run read the version just before Act. Running the command here would
            # record whatever is installed by now, so a judge never does.
            recorded = ws / "tool_version"
            if recorded.is_file():
                version = recorded.read_text(encoding="utf-8").rstrip("\n")
            elif dry_run:
                console.say(f"no recorded tool version in {ws}; the record would carry null")
                version = ""
            else:
                raise RecordRefused(
                    f"no recorded tool version in {ws}",
                    "re-run the probe; a later judge never runs the version command.",
                )
        case CaptureSource(jq=expr):
            # Slurped, as every declared capture expression is array-shaped.
            try:
                out = jq_cli.evaluate(expr, ws / "capture" / "payloads.jsonl", slurp=True, raw=True)
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


def _compact(value: JsonValue) -> str:
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False)
