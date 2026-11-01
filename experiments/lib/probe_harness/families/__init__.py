"""Family runners and the entry points that dispatch a package's definition to one.

`<package>/probe.py` defines `PROBE`, one member of `ProbeDefinition`. A new probe in an
existing family writes only what is specific to it; a probe fitting no family writes a
`CustomProbe`.
"""

import sys
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any, assert_never

from probe_harness import console, record
from probe_harness.contract import (
    CaptureSource,
    Driver,
    ExpectedAssertion,
    Manifest,
    OptionsAssertion,
)
from probe_harness.families import claude_init, claude_otel, manual, opencode_debug, opencode_fake
from probe_harness.families.base import (
    CustomCheck,
    CustomProbe,
    GateFailure,
    JudgeResult,
    RunOptions,
    UsageError,
    kept_on_failure,
    observe,
    record_result,
    record_tool_version,
    refused,
    source_changed,
)
from probe_harness.families.claude_init import ClaudeInitArm, ClaudeInitProbe
from probe_harness.families.claude_otel import ClaudeOtelProbe, McpConfig, PluginDir
from probe_harness.families.manual import ManualProbe
from probe_harness.families.opencode_debug import OpenCodeDebugProbe
from probe_harness.families.opencode_fake import OpenCodeFakeProbe
from probe_harness.json_values import JsonValue
from probe_harness.loader import DefinitionError, load_definition
from probe_harness.preflight import (
    ASSERTION_VARIANT,
    DISAGREEING_FIXTURE,
    PreflightError,
    check_manifest,
    dry_run_from_environment,
)
from probe_harness.preflight import run as run_preflight
from probe_harness.source import (
    SourceMapError,
    clean_tree_refusal,
    read_source,
    source_map,
    write_source,
)
from probe_harness.workspace import create_workspace, read_workspace_run, require_tools

type ProbeDefinition = (
    ManualProbe[Any]
    | OpenCodeDebugProbe[Any]
    | OpenCodeFakeProbe
    | ClaudeOtelProbe
    | ClaudeInitProbe
    | CustomProbe[Any]
)

