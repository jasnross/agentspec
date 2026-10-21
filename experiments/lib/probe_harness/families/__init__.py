"""Family runners and the entry points that dispatch a package's definition to one.

`<package>/probe.py` defines `PROBE`, one member of `ProbeDefinition`. A new probe in an
existing family writes only what is specific to it; a probe fitting no family writes a
`CustomProbe`.
"""

import sys
from pathlib import Path
from typing import assert_never

from probe_harness import console, jq_cli, record
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
    UsageError,
    kept_for_inspection,
    kept_on_failure,
    record_result,
    record_tool_version,
    refused,
)
from probe_harness.families.claude_otel import ClaudeOtelProbe
from probe_harness.families.manual import ManualProbe
from probe_harness.families.opencode_debug import OpenCodeDebugProbe
from probe_harness.families.opencode_fake import OpenCodeFakeProbe
from probe_harness.loader import DefinitionError, load_definition
from probe_harness.preflight import (
    ASSERTION_VARIANT,
    DISAGREEING_FIXTURE,
    PreflightError,
    check_manifest,
    dry_run_from_environment,
)
from probe_harness.preflight import run as run_preflight
from probe_harness.source import write_source
from probe_harness.workspace import create_workspace, read_workspace_run, require_tools

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


def run_package(name: str, experiments_dir: Path) -> int:
    """`probe run <package>`: one blocking run of one package. Returns the exit code."""
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
        console.say(str(e))
        return 1
    except PreflightError as e:
        console.say_refusal(e)
        return e.code
    except GateFailure as e:
        console.say_refusal(e)
        return 1

    try:
        match definition:
            case ManualProbe():
                wait_for = manifest.wait_for
                if wait_for is None:
                    # A missing filter would poll a literal that never matches, burning the
                    # whole timeout of a live session for no reason.
                    console.say("manifest declares no wait_for filter")
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
    except UsageError as e:
        # Defensive: a live run judges interactively, so no family raises this today.
        console.say(str(e))
        return 2


def record_package(
    name: str, ws: Path, selection: str | None, *, dry_run: bool, experiments_dir: Path
) -> int:
    """`probe record <package> <workspace> [--selection <id>] [--dry-run]`.

    Judges a kept workspace through the family's own judge, the one `probe run` uses, so no
    record is written over a workspace its gates refuse; then records it, or under
    `--dry-run` prints what it would record. The definition's preflight function is not
    run: it checks arranging conditions, not the capture. The judge may write `view.json`
    into the workspace, as a live run's does.
    """
    package = experiments_dir / name
    try:
        # `PROBE_DRY_RUN=1` means "write nothing" here as it does for `probe run`.
        dry_run = dry_run or dry_run_from_environment()
        definition: ProbeDefinition = load_definition(package, DEFINITION_TYPES)
        # `check_manifest` runs jq without checking that it exists.
        require_tools("jq")
        manifest = check_manifest(package)
        refusal = _definition_refusal(name, definition, manifest)
        if refusal is not None:
            raise PreflightError(1, refusal)
    except DefinitionError as e:
        console.say(str(e))
        return 1
    except PreflightError as e:
        console.say_refusal(e)
        return e.code

    if not ws.is_dir():
        console.say(f"not a workspace directory: {ws}")
        return 1
    if selection is not None and not isinstance(manifest.assertion, OptionsAssertion):
        console.say(f"{name} declares a projection; --selection does not apply")
        return 2
    run = read_workspace_run(ws)
    if run is not None and run.package != name:
        console.say(f"{ws} is a workspace of {run.package}, not {name}")
        return 1
    if not dry_run:
        if run is None:
            console.say(
                f"{ws} records no package or variant (run.json); it can only be judged "
                "with --dry-run"
            )
            return 1
        if run.variant != ASSERTION_VARIANT:
            console.say(
                f"{ws} ran the {run.variant} fixture, which is dry-run only; pass --dry-run",
                DISAGREEING_FIXTURE,
            )
            return 1

    try:
        match definition:
            case ManualProbe():
                # A dry run is the re-judging loop an author repeats while editing a
                # projection, so it takes the selection as an argument instead of asking
                # every time.
                interactive = not dry_run and sys.stdin.isatty()
                result = manual.judge(manifest, ws, definition, selection, interactive=interactive)
            case OpenCodeDebugProbe():
                result = opencode_debug.judge(ws, definition)
            case OpenCodeFakeProbe():
                result = opencode_fake.judge(ws, definition)
            case ClaudeOtelProbe():
                result = claude_otel.judge(ws, definition)
            case CustomProbe():
                result = _judge_custom(ws, definition)
            case _:
                assert_never(definition)
        computed = record.compute(manifest, ws, result, dry_run=dry_run)
        record.report(computed, None if dry_run else record.write(computed))
    except UsageError as e:
        console.say(str(e))
        return 2
    except (GateFailure, record.RecordRefused) as e:
        return refused(e, ws)
    except jq_cli.JqError as e:
        # An unexpected jq failure while judging: its own diagnostic, not a traceback.
        console.verbatim(e.stderr.splitlines())
        kept_for_inspection(ws)
        return 1
    return 0


