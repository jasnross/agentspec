"""The manual family: a person drives the provider while the runner blocks.

Packages here differ only in their fixtures, the instructions they print, and whether the
answer is read from the capture or from a person.

The workspace separates two halves deliberately:

    <ws>/project/   provider config only; this is what the operator opens
    <ws>/capture/   the hook script and its payloads

The capture sits outside the opened project because a probe whose oracle is the agent's
answer can otherwise be defeated by the agent reading the apparatus: a marker string in a
capture script inside the project is findable with a filesystem search. Nothing is written
outside <ws>, which makes a collision with a real `agentspec sync` target impossible.

Each line of `capture/payloads.jsonl` is parsed on its own, against the definition's
`payload` model, into a `CaptureLine`: a payload, the hook's `{"unparsed": …}` fallback, or
an `Unrecognized` line. A poll reports each unrecognized line as it lands, so a payload
shape the model does not expect surfaces while the operator is still in the session, and
the judge refuses any before an expression reads the capture.
"""

import os
import shutil
import sys
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from pydantic import ValidationError

from probe_harness import console, json_values, views
from probe_harness.contract import CaptureSource, Manifest, OptionsAssertion
from probe_harness.families.base import (
    GateFailure,
    JudgeResult,
    ProbeFunctions,
    RunOptions,
    Selected,
    UsageError,
    call,
    decide,
    kept_on_failure,
    record_result,
    record_tool_version,
    refused,
    run_wait_for,
)
from probe_harness.json_values import JsonValue
from probe_harness.models.raw import ProviderModel, RawJson, field_path, sample_source
from probe_harness.source import write_source
from probe_harness.workspace import (
    create_workspace,
    prompt_selection,
    template_file,
    wait_for_capture,
)

# The capture hook runs jq inside the provider, and `_materialize` templates its absolute path
# into `dump-hook.sh`, so a missing jq must refuse before the workspace exists: found later,
# it would cost the capture of a live session.
FAMILY_TOOLS = ("jq",)

# The directory under `fixtures/` holding the capture apparatus every variant shares.
CAPTURE_DIR = "capture"


@dataclass(frozen=True)
class Unparsed:
    """`dump-hook.sh`'s fallback line, `{"unparsed": <raw>}`, for a payload that was not JSON."""

    raw: str


@dataclass(frozen=True)
class Unrecognized:
    """A capture line that is not JSON, or fails the definition's payload model."""

    # 1-based.
    line: int
    # The line's text when it is not JSON.
    raw: RawJson | str
    error: ValidationError | ValueError

    @property
    def problem(self) -> str:
        """`line <n>: <field path>: <message>`, as the poll and the judge report it."""
        if isinstance(self.error, ValidationError):
            first = self.error.errors()[0]
            where = field_path(first["loc"]) or "the payload"
            return f"line {self.line}: {where}: {first['msg']}"
        return f"line {self.line}: not JSON: {self.error}"


type CaptureLine[P] = P | Unparsed | Unrecognized


@dataclass(frozen=True)
class ManualProbe[P: ProviderModel = ProviderModel]:
    instructions: str
    # The model one capture line is parsed against.
    payload: type[P]
    # Whether the capture holds what the probe waits for; polled, then checked by the judge.
    wait_for: Callable[[Sequence[CaptureLine[P]]], bool]
    variants: tuple[str, ...] = ("assertion",)
    # Required by a manifest declaring `expected`, and refused by an option set.
    assertion: Callable[[Sequence[P | Unparsed]], JsonValue] | None = None
    # The tool version a `capture` version source reads; None for no version. Required by a
    # capture version source, and refused by any other.
    capture_version: Callable[[Sequence[P | Unparsed]], str | None] | None = None
    functions: ProbeFunctions[tuple[CaptureLine[P], ...]] = field(default_factory=ProbeFunctions)


def parse_lines[P: ProviderModel](text: str, payload: type[P]) -> tuple[CaptureLine[P], ...]:
    """Each non-blank line of <text>, parsed on its own."""
    lines: list[CaptureLine[P]] = []
    for number, line in enumerate(text.split("\n"), start=1):
        if not line.strip():
            continue
        try:
            value = json_values.loads(line)
        except ValueError as e:
            lines.append(Unrecognized(number, line, e))
            continue
        if (
            isinstance(value, dict)
            and list(value) == ["unparsed"]
            and isinstance(value["unparsed"], str)
        ):
            lines.append(Unparsed(value["unparsed"]))
            continue
        try:
            lines.append(payload.model_validate(value))
        except ValidationError as e:
            lines.append(Unrecognized(number, RawJson(value), e))
    return tuple(lines)


