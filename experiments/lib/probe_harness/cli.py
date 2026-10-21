"""`experiments/probe`: the harness's single entry point.

    probe run [--billed] [--manual] [--all] [--stale]   run every authorized probe
    probe run <package>                                 run one probe
    probe record <package> <workspace> [--selection <id>] [--dry-run]
                                                        judge a kept workspace and record it
    probe status [--summary]                            report on committed records

Every harness module is imported here, at the top, so an import error surfaces before any
work starts, and never after a manual operator has spent a live session.

Exit codes: 0 success, 1 a refusal, 2 a usage error or an invalid `PROBE_*` variable. Every
diagnostic line the harness writes starts with the subcommand that wrote it (`console.py`).
"""

import argparse
import os
import signal
from pathlib import Path

from probe_harness import console, families, run_all, status

# `experiments/` is the directory above `lib/`, which holds this package.
_DEFAULT_EXPERIMENTS_DIR = Path(__file__).resolve().parents[2]

_RUN_ENVIRONMENT = "set PROBE_FIXTURE or PROBE_DRY_RUN in the environment instead"


def main(argv: list[str]) -> int:
    # A terminated run unwinds like any other failure, so every `finally` runs: no stub
    # server outlives it, and a kept workspace is still named.
    signal.signal(signal.SIGTERM, _terminated)
    parser, subparsers = _parser()
    args, extra = parser.parse_known_args(argv)
    console.set_command(f"probe {args.command}")

    match args.command:
        case "run":
            run_parser = subparsers["run"]
            flags = [f for f in ("billed", "manual", "all", "stale") if getattr(args, f)]
            if args.package is None:
                if extra:
                    run_parser.error(f"unexpected argument: {extra[0]}")
                return run_all.main(
                    billed=args.billed,
                    manual=args.manual,
                    all_drivers=args.all,
                    stale=args.stale,
                    experiments_dir=experiments_dir(),
                )
            if extra or flags:
                stray = extra[0] if extra else f"--{flags[0]}"
                run_parser.error(f"unexpected argument: {stray}; {_RUN_ENVIRONMENT}")
            return families.run_package(args.package, experiments_dir())
        case "record":
            _no_extra(subparsers["record"], extra)
            return families.record_package(
                args.package,
                Path(args.workspace),
                args.selection,
                dry_run=args.dry_run,
                experiments_dir=experiments_dir(),
            )
        case "status":
            _no_extra(subparsers["status"], extra)
            return status.main(summary=args.summary, experiments_dir=experiments_dir())
        case _:
            raise AssertionError(args.command)


def experiments_dir() -> Path:
    """`PROBE_EXPERIMENTS_DIR` when set and non-empty, otherwise the harness's own tree."""
    configured = os.environ.get("PROBE_EXPERIMENTS_DIR", "")
    return (
        Path(os.path.normpath(Path(configured).absolute()))
        if configured
        else _DEFAULT_EXPERIMENTS_DIR
    )


def _parser() -> tuple[argparse.ArgumentParser, dict[str, argparse.ArgumentParser]]:
    parser = argparse.ArgumentParser(prog="probe", allow_abbrev=False)
    commands = parser.add_subparsers(dest="command", required=True)

    run = commands.add_parser("run", prog="probe run", allow_abbrev=False)
    run.add_argument("package", nargs="?")
    run.add_argument("--billed", action="store_true")
    run.add_argument("--manual", action="store_true")
    run.add_argument("--all", action="store_true")
    run.add_argument("--stale", action="store_true")

    record_parser = commands.add_parser("record", prog="probe record", allow_abbrev=False)
    record_parser.add_argument("package")
    record_parser.add_argument("workspace")
    record_parser.add_argument("--selection")
    record_parser.add_argument("--dry-run", action="store_true")

    status_parser = commands.add_parser("status", prog="probe status", allow_abbrev=False)
    status_parser.add_argument("--summary", action="store_true")

    return parser, {
        "run": run,
        "record": record_parser,
        "status": status_parser,
    }


def _no_extra(parser: argparse.ArgumentParser, extra: list[str]) -> None:
    if extra:
        parser.error(f"unrecognized arguments: {' '.join(extra)}")


def _terminated(signum: int, frame: object) -> None:
    raise SystemExit(128 + signum)