DEFINITION_TYPES: tuple[type, ...] = (
    ManualProbe,
    OpenCodeDebugProbe,
    OpenCodeFakeProbe,
    ClaudeOtelProbe,
    ClaudeInitProbe,
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
                return manual.run(manifest, name, options, definition)
            case OpenCodeDebugProbe():
                return opencode_debug.run(manifest, name, options, definition)
            case OpenCodeFakeProbe():
                return opencode_fake.run(manifest, name, options, definition)
            case ClaudeOtelProbe():
                return claude_otel.run(manifest, name, options, definition)
            case ClaudeInitProbe():
                return claude_init.run(manifest, name, options, definition)
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
        manifest = check_manifest(package)
        refusal = _definition_refusal(name, definition, manifest)
        if refusal is not None:
            raise PreflightError(1, refusal)
        # A record names the code that judged it. Computed before judging, and for a real
        # recording checked against `HEAD` then, and against the files again before writing.
        require_tools("git")
        source: Mapping[str, str] = source_map(package)
        gate = clean_tree_refusal(source, package) if not dry_run else None
        if gate is not None:
            raise PreflightError(1, *gate)
    except SourceMapError as e:
        console.say(*e.lines)
        return 1
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
        console.say(f"{name} declares no options; --selection does not apply")
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
    # A record without `arranged` claims the judging code arranged the workspace, so a real
    # one needs the map the arranging run wrote to know whether that is so.
    arranged = read_source(ws)
    if arranged is None and not dry_run:
        console.say(
            f"{ws / 'source.json'} is missing or is not a source map, so a record could not "
            "say which code arranged the workspace; it can only be judged with --dry-run"
        )
        return 1

    try:
        match definition:
            case ManualProbe():
                # A dry run is the re-judging loop an author repeats while editing an
                # assertion, so it takes the selection as an argument instead of asking
                # every time.
                interactive = not dry_run and sys.stdin.isatty()
                result = manual.judge(manifest, ws, definition, selection, interactive=interactive)
            case OpenCodeDebugProbe():
                result = opencode_debug.judge(ws, definition)
            case OpenCodeFakeProbe():
                result = opencode_fake.judge(ws, definition)
            case ClaudeOtelProbe():
                result = claude_otel.judge(ws, definition)
            case ClaudeInitProbe():
                result = claude_init.judge(ws, definition)
            case CustomProbe():
                result = _judge_custom(ws, definition)
            case _:
                assert_never(definition)
        computed = record.compute(
            manifest,
            ws,
            result,
            dry_run=dry_run,
            source=source,
            arranged=arranged if arranged is not None and arranged != source else None,
        )
        if not dry_run:
            changed = source_changed(source, package)
            if changed is not None:
                raise changed
        record.report(computed, None if dry_run else record.write(computed))
    except UsageError as e:
        console.say(str(e))
        return 2
    except (GateFailure, record.RecordRefused) as e:
        return refused(e, ws)
    return 0


def _run_custom(manifest: Manifest, name: str, options: RunOptions, probe: CustomProbe[Any]) -> int:
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


def _judge_custom(ws: Path, probe: CustomProbe[Any]) -> JudgeResult:
    """A custom probe's judge: its own, the definition's judge function, then the outcome."""
    value = probe.judge(ws)
    if probe.functions.judge is not None:
        probe.functions.judge(value)
    # The agreement check guarantees an assertion for a manifest declaring `expected`.
    assert probe.assertion is not None
    return JudgeResult(observe(probe.assertion, value))


def load_package(package: Path) -> tuple[ProbeDefinition, Manifest]:
    """<package>'s definition and manifest, refused unless they agree.

    Raises `DefinitionError` or `PreflightError`, as `probe record` would refuse them.
    """
    definition: ProbeDefinition = load_definition(package, DEFINITION_TYPES)
    manifest = check_manifest(package)
    refusal = _definition_refusal(package.name, definition, manifest)
    if refusal is not None:
        raise PreflightError(1, refusal)
    return definition, manifest


def parse_workspace(definition: ProbeDefinition, ws: Path, lines: int | None = None) -> Any:
    """<ws> read through the definition's family, as its judge reads it before any gate.

    <lines> reads only that many capture lines, which only the manual family has.
    """
    match definition:
        case ManualProbe():
            return manual.parse(ws, definition, lines=lines)
        case OpenCodeDebugProbe():
            return opencode_debug.parse(ws, definition)
        case OpenCodeFakeProbe():
            return opencode_fake.parse(ws, definition)
        case ClaudeOtelProbe():
            return claude_otel.parse(ws, definition)
        case ClaudeInitProbe():
            return claude_init.parse(ws, definition)
        case CustomProbe():
            raise UsageError(f"{ws}: a CustomProbe has no sample format")
        case _:
            assert_never(definition)


def parse_sample(definition: ProbeDefinition, document: JsonValue, label: str) -> Any:
    """A sample's input read through the definition's family, as `parse_workspace` reads a
    workspace. A custom probe has no sample format."""
    match definition:
        case ManualProbe():
            return manual.parse_sample(document, label, definition)
        case OpenCodeDebugProbe():
            return opencode_debug.parse_sample(document, label, definition)
        case OpenCodeFakeProbe():
            return opencode_fake.parse_sample(document, label, definition)
        case ClaudeOtelProbe():
            return claude_otel.parse_sample(document, label, definition)
        case ClaudeInitProbe():
            return claude_init.parse_sample(document, label, definition)
        case CustomProbe():
            raise UsageError(f"{label}: a CustomProbe has no sample format")
        case _:
            assert_never(definition)


@dataclass(frozen=True)
class Expressions:
    """The Python expressions a definition carries, whatever its family."""

    assertion: Callable[..., Any] | None
    custom_checks: tuple[CustomCheck[Any], ...] = ()
    capture_version: Callable[..., Any] | None = None
    # Whether the family polls a capture, and so takes a `wait_for`.
    manual: bool = False
    wait_for: Callable[..., Any] | None = None


def expressions(definition: ProbeDefinition) -> Expressions:
    match definition:
        case ManualProbe():
            return Expressions(
                definition.assertion,
                capture_version=definition.capture_version,
                manual=True,
                wait_for=definition.wait_for,
            )
        case OpenCodeDebugProbe() | OpenCodeFakeProbe() | ClaudeOtelProbe() | ClaudeInitProbe():
            return Expressions(definition.assertion, definition.custom_checks)
        case CustomProbe():
            return Expressions(definition.assertion)
        case _:
            assert_never(definition)


def _definition_refusal(name: str, definition: ProbeDefinition, manifest: Manifest) -> str | None:
    """A definition and manifest that could only fail later, refused before any workspace.

    For the billed family that matters most: each of these would otherwise surface only after
    every arm had spent a model call.
    """
    package = manifest.path.parent
    agreement = _agreement(name, definition, manifest)
    if agreement is not None:
        return agreement
    match definition:
        case ManualProbe():
            return _capture_is_not_a_variant(name, definition) or _variant_trees(
                name, definition, package
            )
        case OpenCodeFakeProbe():
            return (
                _variant_trees(name, definition, package)
                or _unique_arms(name, [arm.name for arm in definition.arms])
                or _needs_capture(name, definition, manifest)
            )
        case ClaudeOtelProbe():
            return (
                _variant_trees(name, definition, package)
                or _unique_arms(name, [arm.name for arm in definition.arms])
                or _claude_gate_arms(name, definition)
                or _needs_capture(name, definition, manifest)
                or _claude_variant_inputs(name, definition, package)
            )
        case ClaudeInitProbe():
            return (
                _variant_trees(name, definition, package)
                or _unique_arms(name, [arm.name for arm in definition.arms])
                or _needs_capture(name, definition, manifest)
                or _claude_variant_inputs(name, definition, package)
            )
        case OpenCodeDebugProbe() | CustomProbe():
            return _variant_trees(name, definition, package) or _needs_capture(
                name, definition, manifest
            )
        case _:
            assert_never(definition)


def _agreement(name: str, definition: ProbeDefinition, manifest: Manifest) -> str | None:
    """Whether the manifest and the definition's expressions agree.

    A manifest declaring `expected` needs an assertion to compare with it, and an option
    set, which a person answers, refuses one. A `capture` version source is read by the
    definition's `capture_version`, which only that source may declare. Custom check names
    are non-empty and unique, since a sample keys each check's result by its name.
    """
    found = expressions(definition)
    family = _family(definition)
    names = [c.name for c in found.custom_checks]
    if "" in names:
        return f"{name}'s definition declares a custom check with an empty name"
    duplicates = sorted({n for n in names if names.count(n) > 1})
    if duplicates:
        return f"{name}'s definition declares custom check {duplicates[0]} more than once"
    if isinstance(manifest.assertion, ExpectedAssertion) and found.assertion is None:
        return (
            f"{name}'s manifest declares expected, but {family} definition carries no "
            "assertion to compare with it"
        )
    if isinstance(manifest.assertion, OptionsAssertion) and found.assertion is not None:
        return (
            f"{name}'s manifest declares options, which a person answers, but its definition "
            "carries an assertion"
        )
    capture = isinstance(manifest.version_source, CaptureSource)
    if capture and found.manual and found.capture_version is None:
        return (
            f"{name}'s manifest declares a capture version source, but its definition "
            "carries no capture_version to read it"
        )
    if not capture and found.capture_version is not None:
        return (
            f"{name}'s definition carries a capture_version, but its manifest's version "
            "source is not a capture"
        )
    return None


def _family(definition: ProbeDefinition) -> str:
    family = type(definition).__name__
    return f"{'an' if family[0] in 'AEIOU' else 'a'} {family}"


def _variant_trees(name: str, definition: ProbeDefinition, package: Path) -> str | None:
    # Every family arranges from `fixtures/<variant>/`, so a declared variant without one
    # could only fail once its run had begun.
    for variant in definition.variants:
        if not (package / "fixtures" / variant).is_dir():
            return f"{name} declares fixture variant {variant}, but has no fixtures/{variant}/"
    return None


def _capture_is_not_a_variant(name: str, definition: ManualProbe[Any]) -> str | None:
    # `fixtures/capture/` is the shared apparatus; a variant of that name would copy it into
    # the project the operator opens, where the provider could read it.
    if manual.CAPTURE_DIR in definition.variants:
        return (
            f"{name} declares fixture variant {manual.CAPTURE_DIR}, the name a manual "
            "probe's capture apparatus reserves"
        )
    return None


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


def _claude_variant_inputs(
    name: str, definition: ClaudeOtelProbe | ClaudeInitProbe, package: Path
) -> str | None:
    # Each variant tree carries its own copy of every input an arm reads, so one a variant
    # lacks would fail only at that arm, after the arms before it had spent model calls.
    for variant in definition.variants:
        fixtures = package / "fixtures" / variant
        for arm in definition.arms:
            match arm.server:
                case McpConfig(template=template):
                    missing = None if (fixtures / template).is_file() else template
                case PluginDir(source=source):
                    missing = None if (fixtures / source / ".mcp.json").is_file() else source
                case None:
                    missing = None
                case _:
                    assert_never(arm.server)
            home = arm.home if isinstance(arm, ClaudeInitArm) else None
            if home is not None and not (fixtures / home.template).is_file():
                missing = home.template
            if not (fixtures / arm.tree).is_dir():
                missing = arm.tree
            if missing is not None:
                return (
                    f"{name}'s arm {arm.name} reads fixtures/{variant}/{missing}, which does "
                    "not exist"
                )
    return None


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
        case ClaudeInitProbe():
            return claude_init.FAMILY_TOOLS
        case CustomProbe():
            return definition.required_tools
        case _:
            assert_never(definition)


__all__ = [
    "DEFINITION_TYPES",
    "Expressions",
    "ProbeDefinition",
    "expressions",
    "load_package",
    "parse_sample",
    "parse_workspace",
    "record_package",
    "run_package",
]
