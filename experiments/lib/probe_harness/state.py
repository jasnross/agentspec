"""A package's freshness, derived from its manifest and its committed records.

Nothing here runs a probe or prints a report. `probe status` renders staleness and
`probe run --stale` filters on it, and the only way to guarantee the two agree is to have
one implementation.

Manifests are read through `parse_manifest`, like every other reader. A package whose
manifest fails a gate is still described, as `manifest invalid`, so the report can say
which package is broken.
"""

import shutil
from dataclasses import dataclass
from fnmatch import fnmatchcase
from pathlib import Path

from probe_harness.contract import (
    CaptureSource,
    CommandSource,
    ManifestError,
    NoSource,
    Record,
    parse_manifest,
    parse_record,
)
from probe_harness.record import VERSION_COMMAND_TIMEOUT_SECONDS, run_version_command

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


def load_record(path: Path) -> Record | None:
    """The record at <path>, or `None` when it cannot be read or is not a valid record."""
    return parse_record(path)


def recorded_version(record: Record) -> str:
    """The record's tool version for display: `—` when it recorded none."""
    return "—" if record.tool_version is None else record.tool_version


def resolve_installed_version(manifest_path: Path) -> InstalledVersion:
    """Resolve the installed version a manifest's `version_source` declares."""
    if not manifest_path.is_file():
        return InstalledVersion("", False)
    try:
        manifest = parse_manifest(manifest_path)
    except ManifestError:
        # A broken manifest must not be indistinguishable from one declaring no source.
        return InstalledVersion("manifest invalid", False)

    match manifest.version_source:
        case CommandSource(argv=argv):
            # Executed directly, never through a shell: `just check` runs this on every
            # invocation, and a manifest is a data file reviewed as data.
            if shutil.which(argv[0]) is None:
                return InstalledVersion("not installed", False)
            resolved = run_version_command(argv, timeout=VERSION_COMMAND_TIMEOUT_SECONDS)
            if not resolved:
                return InstalledVersion("version command produced nothing", False)
            return InstalledVersion(resolved, True)
        case CaptureSource():
            # Knowable only by running the probe, so a caller running none cannot compare.
            return InstalledVersion("from capture", False)
        case NoSource():
            return InstalledVersion("human check", False)


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
        return True, "newest record unreadable or invalid"

    recorded = recorded_version(record)
    installed = resolve_installed_version(package / "probe.json")
    if installed.comparable:
        if installed.display != recorded:
            return True, f"recorded {recorded} · installed {installed.display}"
        return False, f"version {recorded} unchanged"
    return False, f"recorded {recorded} · {installed.display or 'no version source'}"
