"""The Claude OTEL family: billed `claude -p` runs whose request bodies are the oracle.

Each arm copies a fixture tree into its own project and runs `claude -p` there with
OpenTelemetry raw-body logging pointed at the arm's sink. Every arm spends a billed model
call, which is why these packages declare `driver: billed`.

`--setting-sources project` excludes the user settings tier outright rather than
out-ranking it, and `CLAUDE_CODE_EFFORT_LEVEL` is removed because it outranks frontmatter:
an exported one would make a probe measure the operator's shell.

Workspace layout, per arm: `<ws>/<arm>/{project, sink, stdout, stderr, mcp.json | fxp/,
fx.stamp}`, plus `<ws>/view.json`.

Judging is `parse`, which reads every arm's sink into models, then the gates, the custom
checks and the definition's judge function, then the outcome. The expressions see the arms
`expression_input` gives them: responses only when the definition requires them.
"""

import os
import shutil
import subprocess
import sys
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import assert_never

from probe_harness import console, views
from probe_harness.arms import ArmCapture
from probe_harness.contract import Manifest
from probe_harness.families.base import (
    CustomCheck,
    GateFailure,
    JudgeResult,
    ProbeFunctions,
    RunOptions,
    decide,
    file_lines,
    kept_on_failure,
    provider_env,
    record_result,
    record_tool_version,
    refused,
    run_checks,
)
from probe_harness.gates import bodies, stamp
from probe_harness.gates import claude_otel as gates
from probe_harness.gates.bodies import ClaudeArms as ClaudeArms
from probe_harness.gates.bodies import MarkerField as MarkerField
from probe_harness.json_values import JsonValue
from probe_harness.models.raw import sample_source
from probe_harness.source import write_source
from probe_harness.workspace import create_workspace, template_file

FAMILY_TOOLS = ("claude",)

FX_SERVER = Path(__file__).resolve().parents[2] / "stubs" / "fx_server.py"


@dataclass(frozen=True)
class McpConfig:
    """Template `<template>` from the variant's fixture tree to `<arm>/mcp.json`, and pass it
    as the only MCP config."""

    template: str = "mcp.json"


@dataclass(frozen=True)
class PluginDir:
    """Copy `<source>` from the variant's fixture tree to `<arm>/fxp`, template its
    `.mcp.json`, and load it."""

    source: str = "plugin"


@dataclass(frozen=True)
class ClaudeArm:
    name: str
    # The arm's project tree, a directory under `fixtures/<variant>/`.
    tree: str
    prompt: str
    # Passed after the server's arguments.
    args: tuple[str, ...] = ()
    server: McpConfig | PluginDir | None = None


@dataclass(frozen=True)
class Control:
    """Gate 3: the ungoverned main-thread requests declaring an effort agree on one level."""


@dataclass(frozen=True)
class Model:
    """Gate 5: every governed request ran on the definition's model."""


@dataclass(frozen=True)
class DeferredMcp:
    """Gate 4: the arm's governed requests list <tools> as deferred, beside `ToolSearch`."""

    arm: str
    tools: tuple[str, ...]


@dataclass(frozen=True)
class McpConnected:
    """Gate 6: every main-thread request lists <tools>, directly or deferred."""

    tools: tuple[str, ...]


@dataclass(frozen=True)
class McpAbsent:
    """Gate 7: no main-thread request lists a tool starting with <prefix>."""

    prefix: str


@dataclass(frozen=True)
class DelegationBackground:
    """Gate 8: the arm delegated with `run_in_background` equal to <expected>."""

    arm: str
    expected: bool


@dataclass(frozen=True)
class DelegationFork:
    """Gate 10: fork mode was on in the arm, and the arm delegated under it."""

    arm: str


@dataclass(frozen=True)
class ToolAnswered:
    """Gate 9: the arm's model called <tool>, and the call was answered."""

    arm: str
    tool: str


type ClaudeGate = (
    Control
    | Model
    | DeferredMcp
    | McpConnected
    | McpAbsent
    | DelegationBackground
    | DelegationFork
    | ToolAnswered
)

