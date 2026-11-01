"""The Claude init family: free `claude -p` runs whose stream-json `init` event is the oracle.

Each arm copies a fixture tree into its own project and runs `claude -p` there under a model
id no catalog holds, so the run ends at model resolution and no request is billed. Before
that, Claude Code prints the session it resolved as a `system`/`init` event: among other
things, every MCP server it loaded and each one's status. That is what these probes read,
so they measure what Claude Code loads into a run, at `resolved-config` depth.

The user message is held back for `settle_seconds`, because `init` is printed when the
first message arrives and lists only the servers registered by then. Sent at once, it can
list the operator's claude.ai connectors as `pending`, or not at all.

An arm with `home` runs with `HOME` relocated into the arm, for a user-scope
`~/.claude.json` the fixture arranges. A relocated home holds no login, so such an arm also
gets a placeholder API key; it can say nothing about what the operator's account loads.

`--setting-sources project` is passed unless an arm clears `setting_sources`, as the billed
Claude family always passes it.

Workspace layout, per arm: `<ws>/<arm>/{project, stdout, stderr, fxp/, home/, *.stamp}`,
plus `<ws>/view.json`.
"""

import contextlib
import os
import shutil
import subprocess
import sys
import time
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path

from pydantic import BaseModel, ValidationError

from probe_harness import console, json_values, views
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
from probe_harness.families.claude_otel import FX_SERVER, McpConfig, PluginDir, arrange_server
from probe_harness.gates import APPARATUS_FAILURE
from probe_harness.json_values import JsonValue
from probe_harness.models.claude_stream import ClaudeInit, ClaudeResult
from probe_harness.models.raw import field_path, sample_source
from probe_harness.source import write_source
from probe_harness.workspace import create_workspace, template_file

FAMILY_TOOLS = ("claude",)

# The key a relocated home runs under. It authenticates nothing, so no request it sends can
# be billed.
PLACEHOLDER_API_KEY = "sk-ant-agentspec-probe-placeholder"

# Each would decide which login or config directory a run uses instead of the arm's.
_CLEARED = ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CONFIG_DIR")

# How long a run may take once its message is sent; it ends at model resolution.
_RUN_TIMEOUT_SECONDS = 120


@dataclass(frozen=True)
class UserConfig:
    """Relocate `HOME` to `<arm>/home`, and template `<template>` from the variant's fixture
    tree to its `.claude.json`, the user-scope config.

    The template takes `PYTHON`, `FX_SERVER` and `USER_STAMP`, which names `<arm>/user.stamp`.
    """

    template: str = "user.claude.json"


@dataclass(frozen=True)
class ClaudeInitArm:
    name: str
    # The arm's project tree, a directory under `fixtures/<variant>/`.
    tree: str
    # Passed after the server's arguments.
    args: tuple[str, ...] = ()
    server: McpConfig | PluginDir | None = None
    home: UserConfig | None = None
    # `None` passes no `--setting-sources`, so Claude Code reads every source.
    setting_sources: str | None = "project"
    set_env: tuple[tuple[str, str], ...] = ()


@dataclass(frozen=True)
class InitCapture:
    """What one arm printed: the session it resolved, and how the run ended."""

    init: ClaudeInit
    result: ClaudeResult


# Each arm's capture, in the definition's arm order.
type ClaudeInitArms = Mapping[str, InitCapture]


@dataclass(frozen=True)
class ClaudeInitProbe:
    # A model id no catalog holds, which every arm's `init` must name.
    model: str
    arms: tuple[ClaudeInitArm, ...]
    settle_seconds: float = 8.0
    unset_env: tuple[str, ...] = ()
    # Required by a manifest declaring `expected`, and refused by an option set.
    assertion: Callable[[ClaudeInitArms], JsonValue] | None = None
    custom_checks: tuple[CustomCheck[ClaudeInitArms], ...] = ()
    variants: tuple[str, ...] = ("assertion",)
    functions: ProbeFunctions[ClaudeInitArms] = field(default_factory=ProbeFunctions)