def parse[P: ProviderModel](
    ws: Path, probe: ManualProbe[P], *, lines: int | None = None
) -> tuple[CaptureLine[P], ...]:
    """The capture's lines, or its first <lines> lines, refusing a workspace with no capture."""
    capture = ws / "capture"
    if not capture.is_dir():
        raise GateFailure(f"{ws} is not a probe workspace (no capture/ directory)")
    payloads = capture / "payloads.jsonl"
    if not payloads.is_file() or payloads.stat().st_size == 0:
        raise GateFailure(
            f"no captured payloads at {payloads}",
            "the hook did not fire. Check that the provider was fully quit and reopened.",
        )
    text = payloads.read_text(encoding="utf-8")
    if lines is not None:
        text = "\n".join(text.split("\n")[:lines])
    return parse_lines(text, probe.payload)


def parse_sample[P: ProviderModel](
    document: JsonValue, label: str, probe: ManualProbe[P]
) -> tuple[CaptureLine[P], ...]:
    """A sample's input, a list of capture lines, as `parse` would have read them."""
    if not isinstance(document, list):
        raise GateFailure(f"{label}: input must be a list of capture lines")
    parsed = parse_lines("\n".join(json_values.dumps_line(v) for v in document), probe.payload)
    unrecognized = [line for line in parsed if isinstance(line, Unrecognized)]
    if unrecognized:
        raise GateFailure(f"{label}: {unrecognized[0].problem}")
    return parsed


def line_document(line: CaptureLine[Any]) -> JsonValue:
    """A parsed line as the JSON value it was parsed from."""
    if isinstance(line, Unparsed):
        return {"unparsed": line.raw}
    if isinstance(line, Unrecognized):
        return line.raw if isinstance(line.raw, str) else sample_source(line.raw)
    return sample_source(line)


def recognized[P](lines: Sequence[CaptureLine[P]]) -> list[P | Unparsed]:
    """<lines> without `Unrecognized` ones, which the judge has already refused."""
    return [line for line in lines if not isinstance(line, Unrecognized)]


def run(manifest: Manifest, name: str, options: RunOptions, probe: ManualProbe[Any]) -> int:
    """The whole human-driven flow as one blocking invocation: arrange, print, poll, record.

    Blocking is what removes the stale-capture failure: the workspace was created by the
    invocation still running, so there is no earlier capture to mistake for this one.
    """
    ws = arrange_human_workspace(manifest.path.parent, name, options.variant)
    with kept_on_failure(ws):
        write_source(ws, options.source)
        try:
            record_tool_version(manifest, ws)
        except GateFailure as e:
            return refused(e, ws)
        print(f"\nWorkspace ready: {ws / 'project'}", file=sys.stderr)
        print(
            f"(capture lives outside it, at {ws / 'capture'})\n{probe.instructions}",
            file=sys.stderr,
        )

        if not wait_for_capture(ws, _poll(ws, probe), options.timeout):
            # The workspace is never deleted: discarding one an operator spent a live session
            # on is the single unrecoverable mistake a runner could make, and reading the
            # capture is how they tell a hook that never fired from a procedure that went
            # off the rails.
            return refused(
                GateFailure(
                    f"timed out waiting for the capture after {options.timeout}s",
                    "this run is over; re-run the probe to try again.",
                    f"if the capture arrived after the deadline, record it with: "
                    f"experiments/probe record {name} {ws}",
                ),
                ws,
            )
        try:
            result = judge(manifest, ws, probe, None, interactive=True)
        except GateFailure as e:
            return refused(e, ws)
        return record_result(manifest, ws, result, options)


def judge(
    manifest: Manifest,
    ws: Path,
    probe: ManualProbe[Any],
    selection: str | None,
    *,
    interactive: bool,
) -> JudgeResult:
    """Judge a captured workspace, for a live run and for `probe record` alike.

    Refuses unless the capture exists, is non-empty, every line is recognized, and it
    satisfies `wait_for`, so a workspace whose hook never fired, or fired for the wrong
    event, is never recorded. The assertion's shape, not the driver, then decides the
    answer: an option set is a person choosing from a list, and an `expected` is computed
    over the capture.
    """
    lines = parse(ws, probe)
    unrecognized = [line for line in lines if isinstance(line, Unrecognized)]
    if unrecognized:
        raise GateFailure(
            f"the capture holds {len(unrecognized)} line(s) the payload model does not recognize:",
            *(f"  {line.problem}" for line in unrecognized),
            f"see {ws / 'capture' / 'payloads.jsonl'}. No record written.",
        )
    payloads = ws / "capture" / "payloads.jsonl"
    view = ws / "view.json"
    views.write_capture_view(payloads, view)
    if not run_wait_for(probe.wait_for, lines):
        raise GateFailure(
            "the capture does not satisfy the definition's wait_for",
            f"the provider did not produce the payload this probe waits for; see {payloads}.",
        )
    if probe.functions.judge is not None:
        probe.functions.judge(lines)

    captured_version = _captured_version(manifest, probe, lines)
    if isinstance(manifest.assertion, OptionsAssertion):
        if selection is None:
            if not interactive:
                raise UsageError(
                    f"{manifest.path.parent.name} declares options; pass --selection <id>"
                )
            # The machine has already proved the apparatus worked, so the person answers
            # only the question no machine can.
            selection = prompt_selection(manifest.assertion.options, manifest.question)
        return JudgeResult(Selected(selection), captured_version)
    result = decide(probe.assertion, recognized(lines))
    return JudgeResult(result.outcome, captured_version)


