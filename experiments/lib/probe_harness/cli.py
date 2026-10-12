"""`experiments/probe`: the harness's single entry point.

    probe run [--billed] [--manual] [--all] [--stale]   run every authorized probe
    probe run <package>                                 run one probe
    probe record --manifest <probe.json> …              the record writer
    probe status [--summary]                            report on committed records
    probe replay <package> <workspace> [--selection <id>]

Every harness module is imported here, at the top, so an import error surfaces before any
work starts, and never after a manual operator has spent a live session.
"""

import os
import signal
import sys
from pathlib import Path

from probe_harness import families, record, run_all, status
from probe_harness.gates import claude_otel  # noqa: F401 - imported so it fails early

# `experiments/` is the directory above `lib/`, which holds this package.
_DEFAULT_EXPERIMENTS_DIR = Path(__file__).resolve().parents[2]


def main(argv: list[str]) -> int:
    # A terminated run unwinds like any other failure, so every `finally` runs: no stub
    # server outlives it, and a kept workspace is still named.
    signal.signal(signal.SIGTERM, _terminated)
    if not argv:
        print("probe: usage: probe <run|record|status|replay> …", file=sys.stderr)
        return 2
    command, rest = argv[0], argv[1:]
    match command:
        case "run":
            if not rest or rest[0].startswith("-"):
                return run_all.main(rest, experiments_dir())
            return families.run_package(rest[0], rest[1:], experiments_dir())
        case "record":
            return record.main(rest)
        case "status":
            return status.main(rest, experiments_dir())
        case "replay":
            return families.replay_package(rest, experiments_dir())
        case _:
            print(f"probe: unknown subcommand: {command}", file=sys.stderr)
            return 2


def _terminated(signum: int, frame: object) -> None:
    raise SystemExit(128 + signum)


def experiments_dir() -> Path:
    """`PROBE_EXPERIMENTS_DIR` when set and non-empty, otherwise the harness's own tree."""
    configured = os.environ.get("PROBE_EXPERIMENTS_DIR", "")
    return (
        Path(os.path.normpath(Path(configured).absolute()))
        if configured
        else _DEFAULT_EXPERIMENTS_DIR
    )