def run(manifest: Manifest, name: str, options: RunOptions, probe: ClaudeInitProbe) -> int:
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


def parse(ws: Path, probe: ClaudeInitProbe) -> ClaudeInitArms:
    """Every arm's `init` and `result` events as models, from the stdout it left."""
    arms: dict[str, InitCapture] = {}
    for arm in probe.arms:
        stdout = ws / arm.name / "stdout"
        try:
            events = json_values.load_stream(stdout.read_text(encoding="utf-8"))
        except (ValueError, OSError) as e:
            raise GateFailure(
                f"arm {arm.name}: {stdout} is not a stream of JSON events: {e}"
            ) from None
        init = _one(events, "system", arm.name, stdout)
        result = _one(events, "result", arm.name, stdout)
        arms[arm.name] = InitCapture(
            init=_validated(ClaudeInit, init, arm.name, stdout),
            result=_validated(ClaudeResult, result, arm.name, stdout),
        )
    return arms


def parse_sample(document: JsonValue, label: str, probe: ClaudeInitProbe) -> ClaudeInitArms:
    """A sample's `{"<arm>": {"init": {...}, "result": {...}}}` as the arms `parse` reads."""
    names = [arm.name for arm in probe.arms]
    if not isinstance(document, dict) or list(document) != names:
        found = list(document) if isinstance(document, dict) else type(document).__name__
        raise GateFailure(
            f"{label}: input must be an object holding the arms {', '.join(names)} in order, "
            f"but holds {found}"
        )
    arms: dict[str, InitCapture] = {}
    for name, arm in document.items():
        if not isinstance(arm, dict) or sorted(arm) != ["init", "result"]:
            raise GateFailure(f"{label}: {name} must be an object holding init, result")
        arms[name] = InitCapture(
            init=_validated(ClaudeInit, arm["init"], name, f"{label} {name}.init"),
            result=_validated(ClaudeResult, arm["result"], name, f"{label} {name}.result"),
        )
    return arms


def view(arms: ClaudeInitArms) -> dict[str, list[JsonValue]]:
    """The inspection view written to `view.json`: each arm's `init`, then its `result`."""
    return {
        name: [sample_source(capture.init), sample_source(capture.result)]
        for name, capture in arms.items()
    }


def judge(ws: Path, probe: ClaudeInitProbe) -> JudgeResult:
    """Judge a workspace from the stdout its arms left."""
    arms = parse(ws, probe)
    view_path = ws / "view.json"
    views.write_arm_view(view_path, view(arms))
    for arm in probe.arms:
        _unbilled(arms[arm.name], arm, probe.model)
        _settled(arms[arm.name], arm.name)
    run_checks(probe.custom_checks, arms)
    if probe.functions.judge is not None:
        probe.functions.judge(arms)
    # Printed before recording: a dry or failed record keeps the workspace, and this view is
    # kept so an author can inspect what the expressions read.
    console.say(f"the assembled view is at {view_path}")
    return decide(probe.assertion, arms)


def _unbilled(capture: InitCapture, arm: ClaudeInitArm, model: str) -> None:
    """The arm resolved the definition's model, and the run ended in an error.

    An arm on the operator's login must have failed on the model itself, which shows the
    login held and the run reached model resolution: a run that never got that far may not
    have loaded the account's connectors either. An arm on the placeholder key fails on the
    key instead.
    """
    if capture.init.model != model:
        raise GateFailure(
            f"arm {arm.name}: init names the model {capture.init.model}, not {model}.",
            APPARATUS_FAILURE,
        )
    if not capture.result.is_error:
        raise GateFailure(
            f"arm {arm.name}: the run ended without an error, so a model answered it.",
            APPARATUS_FAILURE,
        )
    if arm.home is None and model not in capture.result.result:
        raise GateFailure(
            f"arm {arm.name}: the run failed on something other than the model:",
            f"  {capture.result.result}",
            "the operator's login may not have held, so the arm may not have loaded what the "
            "account would. " + APPARATUS_FAILURE,
        )


