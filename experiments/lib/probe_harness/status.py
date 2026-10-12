"""A report derived from the committed records. Invokes no probe.

Everything printed comes from `experiments/*/results/*.json` plus the `version_source` each
package's own manifest declares. There is no list of providers and no list of renderings
that ought to be verified: the report describes what was measured, and a package with no
records has no row.

The one exception is a `billed` package, which gets a `not yet run` row from its manifest.
No batch run will ever populate it, so absence from the report would be its permanent
resting state rather than a transient one.

Always exits 0. This hangs off `just check`, and a check that can fail the build gets
muted within a week.
"""

import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from probe_harness.contract import jq_raw
from probe_harness.state import (
    load_record,
    manifest_field,
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


def main(argv: list[str], experiments_dir: Path) -> int:
    summary_only = False
    if argv and argv[0] == "--summary":
        summary_only = True
    elif argv and argv[0] != "":
        print(f"probe-status: unknown argument: {argv[0]}", file=sys.stderr)
        return 0

    report = collect_status(experiments_dir)
    if summary_only:
        sys.stdout.write(render_summary(report))
    else:
        sys.stdout.write(render_report(report))
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
        driver = manifest_field(package / "probe.json", "driver", "")

        # Only a comparable-and-different version is staleness; every other annotation
        # says why no comparison was possible.
        note = ""
        if installed.display:
            if installed.comparable:
                if installed.display != recorded:
                    if driver == "billed":
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

        status = _record_field(record, "status", "unknown")
        report.recorded += 1
        if status == "refuted":
            report.refuted += 1
        if status == "inconclusive":
            report.inconclusive += 1
        report.rows.append(
            StatusRow(
                provider=_record_field(record, "provider", "unknown"),
                name=package.name,
                label=_label(status),
                depth=_dash_if_null(record, "depth"),
                date=_record_field(record, "date", "—"),
                version=f"{recorded}{note}",
            )
        )

    # Counted from the manifests rather than from the records, because a billed package's
    # most likely state is declared-and-never-run.
    for manifest in sorted(experiments_dir.glob("*/probe.json")):
        if manifest.parent.name.startswith(".") or not manifest.exists():
            continue
        if manifest_field(manifest, "driver", "") != "billed":
            continue
        report.billed += 1
        if manifest.parent.name in charted:
            continue
        root = load_record(manifest)
        report.rows.append(
            StatusRow(
                provider=manifest_field(manifest, "provider", "unknown"),
                name=manifest.parent.name,
                label="not yet run",
                depth=_dash_if_null(root, "depth"),
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
    # Starts empty, as awk's unset variable did, so a row with no provider opens no header.
    current = ""
    # Ordered as `sort` ordered the tab-joined rows, by codepoint.
    for row in sorted(
        report.rows,
        key=lambda r: "\t".join((r.provider, r.name, r.label, r.depth, r.date, r.version)),
    ):
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


def _record_field(record: Any, key: str, default: str) -> str:
    """`jq -r '.<key> // "<default>"'` over a record, empty where jq would error."""
    if not isinstance(record, dict):
        return ""
    value = record.get(key)
    return default if value is None or value is False else jq_raw(value)


def _dash_if_null(document: Any, key: str) -> str:
    """`if .<key> == null then "—" else .<key> end`, empty where jq would error."""
    if document is None:
        return "—"
    if not isinstance(document, dict):
        return ""
    value = document.get(key)
    return "—" if value is None else jq_raw(value)


def _label(status: str) -> str:
    """`inconclusive` must not read as a pass: it is what an undecided operator produces."""
    return {"confirmed": "confirmed", "refuted": "REFUTED", "inconclusive": "INCONCLUSIVE"}.get(
        status, status
    )


def _pad(cell: str, width: int) -> str:
    """Left-justify by UTF-8 bytes, as the macOS awk that rendered this report did."""
    return cell + " " * max(0, width - len(cell.encode("utf-8")))
