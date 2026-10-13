"""Run the probes an invocation has authorized, and record each.

Every package is runnable here, including the `manual` ones, which block through a live
provider session. What separates them is cost: an `unattended` probe costs nothing, a
`billed` one spends model quota, and a `manual` one spends a person's afternoon. So a run
is defined by what it is willing to spend.

Two orthogonal questions decide each package, in this order:

    selection      is this package interesting to this run?  (`--stale`)
    authorization  may this run pay what the package costs?  (`--billed`, `--manual`, `--all`)

Authorization is a set of drivers, seeded with the free one and added to by each flag, so
the flags stack in any order. Selection knows nothing about drivers; keeping them separate
is what lets `--stale --billed` mean the obvious thing.

Probes execute sequentially, each in its own `probe run <package>` subprocess, so one
probe's failure cannot abort the loop.
"""

import os
import subprocess
import sys
from pathlib import Path

from probe_harness import console
from probe_harness.contract import DRIVERS, FREE_DRIVER, Driver, ManifestError, parse_manifest
from probe_harness.state import needs_run

# The CLI this loop runs each package through: the `experiments/probe` beside `lib/`.
PROBE_SCRIPT = Path(__file__).resolve().parents[2] / "probe"


def main(
    *, billed: bool, manual: bool, all_drivers: bool, stale: bool, experiments_dir: Path
) -> int:
    known = {d.value for d in DRIVERS}
    authorized: list[str] = [FREE_DRIVER.value]

    def authorize(driver: str) -> None:
        if driver not in authorized:
            authorized.append(driver)

    # The environment form exists so a caller that cannot pass arguments can still opt in.
    # It seeds the same set the flags add to.
    for driver in os.environ.get("PROBE_AUTHORIZE_DRIVERS", "").split():
        if driver not in known:
            console.say(f"PROBE_AUTHORIZE_DRIVERS names an unknown driver: {driver}")
            return 2
        authorize(driver)

    if billed:
        authorize(Driver.BILLED.value)
    if manual:
        authorize(Driver.MANUAL.value)
    if all_drivers:
        authorized = [d.value for d in DRIVERS]
    stale_only = stale

    ran = 0
    fresh = 0
    # One entry per withheld package, so the summary counts by driver.
    withheld: list[str] = []
    failed: list[str] = []

    for manifest in sorted(experiments_dir.glob("*/probe.json")):
        if manifest.parent.name.startswith(".") or not manifest.exists():
            continue
        package = manifest.parent
        name = package.name

        try:
            driver = parse_manifest(manifest).driver.value
        except ManifestError as e:
            console.say(f"{name}: {e}")
            failed.append(name)
            continue

        # Selection before authorization, so a fresh package is passed over whatever it
        # would have cost.
        if stale_only:
            owed, reason = needs_run(package)
            if not owed:
                _out(f"fresh    {name:<32} {reason}\n")
                fresh += 1
                continue

        if driver in authorized:
            _out(f"\n=== {name} ===\n")
            # A failing probe must not abort the loop: records written by probes that
            # passed stay valid, and the run reports what broke at the end.
            argv = [sys.executable, str(PROBE_SCRIPT), "run", name]
            if subprocess.run(argv, check=False).returncode == 0:
                ran += 1
            else:
                console.say(f"{name} FAILED")
                failed.append(name)
        else:
            _out(f"skipped  {name:<32} driver {driver} — {_withheld_note(driver, name)}\n")
            withheld.append(driver)

    # One segment per withheld driver, generated from the enum. `unattended` is skipped: it
    # is always authorized, so a segment for it would be a structural zero.
    segments = [f"{withheld.count(d.value)} need --{d.value}" for d in DRIVERS if d != FREE_DRIVER]
    withheld_total = len(withheld)
    # `fresh` appears only under `--stale`: printing `0 fresh` on a run that never
    # evaluated freshness would claim it had been evaluated and found nothing.
    if stale_only:
        segments.append(f"{fresh} fresh")
    _out(f"\nprobe run: {ran} ran · {withheld_total + fresh} skipped ({' · '.join(segments)})\n")

    if failed:
        console.say(f"failed: {' '.join(failed)}")
        return 1
    return 0


def _withheld_note(driver: str, name: str) -> str:
    """What a withheld package costs, phrased as the flag to type."""
    match driver:
        case Driver.BILLED:
            return "spends model quota; authorize with --billed"
        case Driver.MANUAL:
            return f"needs a live session; authorize with --manual, following {name}/README.md"
        case _:
            return "authorize with --all"


def _out(text: str) -> None:
    # Flushed so it interleaves correctly with the output of each probe subprocess.
    sys.stdout.write(text)
    sys.stdout.flush()
