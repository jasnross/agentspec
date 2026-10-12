"""Family runners and the entry points that dispatch a package's definition to one.

`<package>/probe.py` defines `PROBE`, one member of `ProbeDefinition`. A new probe in an
existing family writes only what is specific to it; a probe fitting no family writes a
`CustomProbe`.
"""

import sys
from pathlib import Path
from typing import assert_never

from probe_harness.contract import (
    CaptureSource,
    Driver,
    Manifest,
    OptionsAssertion,
    parse_manifest,
)
from probe_harness.families import manual
from probe_harness.families.base import (
    CustomProbe,
    GateFailure,
    JudgeResult,
    RunOptions,
    kept_for_inspection,
    kept_on_failure,
    record_and_retain,
)
from probe_harness.families.manual import ManualProbe
from probe_harness.loader import DefinitionError, load_definition
from probe_harness.preflight import ASSERTION_VARIANT, PreflightError, check_manifest
from probe_harness.preflight import run as run_preflight
from probe_harness.record import record
from probe_harness.workspace import create_workspace

type ProbeDefinition = ManualProbe | CustomProbe

DEFINITION_TYPES: tuple[type, ...] = (ManualProbe, CustomProbe)

REPLAY_USAGE = "probe replay: usage: probe replay <package> <workspace> [--selection <id>]"


def run_package(name: str, extra: list[str], experiments_dir: Path) -> int:
    """`probe run <package>`: one blocking run of one package. Returns the exit code."""
    if extra:
        print(f"probe run: unexpected argument: {extra[0]}", file=sys.stderr)
        print(
            "probe: set PROBE_FIXTURE or PROBE_DRY_RUN in the environment instead.", file=sys.stderr
        )
        return 2

    package = experiments_dir / name
    try:
        definition: ProbeDefinition = load_definition(package, DEFINITION_TYPES)
        options = run_preflight(package, definition.variants, _family_tools(definition))
        refusal = _definition_refusal(name, definition, parse_manifest(package / "probe.json"))
        if refusal is not None:
            raise PreflightError(1, refusal)
        if definition.functions.preflight is not None:
            definition.functions.preflight()
    except DefinitionError as e:
        print(e, file=sys.stderr)
        return 1
    except PreflightError as e:
        _print_lines(e.lines)
        return e.code
    except GateFailure as e:
        _print_lines(e.lines)
        return 1

    match definition:
        case ManualProbe():
            wait_for = parse_manifest(package / "probe.json").wait_for
            if wait_for is None:
                # A missing filter would poll a literal that never matches, burning the
                # whole timeout of a live session for no reason.
                print("probe: manifest declares no wait_for filter", file=sys.stderr)
                return 1
            return manual.run(package, name, wait_for, options, definition)
        case CustomProbe():
            return _run_custom(package, name, options, definition)
        case _:
            assert_never(definition)


def replay_package(argv: list[str], experiments_dir: Path) -> int:
    """`probe replay <package> <workspace> [--selection <id>]`: judge a kept workspace again.

    Always a dry run; it never records. It writes `view.json` into the workspace it judges,
    as the judging step of a live run does, so replay a copy.
    """
    selection: str | None = None
    positional: list[str] = []
    rest = list(argv)
    while rest:
        arg = rest.pop(0)
        if arg == "--selection":
            if not rest:
                print(REPLAY_USAGE, file=sys.stderr)
                return 2
            selection = rest.pop(0)
        else:
            positional.append(arg)
    if len(positional) != 2:
        print(REPLAY_USAGE, file=sys.stderr)
        return 2
    name, workspace = positional
    package = experiments_dir / name
    ws = Path(workspace)

    try:
        definition: ProbeDefinition = load_definition(package, DEFINITION_TYPES)
        manifest = check_manifest(package)
    except DefinitionError as e:
        print(e, file=sys.stderr)
        return 1
    except PreflightError as e:
        _print_lines(e.lines)
        return e.code

    if selection is not None and not isinstance(manifest.assertion, OptionsAssertion):
        print(
            f"probe replay: {name} declares a projection; --selection does not apply",
            file=sys.stderr,
        )
        return 2

    try:
        match definition:
            case ManualProbe():
                if isinstance(manifest.assertion, OptionsAssertion) and selection is None:
                    print(
                        f"probe replay: {name} declares options; pass --selection <id>",
                        file=sys.stderr,
                    )
                    return 2
                if definition.functions.judge is not None:
                    definition.functions.judge(ws)
                return manual.record_capture(package, ws, dry_run=True, selection=selection)
            case CustomProbe():
                result = definition.judge(ws)
                if definition.functions.judge is not None:
                    definition.functions.judge(ws)
                return record(
                    manifest.path, view=result.view, selection=result.selection, dry_run=True
                )
            case _:
                assert_never(definition)
    except GateFailure as e:
        _print_lines(e.lines)
        return 1


def _run_custom(package: Path, name: str, options: RunOptions, probe: CustomProbe) -> int:
    ws = create_workspace(name)
    with kept_on_failure(ws):
        try:
            probe.arrange(ws, options.variant)
            probe.act(ws)
            result: JudgeResult = probe.judge(ws)
            if probe.functions.judge is not None:
                probe.functions.judge(ws)
        except GateFailure as e:
            _print_lines(e.lines)
            kept_for_inspection(ws)
            return 1
        return record_and_retain(package, ws, result, options, probe.functions)


def _definition_refusal(name: str, definition: ProbeDefinition, manifest: Manifest) -> str | None:
    """A definition and manifest that could only fail later, refused before any workspace."""
    match definition:
        case ManualProbe():
            # A manual workspace templates the whole fixtures/ tree, so it has no variant
            # trees for `PROBE_FIXTURE` to select between.
            if definition.variants != (ASSERTION_VARIANT,):
                return (
                    f"probe: {name} is a manual probe, whose only fixture variant is "
                    f"{ASSERTION_VARIANT}; its definition declares: "
                    f"{', '.join(definition.variants)}"
                )
        case CustomProbe():
            # A custom probe records without a capture, which these manifests require.
            if manifest.driver == Driver.MANUAL:
                return (
                    f"probe: {name} is a CustomProbe, which records without a capture, but "
                    'its manifest declares driver "manual"'
                )
            if isinstance(manifest.version_source, CaptureSource):
                return (
                    f"probe: {name} is a CustomProbe, which records without a capture, but "
                    "its manifest declares a capture version source"
                )
        case _:
            assert_never(definition)
    return None


def _family_tools(definition: ProbeDefinition) -> tuple[str, ...]:
    match definition:
        case ManualProbe():
            return manual.FAMILY_TOOLS
        case CustomProbe():
            return definition.required_tools
        case _:
            assert_never(definition)


def _print_lines(lines: tuple[str, ...]) -> None:
    for line in lines:
        print(line, file=sys.stderr)


__all__ = [
    "DEFINITION_TYPES",
    "ProbeDefinition",
    "replay_package",
    "run_package",
]
