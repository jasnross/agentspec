"""A report derived from the committed records. Invokes no probe.

Everything printed comes from `experiments/*/results/*.json` plus the `version_source` each
package's own manifest declares. There is no list of providers and no list of renderings
that ought to be verified: the report describes what was measured, and a package with no
records has no row.

There are two exceptions. A `billed` package gets a `not yet run` row from its manifest: no
batch run will ever populate it, so absence from the report would be its permanent resting
state rather than a transient one. And a package whose manifest fails a gate gets a
`manifest invalid` row, since its driver is unknown and it would otherwise vanish from the
report.

Exits 0 on every report, and 2 on a usage error, which `just probe-summary` ignores through
`-@`. This hangs off `just check`, and a check that can fail the build gets muted within a
week.
"""

import sys
from dataclasses import dataclass, field
from pathlib import Path

from probe_harness.contract import Driver, Manifest, ManifestError, Status, parse_manifest
from probe_harness.state import (
    load_record,
    newest_record,
    recorded_version,
    resolve_installed_version,
)

# The widths of the five padded columns; the version column is the unpadded sixth.
COLUMN_WIDTHS = (32, 13, 17, 12)


@dataclass(frozen=True)
class StatusRow:
    provider: str
    name: str
    label: str
    depth: str
    date: str
    version: str


@dataclass
class StatusReport:
    rows: list[StatusRow] = field(default_factory=list)
    recorded: int = 0
    refuted: int = 0
    inconclusive: int = 0
    version_drift: int = 0
    billed: int = 0


def main(*, summary: bool, experiments_dir: Path) -> int:
    report = collect_status(experiments_dir)
    sys.stdout.write(render_summary(report) if summary else render_report(report))
    return 0


def collect_status(experiments_dir: Path) -> StatusReport:
    report = StatusReport()
    # Packages with a record, charted before the record is parsed: a package holding an
    # unreadable record has still been run, and the billed scan below would otherwise
    # label it `not yet run`.
    charted: set[str] = set()

    for package in _package_dirs(experiments_dir):
        if not (package / "results").is_dir():
            continue
        path = newest_record(package)
        if path is None:
            continue
        charted.add(package.name)
        record = load_record(path)
        if record is None:
            continue

        recorded = recorded_version(record)
        installed = resolve_installed_version(package / "probe.json")
        manifest = _parsed(package / "probe.json")
        driver = manifest.driver if manifest is not None else None

        # Only a comparable-and-different version is staleness; every other annotation
        # says why no comparison was possible.
        note = ""
        if installed.display:
            if installed.comparable:
                if installed.display != recorded:
                    if driver == Driver.BILLED:
                        # A billed package is never refreshed by a batch run, so counting
                        # its drift would make the total structurally nonzero.
                        note = (
                            f" (installed {installed.display} · refresh: just probe-run --billed)"
                        )
                    else:
                        note = f" (installed {installed.display})"
                        report.version_drift += 1
            else:
                note = f" ({installed.display})"

        report.recorded += 1
        if record.status == Status.REFUTED:
            report.refuted += 1
        if record.status == Status.INCONCLUSIVE:
            report.inconclusive += 1
        report.rows.append(
            StatusRow(
                provider=record.provider.value,
                name=package.name,
                label=_label(record.status),
                depth=record.depth.value if record.depth is not None else "—",
                date=record.date,
                version=f"{recorded}{note}",
            )
        )

    # Counted from the manifests rather than from the records, because a billed package's
    # most likely state is declared-and-never-run. A package with a record already has a row,
    # which marks an invalid manifest in its version cell.
    for manifest_path in sorted(experiments_dir.glob("*/probe.json")):
        if manifest_path.parent.name.startswith(".") or not manifest_path.exists():
            continue
        manifest = _parsed(manifest_path)
        if manifest is None:
            if manifest_path.parent.name not in charted:
                report.rows.append(
                    StatusRow(
                        provider="unknown",
                        name=manifest_path.parent.name,
                        label="manifest invalid",
                        depth="—",
                        date="—",
                        version="—",
                    )
                )
            continue
        if manifest.driver != Driver.BILLED:
            continue
        report.billed += 1
        if manifest_path.parent.name in charted:
            continue
        report.rows.append(
            StatusRow(
                provider=manifest.provider.value,
                name=manifest_path.parent.name,
                label="not yet run",
                depth=manifest.depth.value if manifest.depth is not None else "—",
                date="—",
                version="— (billed; run: just probe-run --billed)",
            )
        )
    return report


def render_summary(report: StatusReport) -> str:
    line = (
        f"probes: {report.recorded} recorded · {report.refuted} refuted · "
        f"{report.inconclusive} inconclusive · {report.version_drift} version drift"
    )
    if report.billed > 0:
        line += f" · {report.billed} billed (drift not tracked)"
    return line + "\n"


def render_report(report: StatusReport) -> str:
    # `billed` is part of the guard: a billed package whose only record is unreadable
    # emits no row and increments no count.
    if report.recorded == 0 and not report.rows and report.billed == 0:
        return "No probe records. Write a probe: see experiments/README.md\n"

    lines: list[str] = []
    # Rows sort by provider, then name, and a provider header opens each provider's rows.
    current = ""
    for row in sorted(report.rows, key=lambda r: (r.provider, r.name)):
        if row.provider != current:
            current = row.provider
            lines.append(f"\n{row.provider.upper()}\n")
        cells = (row.name, row.label, row.depth, row.date)
        padded = " ".join(
            _pad(cell, width) for cell, width in zip(cells, COLUMN_WIDTHS, strict=True)
        )
        lines.append(f"  {padded} {row.version}\n")
    return "".join(lines) + "\n" + render_summary(report)


def _package_dirs(experiments_dir: Path) -> list[Path]:
    try:
        entries = sorted(experiments_dir.iterdir())
    except OSError:
        return []
    return [e for e in entries if not e.name.startswith(".") and e.is_dir()]


def _parsed(manifest_path: Path) -> Manifest | None:
    try:
        return parse_manifest(manifest_path)
    except ManifestError:
        return None


def _label(status: Status) -> str:
    """`inconclusive` must not read as a pass: it is what an undecided operator produces."""
    return status.value if status == Status.CONFIRMED else status.value.upper()


def _pad(cell: str, width: int) -> str:
    """Left-justify by characters, so a cell holding `—` lines up with an ASCII one."""
    return cell.ljust(width)
