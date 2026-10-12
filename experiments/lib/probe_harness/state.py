"""A package's freshness, derived from its manifest and its committed records.

Nothing here runs a probe or prints a report. `probe status` renders staleness and
`probe run --stale` filters on it, and the only way to guarantee the two agree is to have
one implementation.

Manifests are read here without the recorder's gates: the report must still describe a
package whose manifest is broken, and say so.
"""

import json
import shutil
from dataclasses import dataclass
from fnmatch import fnmatchcase
from pathlib import Path
from typing import Any

from probe_harness.contract import jq_raw
from probe_harness.record import run_version_command

# Bounds a version command, so a wedged one cannot hang `just check`.
VERSION_COMMAND_TIMEOUT_SECONDS = 10

# The stamped filename shape. An unrelated `.json` dropped into `results/` cannot sort
# after every record and be taken for the newest.
RECORD_NAME_PATTERN = "[0-9]*T[0-9]*-*.json"


@dataclass(frozen=True)
class InstalledVersion:
    """What to show for the installed version, and whether it compares against a record.

    The flag is separate from the string on purpose: sniffing the display text for a
    sentinel would treat a real version starting with that word as incomparable.
    """

    display: str
    comparable: bool


def newest_record(package: Path) -> Path | None:
    """The newest record in <package>, or `None` when it has none.

    The filename carries a UTC date and time, so name order is run order.
    """
    results = package / "results"
    try:
        names = sorted(
            entry.name
            for entry in results.iterdir()
            if fnmatchcase(entry.name, RECORD_NAME_PATTERN) and entry.exists()
        )
    except OSError:
        return None
    return results / names[-1] if names else None


def load_record(path: Path) -> Any | None:
    """A record's JSON value, or `None` when it is unreadable (as `jq -e .` judges it)."""
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except ValueError, OSError:
        return None
    return None if value is None or value is False else value


def recorded_version(record: Any) -> str:
    """The record's tool version for display: `—` when it recorded none."""
    if not isinstance(record, dict):
        return ""
    version = record.get("tool_version")
    return "—" if version is None else jq_raw(version)


def resolve_installed_version(manifest: Path) -> InstalledVersion:
    """Resolve the installed version a manifest's `version_source` declares."""
    if not manifest.is_file():
        return InstalledVersion("", False)
    kind = _version_kind(manifest)
    if kind is None:
        # A broken manifest must not be indistinguishable from one declaring no source.
        return InstalledVersion("manifest unreadable", False)

    match kind:
        case "command":
            command_line = _command_line(manifest)
            if not command_line:
                return InstalledVersion("manifest declares no command", False)
            # Split into argv and exec directly, never through a shell: `just check` runs
            # this on every invocation, and a manifest is a data file reviewed as data.
            argv = tuple(
                w for w in command_line.split("\n", 1)[0].replace("\t", " ").split(" ") if w
            )
            if not argv or shutil.which(argv[0]) is None:
                return InstalledVersion("not installed", False)
            resolved = run_version_command(argv, timeout=VERSION_COMMAND_TIMEOUT_SECONDS)
            if not resolved:
                return InstalledVersion("version command produced nothing", False)
            return InstalledVersion(resolved, True)
        case "capture":
            # Knowable only by running the probe, so a caller running none cannot compare.
            return InstalledVersion("from capture", False)
        case "none":
            return InstalledVersion("human check", False)
        case _:
            return InstalledVersion("unrecognized version_source kind", False)


def needs_run(package: Path) -> tuple[bool, str]:
    """Whether <package> is owed a run, and the phrase a caller can print either way.

    Owed means exactly one of two things: the package has never produced a readable
    record, or its recorded version is comparable against the installed one and differs.
    Everything else is fresh, including a version that cannot be compared at all; treating
    "cannot tell" as owed would put every Cursor package permanently in the run set.

    The driver is deliberately not consulted. Whether a stale package may run is the
    caller's gate.
    """
    path = newest_record(package)
    if path is None:
        return True, "no record"
    record = load_record(path)
    if record is None:
        return True, "newest record unreadable"

    recorded = recorded_version(record)
    installed = resolve_installed_version(package / "probe.json")
    if installed.comparable:
        if installed.display != recorded:
            return True, f"recorded {recorded} · installed {installed.display}"
        return False, f"version {recorded} unchanged"
    return False, f"recorded {recorded} · {installed.display or 'no version source'}"


def manifest_field(manifest: Path, key: str, default: str) -> str:
    """`jq -r '.<key> // "<default>"'` over a manifest, empty when it cannot be read."""
    try:
        root = json.loads(manifest.read_text(encoding="utf-8"))
    except ValueError, OSError:
        return ""
    if root is None:
        return default
    if not isinstance(root, dict):
        return ""
    value = root.get(key)
    return default if value is None or value is False else jq_raw(value)


def _version_kind(manifest: Path) -> str | None:
    """`.version_source.kind // "none"`, or `None` where jq would fail to evaluate it."""
    try:
        root = json.loads(manifest.read_text(encoding="utf-8"))
    except ValueError, OSError:
        return None
    source = _index(root, "version_source")
    if source is _ERROR:
        return None
    kind = _index(source, "kind")
    if kind is _ERROR:
        return None
    return "none" if kind is None or kind is False else jq_raw(kind)


def _command_line(manifest: Path) -> str:
    try:
        root = json.loads(manifest.read_text(encoding="utf-8"))
    except ValueError, OSError:
        return ""
    command = _index(_index(root, "version_source"), "command")
    if command is _ERROR or command is None or command is False:
        return ""
    return jq_raw(command)


_ERROR = object()


def _index(value: Any, key: str) -> Any:
    """jq's `.key`: `null` through `null`, a member of an object, an error otherwise."""
    if value is _ERROR:
        return _ERROR
    if value is None:
        return None
    if isinstance(value, dict):
        return value.get(key)
    return _ERROR