def _run_custom(manifest: Manifest, name: str, options: RunOptions, probe: CustomProbe) -> int:
    ws = create_workspace(name, options.variant)
    with kept_on_failure(ws):
        write_source(ws, options.source)
        try:
            probe.arrange(ws, options.variant)
            record_tool_version(manifest, ws)
            probe.act(ws)
            result = _judge_custom(ws, probe)
        except GateFailure as e:
            return refused(e, ws)
        return record_result(manifest, ws, result, options)


def _judge_custom(ws: Path, probe: CustomProbe) -> JudgeResult:
    """A custom probe's judge: its own, then the definition's judge function when set."""
    result = probe.judge(ws)
    if probe.functions.judge is not None:
        probe.functions.judge(ws)
    return result


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
        f"{name} is {_family(definition)}, whose only fixture variant is "
        f"{ASSERTION_VARIANT}; its definition declares: {', '.join(definition.variants)}"
    )


def _needs_capture(name: str, definition: ProbeDefinition, manifest: Manifest) -> str | None:
    # Every family but the manual one records without a capture, which these manifests need.
    if manifest.driver == Driver.MANUAL:
        return (
            f"{name} is {_family(definition)}, which records without a capture, but its "
            'manifest declares driver "manual"'
        )
    if isinstance(manifest.version_source, CaptureSource):
        return (
            f"{name} is {_family(definition)}, which records without a capture, but its "
            "manifest declares a capture version source"
        )
    return None


def _unique_arms(name: str, arms: list[str]) -> str | None:
    duplicates = sorted({arm for arm in arms if arms.count(arm) > 1})
    if duplicates:
        return f"{name}'s definition declares arm {duplicates[0]} more than once"
    return None


def _claude_gate_arms(name: str, definition: ClaudeOtelProbe) -> str | None:
    declared = {arm.name for arm in definition.arms}
    for gate in definition.gates:
        arm = getattr(gate, "arm", None)
        if arm is not None and arm not in declared:
            return (
                f"{name}'s definition gates arm {arm} ({type(gate).__name__}), but "
                "declares no such arm"
            )
    return None


def _claude_variant_trees(name: str, definition: ClaudeOtelProbe) -> str | None:
    if definition.variants == (ASSERTION_VARIANT,):
        return None
    if any(claude_otel.VARIANT in arm.tree for arm in definition.arms):
        return None
    return (
        f"{name} declares variants {', '.join(definition.variants)}, but no arm's "
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


__all__ = [
    "DEFINITION_TYPES",
    "ProbeDefinition",
    "record_package",
    "run_package",
]