# (the variant's fixture directory, arm directory, arm), run after the arm's server is set up.
type ArmArrange = Callable[[Path, Path, ClaudeArm], None]


@dataclass(frozen=True)
class ClaudeOtelProbe:
    model: str
    budget_usd: str
    marker: str
    arms: tuple[ClaudeArm, ...]
    gates: tuple[ClaudeGate, ...] = ()
    # Which request field carries the fixture: an agent becomes the system prompt, a
    # skill's body arrives in `messages[]`.
    marker_field: MarkerField = MarkerField.SYSTEM
    unset_env: tuple[str, ...] = ()
    set_env: tuple[tuple[str, str], ...] = ()
    # Gate on each arm's response bodies and hand them to the assertion, the custom checks
    # and the judge function, for an assertion reading calls; `view.json` includes them too.
    requires_responses: bool = False
    # Required by a manifest declaring `expected`, and refused by an option set.
    assertion: Callable[[ClaudeArms], JsonValue] | None = None
    custom_checks: tuple[CustomCheck[ClaudeArms], ...] = ()
    variants: tuple[str, ...] = ("assertion",)
    arrange_arm: ArmArrange | None = None
    functions: ProbeFunctions[ClaudeArms] = field(default_factory=ProbeFunctions)


def run(manifest: Manifest, name: str, options: RunOptions, probe: ClaudeOtelProbe) -> int:
    package = manifest.path.parent
    ws = create_workspace(name, options.variant)
    with kept_on_failure(ws):
        write_source(ws, options.source)
        try:
            record_tool_version(manifest, ws)
            for arm in probe.arms:
                _run_arm(package / "fixtures" / options.variant, ws, arm, probe)
            result = judge(ws, probe)
        except GateFailure as e:
            return refused(e, ws)
        return record_result(manifest, ws, result, options)


def parse(ws: Path, probe: ClaudeOtelProbe) -> ClaudeArms:
    """Every arm's sink as models, refusing an arm that captured nothing it can be read by."""
    arms = [arm.name for arm in probe.arms]
    # Without a connected server, an arm's tool set describes nothing.
    for served in (arm for arm in probe.arms if arm.server is not None):
        stamp(ws, served.name, "fx")
    # Gate 1. Only `*.request.json` files count: the sink also writes
    # `<request_id>.response.json`. Files are counted rather than size-checked, and never
    # against an expected number, since a single turn has been observed producing one, two,
    # and sixteen requests.
    for name in arms:
        sink = ws / name / "sink"
        if not any(sink.glob("*.request.json")):
            raise GateFailure(f"the {name} arm captured no request files in {sink}")
    if probe.requires_responses:
        for name in arms:
            if not any((ws / name / "sink").glob("*.response.json")):
                raise GateFailure(
                    f"the {name} arm captured no response files in {ws / name / 'sink'}"
                )
    return bodies.read_claude_arms(ws, arms)


def parse_sample(document: JsonValue, label: str, probe: ClaudeOtelProbe) -> ClaudeArms:
    """A sample's input as the arms `parse` would have read."""
    return bodies.claude_sample_arms(document, label, [arm.name for arm in probe.arms])


def expression_input(arms: ClaudeArms, probe: ClaudeOtelProbe) -> ClaudeArms:
    """What the assertion, custom checks and judge function see: responses only on request."""
    if probe.requires_responses:
        return arms
    return {name: ArmCapture(capture.requests, ()) for name, capture in arms.items()}


def view(arms: ClaudeArms, *, with_responses: bool) -> dict[str, list[JsonValue]]:
    """The inspection view written to `view.json`: each arm's request bodies, then, with
    <with_responses>, its responses."""
    return {
        name: [
            sample_source(body)
            for body in (*capture.requests, *(capture.responses if with_responses else ()))
        ]
        for name, capture in arms.items()
    }