def _settled(capture: InitCapture, arm: str) -> None:
    """No server in the arm's `init` was still connecting."""
    pending = [s for s in capture.init.mcp_servers if s.status == "pending"]
    if pending:
        raise GateFailure(
            f"arm {arm}: init listed {len(pending)} MCP server(s) still pending, so what it "
            "loaded was not settled when init was printed.",
            "Raise the definition's settle_seconds. " + APPARATUS_FAILURE,
        )


def _run_arm(fixtures: Path, ws: Path, arm: ClaudeInitArm, probe: ClaudeInitProbe) -> None:
    arm_dir = ws / arm.name
    project = arm_dir / "project"
    project.mkdir(parents=True)
    shutil.copytree(fixtures / arm.tree, project, symlinks=True, dirs_exist_ok=True)
    server_args = arrange_server(fixtures, arm_dir, arm.server)

    env = {k: v for k, v in os.environ.items() if k not in probe.unset_env and k not in _CLEARED}
    if arm.home is not None:
        home = arm_dir / "home"
        home.mkdir()
        template_file(
            fixtures / arm.home.template,
            home / ".claude.json",
            PYTHON=sys.executable,
            FX_SERVER=str(FX_SERVER),
            USER_STAMP=str(arm_dir / "user.stamp"),
        )
        env["HOME"] = str(home)
        env["ANTHROPIC_API_KEY"] = PLACEHOLDER_API_KEY
    env.update(dict(arm.set_env))

    sources = () if arm.setting_sources is None else ("--setting-sources", arm.setting_sources)
    argv = [
        "claude",
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        *sources,
        "--model",
        probe.model,
        *server_args,
        *arm.args,
    ]
    message: JsonValue = {"type": "user", "message": {"role": "user", "content": "hi"}}
    with (arm_dir / "stdout").open("wb") as out, (arm_dir / "stderr").open("wb") as err:
        process = subprocess.Popen(
            argv,
            cwd=project,
            env=provider_env(project, env),
            stdin=subprocess.PIPE,
            stdout=out,
            stderr=err,
        )
        assert process.stdin is not None
        try:
            time.sleep(probe.settle_seconds)
            # A run that already exited has closed its stdin; its events say why.
            with contextlib.suppress(BrokenPipeError):
                process.stdin.write((json_values.dumps_line(message) + "\n").encode("utf-8"))
                process.stdin.close()
            process.wait(timeout=_RUN_TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            raise GateFailure(
                f"arm {arm.name}: claude did not exit within {_RUN_TIMEOUT_SECONDS} s of its "
                "message; its stderr follows:",
                detail=file_lines(arm_dir / "stderr"),
            ) from None
        except BaseException:
            process.kill()
            process.wait()
            raise
    # Every arm exits nonzero, since the model cannot resolve; the events say how it ended.
    if not (arm_dir / "stdout").stat().st_size:
        raise GateFailure(
            f"arm {arm.name}: claude printed no events; its stderr follows:",
            detail=file_lines(arm_dir / "stderr"),
        )


def _one(events: Sequence[JsonValue], kind: str, arm: str, stdout: Path) -> JsonValue:
    """The one event of type <kind> — for `system`, of subtype `init` — refusing none or two."""
    found = [
        e
        for e in events
        if isinstance(e, dict)
        and e.get("type") == kind
        and (kind != "system" or e.get("subtype") == "init")
    ]
    name = "init" if kind == "system" else kind
    if len(found) != 1:
        raise GateFailure(
            f"arm {arm}: {stdout} holds {len(found)} {name} events, not one.", APPARATUS_FAILURE
        )
    return found[0]


def _validated[M: BaseModel](model: type[M], value: JsonValue, arm: str, label: object) -> M:
    try:
        return model.model_validate(value)
    except ValidationError as e:
        error = e.errors()[0]
        where = field_path(error["loc"])
        problem = f"{where}: {error['msg']}" if where else "the event is not a JSON object"
        raise GateFailure(
            f"arm {arm}: {label}: {problem}",
            "the gates cannot read this event, so the run describes nothing. No record written.",
        ) from None
