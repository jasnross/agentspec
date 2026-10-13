"""Family runners and the entry points that dispatch a package's definition to one.

`<package>/probe.py` defines `PROBE`, one member of `ProbeDefinition`. A new probe in an
existing family writes only what is specific to it; a probe fitting no family writes a
`CustomProbe`.
"""

import sys
from pathlib import Path
from typing import assert_never

from probe_harness import jq_cli, record
from probe_harness.contract import (
    CaptureSource,
    Driver,
    Manifest,
    OptionsAssertion,
)
from probe_harness.families import claude_otel, manual, opencode_debug, opencode_fake
from probe_harness.families.base import (
    CustomProbe,
    GateFailure,
    JudgeResult,
    RunOptions,
    kept_on_failure,
    record_and_retain,
    record_refused,
    refused,
)
from probe_harness.families.claude_otel import ClaudeOtelProbe
from probe_harness.families.manual import ManualProbe
from probe_harness.families.opencode_debug import OpenCodeDebugProbe
from probe_harness.families.opencode_fake import OpenCodeFakeProbe
from probe_harness.loader import DefinitionError, load_definition
from probe_harness.preflight import ASSERTION_VARIANT, PreflightError, check_manifest
from probe_harness.preflight import run as run_preflight
from probe_harness.workspace import create_workspace

type ProbeDefinition = (
    ManualProbe | OpenCodeDebugProbe | OpenCodeFakeProbe | ClaudeOtelProbe | CustomProbe
)

DEFINITION_TYPES: tuple[type, ...] = (
    ManualProbe,
    OpenCodeDebugProbe,
    OpenCodeFakeProbe,
    ClaudeOtelProbe,
    CustomProbe,
)

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
        manifest, options = run_preflight(package, definition.variants, _family_tools(definition))
        refusal = _definition_refusal(name, definition, manifest)
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
            wait_for = manifest.wait_for
            if wait_for is None:
                # A missing filter would poll a literal that never matches, burning the
                # whole timeout of a live session for no reason.
                print("probe: manifest declares no wait_for filter", file=sys.stderr)
                return 1
            return manual.run(manifest, name, wait_for, options, definition)
        case OpenCodeDebugProbe():
            return opencode_debug.run(manifest, name, options, definition)
        case OpenCodeFakeProbe():
            return opencode_fake.run(manifest, name, options, definition)
        case ClaudeOtelProbe():
            return claude_otel.run(manifest, name, options, definition)
        case CustomProbe():
            return _run_custom(manifest, name, options, definition)
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
                return manual.record_capture(manifest, ws, dry_run=True, selection=selection)
            case OpenCodeDebugProbe():
                result = opencode_debug.judge(ws, definition)
            case OpenCodeFakeProbe():
                result = opencode_fake.judge(ws, definition)
            case ClaudeOtelProbe():
                result = claude_otel.judge(ws, definition)
            case CustomProbe():
                result = definition.judge(ws)
                if definition.functions.judge is not None:
                    definition.functions.judge(ws)
            case _:
                assert_never(definition)
        computed = record.compute(manifest, view=result.view, selection=result.selection)
        record.report(computed, None)
    except record.RecordRefused as e:
        record_refused(e)
        return 1
    except GateFailure as e:
        _print_lines(e.lines)
        return 1
    except jq_cli.JqError as e:
        # An unexpected jq failure while judging: its own diagnostic, not a traceback.
        sys.stderr.write(e.stderr)
        return 1
    return 0


def _run_custom(manifest: Manifest, name: str, options: RunOptions, probe: CustomProbe) -> int:
    ws = create_workspace(name)
    with kept_on_failure(ws):
        try:
            probe.arrange(ws, options.variant)
            probe.act(ws)
            result: JudgeResult = probe.judge(ws)
            if probe.functions.judge is not None:
                probe.functions.judge(ws)
        except GateFailure as e:
            return refused(e, ws)
        return record_and_retain(manifest, ws, result, options, probe.functions)