def arrange_human_workspace(package: Path, name: str, variant: str) -> Path:
    """Materialize the workspace and return its root, the path `probe record` takes."""
    ws = create_workspace(name, variant)
    with kept_on_failure(ws):
        _materialize(package, ws, variant)
    return ws


def _materialize(package: Path, ws: Path, variant: str) -> None:
    project = ws / "project"
    project.mkdir()

    capture_script = ws / "capture" / "dump-hook.sh"
    template_file(
        package / "fixtures" / CAPTURE_DIR / "dump-hook.sh",
        capture_script,
        JQ=shutil.which("jq") or "jq",
        PAYLOADS=str(ws / "capture" / "payloads.jsonl"),
    )
    _make_executable(capture_script)

    fixtures = package / "fixtures" / variant
    for src in sorted(fixtures.rglob("*")):
        # Regular files only: a fixture tree is repo-controlled, so a symlink would be a
        # mistake rather than a feature.
        if src.is_symlink() or not src.is_file():
            continue
        rel = src.relative_to(fixtures)
        dst = project / rel
        template_file(src, dst, CAPTURE_SCRIPT=str(capture_script))

        # Templating writes a fresh file, so carry the executable bit across, or a fixture
        # the provider is meant to run becomes a hook that silently never fires.
        if os.access(src, os.X_OK):
            _make_executable(dst)

        # A workspace path carrying a quote or backslash would yield config the provider
        # silently ignores. Fail here, where the cause is obvious.
        if rel.suffix == ".json" and not _parses(dst):
            console.say(f"generated {dst} is not valid JSON — check the workspace path")
            raise SystemExit(1)


def _make_executable(path: Path) -> None:
    # As `chmod +x`: add every execute bit the umask allows.
    umask = os.umask(0)
    os.umask(umask)
    path.chmod(path.stat().st_mode | (0o111 & ~umask))


def _parses(path: Path) -> bool:
    try:
        json_values.loads(path.read_text(encoding="utf-8"))
    except ValueError:
        return False
    return True


def _captured_version(
    manifest: Manifest, probe: ManualProbe[Any], lines: Sequence[CaptureLine[Any]]
) -> str | None:
    """A capture version source's value; None for every other source."""
    if not isinstance(manifest.version_source, CaptureSource):
        return None
    # The agreement check guarantees a `capture_version` for a capture source.
    assert probe.capture_version is not None
    version = call("capture_version", probe.capture_version, recognized(lines))
    if version is not None and type(version) is not str:
        raise GateFailure(
            f"the capture_version yielded a {type(version).__name__}, not a string or None"
        )
    return version


def _poll(ws: Path, probe: ManualProbe[Any]) -> Callable[[], bool]:
    """The poll's test: the definition's `wait_for` over the capture's parsed lines.

    It reports each newly unrecognized line once, and a `wait_for` that raises once per
    distinct message, and counts either as "not yet": the operator may still be
    mid-session, and only the judge refuses.
    """
    payloads = ws / "capture" / "payloads.jsonl"
    wait = probe.wait_for
    reported_lines: set[int] = set()
    reported_errors: set[str] = set()

    def matches() -> bool:
        try:
            data = payloads.read_bytes()
        except OSError:
            return False
        # A line still being written has no newline yet, and may end inside a multi-byte
        # character; it is read on a later poll.
        try:
            text = data[: data.rfind(b"\n") + 1].decode("utf-8")
        except UnicodeDecodeError:
            return False
        lines = parse_lines(text, probe.payload)
        for line in lines:
            if isinstance(line, Unrecognized) and line.line not in reported_lines:
                reported_lines.add(line.line)
                console.say(
                    f"the capture holds a line the payload model does not recognize: {line.problem}"
                )
        try:
            return run_wait_for(wait, lines)
        except GateFailure as e:
            message = " ".join(e.lines)
            if message not in reported_errors:
                reported_errors.add(message)
                console.say(f"{message} (polling continues)")
                console.verbatim(e.detail)
            return False

    return matches
