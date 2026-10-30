"""`experiments/probe`: the harness's single entry point.

    probe run [--billed] [--manual] [--all] [--stale]   run every authorized probe
    probe run <package>                                 run one probe
    probe record <package> <workspace> [--selection <id>] [--dry-run]
                                                        judge a kept workspace and record it
    probe status [--summary]                            report on committed records
    probe source <record> <dir> [--arranged]            write a v2 record's files to <dir>
    probe sample <package> <workspace> <name> [--lines N]
                                                        write a sample from a kept workspace
    probe sample <package> --regenerate                 recompute every sample's results

Every harness module a subcommand needs is imported before it does any work, so an import
error surfaces before any work starts, and never after a manual operator has spent a live
session. `probe source` imports none that loads a definition: like `probe status`, it
executes no probe-authored code.

Exit codes: 0 success, 1 a refusal, 2 a usage error or an invalid `PROBE_*` variable. Every
diagnostic line the harness writes starts with the subcommand that wrote it (`console.py`).
"""

import argparse
import os
import signal
from pathlib import Path

from probe_harness import console, contract, source

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

    if args.command == "source":
        _no_extra(subparsers["source"], extra)
        return _source(Path(args.record), Path(args.dir), arranged=args.arranged)

    # Imported only here, so `probe source` never loads a module that loads a definition.
    from probe_harness import families, run_all, status

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
        case "sample":
            _no_extra(subparsers["sample"], extra)
            return _sample(subparsers["sample"], args)
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

    sample_parser = commands.add_parser("sample", prog="probe sample", allow_abbrev=False)
    sample_parser.add_argument("package")
    sample_parser.add_argument("workspace", nargs="?")
    sample_parser.add_argument("name", nargs="?")
    sample_parser.add_argument("--lines", type=_count)
    sample_parser.add_argument("--regenerate", action="store_true")

    source_parser = commands.add_parser("source", prog="probe source", allow_abbrev=False)
    source_parser.add_argument("record")
    source_parser.add_argument("dir")
    source_parser.add_argument("--arranged", action="store_true")

    return parser, {
        "run": run,
        "record": record_parser,
        "status": status_parser,
        "source": source_parser,
        "sample": sample_parser,
    }


def _sample(parser: argparse.ArgumentParser, args: argparse.Namespace) -> int:
    """`probe sample`: write one sample from a kept workspace, or regenerate every result."""
    # Imported here: `samples` loads every family, which `probe source` must not.
    from probe_harness import families, samples
    from probe_harness.families.base import UsageError
    from probe_harness.families.manual import ManualProbe
    from probe_harness.gates import GateFailure
    from probe_harness.loader import DefinitionError
    from probe_harness.preflight import PreflightError

    if args.regenerate:
        if args.workspace is not None or args.name is not None or args.lines is not None:
            parser.error("--regenerate takes only a package")
    elif args.workspace is None or args.name is None:
        parser.error("a workspace and a sample name are required without --regenerate")
    package = experiments_dir() / args.package
    try:
        definition, manifest = families.load_package(package)
        if args.lines is not None and not isinstance(definition, ManualProbe):
            parser.error("--lines applies only to a manual package")
        if args.regenerate:
            for line in samples.regenerate(definition, manifest):
                print(line)
            return 0
        workspace, name = str(args.workspace), str(args.name)
        written = samples.write_sample(
            definition, manifest, Path(workspace), name, lines=args.lines
        )
    except DefinitionError as e:
        console.say(str(e))
        return 1
    except PreflightError as e:
        console.say_refusal(e)
        return e.code
    except (GateFailure, samples.SampleRefused) as e:
        console.say_refusal(e)
        return 1
    except UsageError as e:
        console.say(str(e))
        return 2
    print(written)
    return 0


def _count(text: str) -> int:
    if not (text.isascii() and text.isdecimal()):
        raise argparse.ArgumentTypeError(f"must be a non-negative integer (got: {text})")
    return int(text)


def _source(record_path: Path, out_dir: Path, *, arranged: bool) -> int:
    """`probe source`: write the files a v2 record names, out of its repository's objects."""
    parsed = contract.parse_record(record_path)
    if parsed is None:
        console.say(f"not a readable record: {record_path}")
        return 1
    if isinstance(parsed, contract.RecordV1):
        console.say(f"{record_path} is a schema 1 record; its assertion text is in the record")
        return 1
    try:
        missing = source.materialize(parsed, record_path, out_dir, arranged=arranged)
    except FileExistsError as e:
        console.say(f"refusing to overwrite {e.args[0]}")
        return 1
    except ValueError as e:
        console.say(str(e))
        return 1
    if missing:
        console.say("these files' objects are missing from the repository:")
        for path in missing:
            print(path)
        return 1
    print(f"wrote {len(source.recorded_files(parsed, arranged=arranged))} files to {out_dir}")
    return 0


def _no_extra(parser: argparse.ArgumentParser, extra: list[str]) -> None:
    if extra:
        parser.error(f"unrecognized arguments: {' '.join(extra)}")


def _terminated(signum: int, frame: object) -> None:
    raise SystemExit(128 + signum)