def _definition_refusal(name: str, definition: ProbeDefinition, manifest: Manifest) -> str | None:
    """A definition and manifest that could only fail later, refused before any workspace.

    For the billed family that matters most: each of these would otherwise surface only after
    every arm had spent a model call.
    """
    match definition:
        case ManualProbe():
            return _single_variant(name, definition)
        case OpenCodeFakeProbe():
            return (
                _single_variant(name, definition)
                or _unique_arms(name, [arm.name for arm in definition.arms])
                or _needs_capture(name, definition, manifest)
            )
        case ClaudeOtelProbe():
            return (
                _unique_arms(name, [arm.name for arm in definition.arms])
                or _claude_gate_arms(name, definition)
                or _claude_variant_trees(name, definition)
                or _needs_capture(name, definition, manifest)
            )
        case OpenCodeDebugProbe() | CustomProbe():
            return _needs_capture(name, definition, manifest)
        case _:
            assert_never(definition)


def _family(definition: ProbeDefinition) -> str:
    family = type(definition).__name__
    return f"{'an' if family[0] in 'AEIOU' else 'a'} {family}"


def _single_variant(name: str, definition: ManualProbe | OpenCodeFakeProbe) -> str | None:
    # These families materialize one fixed tree (all of fixtures/, or fixtures/project/), so
    # they have no variant trees for `PROBE_FIXTURE` to select between.
    if definition.variants == (ASSERTION_VARIANT,):
        return None
    return (
        f"probe: {name} is {_family(definition)}, whose only fixture variant is "
        f"{ASSERTION_VARIANT}; its definition declares: {', '.join(definition.variants)}"
    )


def _needs_capture(name: str, definition: ProbeDefinition, manifest: Manifest) -> str | None:
    # Every family but the manual one records without a capture, which these manifests need.
    if manifest.driver == Driver.MANUAL:
        return (
            f"probe: {name} is {_family(definition)}, which records without a capture, but its "
            'manifest declares driver "manual"'
        )
    if isinstance(manifest.version_source, CaptureSource):
        return (
            f"probe: {name} is {_family(definition)}, which records without a capture, but its "
            "manifest declares a capture version source"
        )
    return None


def _unique_arms(name: str, arms: list[str]) -> str | None:
    duplicates = sorted({arm for arm in arms if arms.count(arm) > 1})
    if duplicates:
        return f"probe: {name}'s definition declares arm {duplicates[0]} more than once"
    return None


def _claude_gate_arms(name: str, definition: ClaudeOtelProbe) -> str | None:
    declared = {arm.name for arm in definition.arms}
    for gate in definition.gates:
        arm = getattr(gate, "arm", None)
        if arm is not None and arm not in declared:
            return (
                f"probe: {name}'s definition gates arm {arm} ({type(gate).__name__}), but "
                "declares no such arm"
            )
    return None


def _claude_variant_trees(name: str, definition: ClaudeOtelProbe) -> str | None:
    if definition.variants == (ASSERTION_VARIANT,):
        return None
    if any(claude_otel.VARIANT in arm.tree for arm in definition.arms):
        return None
    return (
        f"probe: {name} declares variants {', '.join(definition.variants)}, but no arm's "
        f"fixture tree contains {claude_otel.VARIANT}, so every variant would run the same tree"
    )


def _family_tools(definition: ProbeDefinition) -> tuple[str, ...]:
    match definition:
        case ManualProbe():
            return manual.FAMILY_TOOLS
        case OpenCodeDebugProbe():
            return opencode_debug.FAMILY_TOOLS
        case OpenCodeFakeProbe():
            return opencode_fake.FAMILY_TOOLS
        case ClaudeOtelProbe():
            return claude_otel.FAMILY_TOOLS
        case CustomProbe():
            # The recorder runs jq whatever the definition itself needs.
            return tuple(dict.fromkeys(("jq", *definition.required_tools)))
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