def judge(ws: Path, probe: ClaudeOtelProbe) -> JudgeResult:
    """Judge a workspace from the sinks and stamps its arms left."""
    arms = parse(ws, probe)
    view_path = ws / "view.json"
    views.write_arm_view(view_path, view(arms, with_responses=probe.requires_responses))
    gates.marker(arms, probe.marker, probe.marker_field)
    for gate in probe.gates:
        _check(gate, arms, ws, probe)
    value = expression_input(arms, probe)
    run_checks(probe.custom_checks, value)
    if probe.functions.judge is not None:
        probe.functions.judge(value)
    # Printed before recording: a dry or failed record keeps the workspace, and this view is
    # kept so an author can inspect what the expressions read.
    console.say(f"the assembled view is at {view_path}")
    return decide(probe.assertion, value)


def arrange_server(
    fixtures: Path, arm_dir: Path, server: McpConfig | PluginDir | None
) -> list[str]:
    """Set up an arm's MCP server in <arm_dir>, and return the `claude` arguments that load it.

    An `--mcp-config` server always comes with `--strict-mcp-config`, so it is the only one the
    arm loads.
    """
    pairs = {
        "PYTHON": sys.executable,
        "FX_SERVER": str(FX_SERVER),
        "FX_STAMP": str(arm_dir / "fx.stamp"),
    }
    match server:
        case None:
            return []
        case McpConfig(template=template):
            config = arm_dir / "mcp.json"
            template_file(fixtures / template, config, **pairs)
            return ["--mcp-config", str(config), "--strict-mcp-config"]
        case PluginDir(source=source):
            plugin = arm_dir / "fxp"
            shutil.copytree(fixtures / source, plugin, symlinks=True)
            template_file(fixtures / source / ".mcp.json", plugin / ".mcp.json", **pairs)
            return ["--plugin-dir", str(plugin)]
        case _:
            assert_never(server)


def _check(gate: ClaudeGate, arms: ClaudeArms, ws: Path, probe: ClaudeOtelProbe) -> None:
    match gate:
        case Control():
            gates.control(arms, probe.marker, probe.marker_field)
        case Model():
            gates.model(arms, probe.marker, probe.model)
        case DeferredMcp(arm=arm, tools=tools):
            gates.deferred_mcp(arms, probe.marker, arm, tools)
        case McpConnected(tools=tools):
            gates.mcp_connected(arms, probe.marker, tools)
        case McpAbsent(prefix=prefix):
            gates.mcp_absent(arms, probe.marker, prefix)
        case DelegationBackground(arm=arm, expected=expected):
            gates.delegation_background(arms, ws, arm, expected)
        case DelegationFork(arm=arm):
            gates.delegation_fork(arms, ws, arm)
        case ToolAnswered(arm=arm, tool=tool):
            gates.tool_answered(arms, ws, arm, tool)
        case _:
            assert_never(gate)


def _run_arm(fixtures: Path, ws: Path, arm: ClaudeArm, probe: ClaudeOtelProbe) -> None:
    arm_dir = ws / arm.name
    project = arm_dir / "project"
    sink = arm_dir / "sink"
    project.mkdir(parents=True)
    sink.mkdir()
    shutil.copytree(fixtures / arm.tree, project, symlinks=True, dirs_exist_ok=True)

    server_args = arrange_server(fixtures, arm_dir, arm.server)
    if probe.arrange_arm is not None:
        probe.arrange_arm(fixtures, arm_dir, arm)

    env = {
        k: v
        for k, v in os.environ.items()
        if k != "CLAUDE_CODE_EFFORT_LEVEL" and k not in probe.unset_env
    }
    env.update(dict(probe.set_env))
    env["CLAUDE_CODE_ENABLE_TELEMETRY"] = "1"
    env["OTEL_LOG_RAW_API_BODIES"] = f"file:{sink}"
    argv = [
        "claude",
        "-p",
        arm.prompt,
        "--setting-sources",
        "project",
        "--model",
        probe.model,
        "--max-budget-usd",
        probe.budget_usd,
        *server_args,
        *arm.args,
    ]
    with (arm_dir / "stdout").open("wb") as out, (arm_dir / "stderr").open("wb") as err:
        completed = subprocess.run(
            argv, cwd=project, env=provider_env(project, env), stdout=out, stderr=err, check=False
        )
    if completed.returncode != 0:
        raise GateFailure(
            f"the {arm.name} arm exited nonzero; its stderr follows:",
            detail=file_lines(arm_dir / "stderr"),
        )
