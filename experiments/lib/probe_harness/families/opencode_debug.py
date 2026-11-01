"""The OpenCode debug family: one `opencode debug …` oracle whose output is the view.

Fully script-driven: no human step, no network, no credentials. The oracle's raw output
goes to `view.json` unprojected, and is validated against the definition's `output` model:
the definition's assertion reads the model.

Every run is isolated (`OpenCodeIsolation`). Workspace layout: the variant's fixture tree
copied to `<ws>/project/`, beside `<ws>/home/` and `<ws>/xdg/{config,cache,data,state}/`,
plus `<ws>/view.json` and `<ws>/oracle.stderr`.
"""

import shutil
import subprocess
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, cast

from pydantic import RootModel, ValidationError

from probe_harness import console, json_values
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
from probe_harness.families.opencode_env import OpenCodeIsolation, ancestor_instructions_refusal
from probe_harness.json_values import JsonValue
from probe_harness.models.raw import ProviderModel, field_path
from probe_harness.source import write_source
from probe_harness.workspace import create_workspace, workspace_root

FAMILY_TOOLS = ("opencode",)


@dataclass(frozen=True)
class OpenCodeDebugProbe[D: ProviderModel | RootModel[Any]]:
    # After `opencode debug`, e.g. ("config", "--pure").
    argv: tuple[str, ...]
    # The model the oracle's document is validated against.
    output: type[D]
    isolation: OpenCodeIsolation
    variants: tuple[str, ...] = ("assertion",)
    # Required by a manifest declaring `expected`, and refused by an option set.
    assertion: Callable[[D], JsonValue] | None = None
    custom_checks: tuple[CustomCheck[D], ...] = ()
    functions: ProbeFunctions[D] = field(default_factory=ProbeFunctions)


def run[D: ProviderModel | RootModel[Any]](
    manifest: Manifest, name: str, options: RunOptions, probe: OpenCodeDebugProbe[D]
) -> int:
    package = manifest.path.parent
    if (refusal := ancestor_instructions_refusal(workspace_root())) is not None:
        console.say_refusal(refusal)
        return 1
    ws = create_workspace(name, options.variant)
    with kept_on_failure(ws):
        write_source(ws, options.source)
        try:
            _arrange_and_act(manifest, package, ws, options.variant, probe)
            result = judge(ws, probe)
        except GateFailure as e:
            return refused(e, ws)
        return record_result(manifest, ws, result, options)


def parse[D: ProviderModel | RootModel[Any]](ws: Path, probe: OpenCodeDebugProbe[D]) -> D:
    """The oracle's document as the definition's `output` model, from the files it left."""
    view = ws / "view.json"
    if not view.is_file() or view.stat().st_size == 0:
        raise GateFailure(
            "the oracle produced no output; its stderr follows:",
            detail=file_lines(ws / "oracle.stderr"),
        )
    # Refused here, where the file that is not JSON is known, rather than leaving the reader
    # to suspect an expression that is correct.
    try:
        document = json_values.loads(view.read_text(encoding="utf-8"))
    except ValueError:
        document = None
    # A resolved config or listing is an object or an array; a bare `null` would read as
    # nulls and record a false `refuted`.
    if not isinstance(document, dict | list):
        raise GateFailure(
            f"the oracle wrote a document that is not valid JSON — {view.stat().st_size} bytes at "
            f"{view}"
        )
    return _validated(probe, document, str(view))


def parse_sample[D: ProviderModel | RootModel[Any]](
    document: JsonValue, label: str, probe: OpenCodeDebugProbe[D]
) -> D:
    """A sample's input as the model `parse` would have read."""
    return _validated(probe, document, label)


def judge[D: ProviderModel | RootModel[Any]](ws: Path, probe: OpenCodeDebugProbe[D]) -> JudgeResult:
    """Judge a workspace from the files the oracle left: its output and its stderr."""
    value = parse(ws, probe)
    run_checks(probe.custom_checks, value)
    if probe.functions.judge is not None:
        probe.functions.judge(value)
    view = ws / "view.json"
    # Printed before recording: a dry or failed record keeps the workspace, and this view is
    # kept so an author can inspect what the expressions read.
    console.say(f"the assembled view is at {view}")
    return decide(probe.assertion, value)


def _validated[D: ProviderModel | RootModel[Any]](
    probe: OpenCodeDebugProbe[D], document: JsonValue, label: str
) -> D:
    try:
        # `model_validate` returns an instance of `output`, which is `D`; mypy cannot follow
        # `Self` through a union bound.
        return cast(D, probe.output.model_validate(document))
    except ValidationError as e:
        error = e.errors()[0]
        where = field_path(error["loc"]) or "the document"
        raise GateFailure(
            f"{label}: {where}: {error['msg']}",
            f"the oracle's output does not fit {probe.output.__name__}, so the run cannot be "
            "read. No record written.",
        ) from None


def _arrange_and_act[D: ProviderModel | RootModel[Any]](
    manifest: Manifest, package: Path, ws: Path, variant: str, probe: OpenCodeDebugProbe[D]
) -> None:
    source = package / "fixtures" / variant
    layout = probe.isolation.layout(ws)
    cwd = layout.project
    env = probe.isolation.env(layout)
    shutil.copytree(source, cwd, symlinks=True, dirs_exist_ok=True)

    # Redirected to a file rather than piped: `opencode debug` truncates its stdout at
    # 65536 bytes when it is a pipe, which would hand the recorder a half-written document.
    command = " ".join(("opencode", "debug", *probe.argv))
    record_tool_version(manifest, ws)
    with (ws / "view.json").open("wb") as out, (ws / "oracle.stderr").open("wb") as err:
        completed = subprocess.run(
            ["opencode", "debug", *probe.argv],
            cwd=cwd,
            env=provider_env(cwd, env),
            stdout=out,
            stderr=err,
            check=False,
        )
    if completed.returncode != 0:
        raise GateFailure(f'"{command}" exited nonzero:', detail=file_lines(ws / "oracle.stderr"))
