"""Every view a projection reads, built by the harness from what a run left on disk.

An arm view is a JSON object mapping each arm to the list of its request bodies, with the
response bodies appended when a definition asks. A capture view is the list of the
captured payloads. Files are read in sorted name order, and every JSON value in each file
counts: a sink file may hold more than one.
"""

import json
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path

from probe_harness import json_values
from probe_harness.gates import GateFailure
from probe_harness.json_values import JsonValue


@dataclass(frozen=True)
class SinkEntry:
    """One JSON value from a sink file, with the file it came from."""

    path: Path
    value: JsonValue
    # The value's 1-based position in its file, when the file holds more than one.
    position: int | None = None

    @property
    def label(self) -> str:
        """The file, and the value's position in it when that is needed to find it."""
        return str(self.path) if self.position is None else f"{self.path} value {self.position}"


@dataclass(frozen=True)
class SinkValues:
    """The bodies one arm's sink holds, one entry per JSON value, in file-name order."""

    requests: list[SinkEntry]
    responses: list[SinkEntry]


def read_sink(sink: Path) -> SinkValues:
    """Every value of `<sink>/*.request.json` and `<sink>/*.response.json`."""
    return SinkValues(
        requests=_read_values(sorted(sink.glob("*.request.json"))),
        responses=_read_values(sorted(sink.glob("*.response.json"))),
    )


def write_arm_view(out: Path, arms: Mapping[str, list[JsonValue]]) -> None:
    _write(out, dict(arms))


def write_capture_view(payloads: Path, out: Path) -> None:
    """The captured payloads, one JSON value per line, as one list."""
    try:
        values = json_values.load_stream(payloads.read_text(encoding="utf-8"))
    except ValueError as e:
        raise GateFailure(f"the captured payloads at {payloads} are not valid JSON: {e}") from None
    _write(out, values)


def _read_values(paths: list[Path]) -> list[SinkEntry]:
    entries: list[SinkEntry] = []
    for path in paths:
        try:
            values = json_values.load_stream(path.read_text(encoding="utf-8"))
        except (ValueError, OSError) as e:
            raise GateFailure(f"{path} is not valid JSON: {e}") from None
        several = len(values) > 1
        entries += (
            SinkEntry(path, value, n if several else None)
            for n, value in enumerate(values, start=1)
        )
    return entries


def _write(out: Path, value: JsonValue) -> None:
    # ASCII-escaped: a body may carry an escaped lone surrogate (a string cut between the
    # halves of an emoji), which has no UTF-8 encoding. Escaped, it reaches jq as written.
    out.write_text(json.dumps(value, indent=2, ensure_ascii=True) + "\n", encoding="utf-8")
