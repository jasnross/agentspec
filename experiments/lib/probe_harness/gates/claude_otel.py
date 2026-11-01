"""The ten gates of the billed Claude probes, over each arm's typed bodies.

A gate reads each arm's `ArmCapture`, validated once per arm by the family's parse step, so a
body that does not fit its model has already refused the run before any gate runs. Gates 1
(request files) and 2 (`marker`) and the isolation gate, gate 7 (`isolated`), run on every
probe; a definition opts into the rest: 3 `control`, 4 `deferred_mcp`, 5 `model`, 6
`mcp_connected`, 8 `delegation_background`, 9 `tool_answered` and 10 `delegation_fork`.

No gate names an effort level: the effort packages' assertion is relational, comparing
each arm against the run's ungoverned main-thread requests.

Beside the gates sit the readings assertions share with them: `control_efforts`, the set
gate 3 compares, `relative_effort`, which reads an arm against it, and `call_outcome`, which
classifies an arm's calls to one tool.
"""

import re
from collections.abc import Iterator, Sequence
from pathlib import Path

from pydantic import MISSING

from probe_harness import json_values
from probe_harness.arms import ArmCapture
from probe_harness.assertions import NO_GOVERNED_REQUEST, unique
from probe_harness.gates import APPARATUS_FAILURE, GateFailure
from probe_harness.gates.bodies import (
    ClaudeArms,
    MarkerField,
    deferred_tools,
    governed,
    governed_requests,
    listed_tools,
    subagent,
)
from probe_harness.json_values import JsonValue, to_text
from probe_harness.models.claude_request import ClaudeRequest, ClaudeTool, ToolResultBlock
from probe_harness.models.claude_response import AgentCall, ClaudeResponse, ToolUseBlock

type Arms = ClaudeArms
type _Capture = ArmCapture[ClaudeRequest, ClaudeResponse]

# What `relative_effort` gives every arm when the control set is not one level, and an arm
# whose governed requests ran at that level.
NO_CONTROL_SET = "no-usable-control-set"
SAME_AS_UNGOVERNED = "same-as-ungoverned"

# The key `call_outcome` files a call's result texts under when neither outcome names them.
UNRECOGNIZED = "unrecognized"

# What every arm's canary token opens with. The family writes `<arm>/CLAUDE.md` holding it,
# followed by 32 hex digits, and gate 7 reads the token back from there.
CANARY_PREFIX = "AGENTSPEC-CANARY-"
_CANARY = re.compile(re.escape(CANARY_PREFIX) + r"[0-9a-f]{32}")

# An MCP tool's server prefix: `mcp__`, the shortest name up to the next `__`, then `__`.
_MCP_PREFIX = re.compile(r"mcp__[A-Za-z0-9_-]+?__")

# The text Claude Code answers a call with when `--permission-prompts none` refuses it.
_PERMISSION_DENIED = "Permission for this tool use was denied"


def marker(arms: Arms, marker: str, field: MarkerField = MarkerField.SYSTEM) -> None:
    """Gate 2: every arm holds at least one request the fixture governs.

    "Governs" is the marker appearing in one named field, never anywhere in the body: a
    subagent's reply echoes the fixture's text back to the main thread, on a request the
    fixture does not govern.
    """
    if not all(any(governed(r, field, marker) for r in arm.requests) for arm in arms.values()):
        raise GateFailure(
            f"an arm captured no request whose .{field.value} carries the fixture marker.",
            "that arm never engaged the fixture, so its value describes nothing.",
        )


def control(arms: Arms, marker: str, field: MarkerField = MarkerField.SYSTEM) -> None:
    """Gate 3: the ungoverned requests declaring an effort exist and agree on one level.

    The control set is the session's own level, so two kinds of request Claude sends beside
    the main thread are excluded. One is the intermittent title-generation sidecar, whose
    `output_config` holds no `effort`. The other is a subagent's request (`subagent`): its
    follow-up turns after a tool call carry no marker but run at the subagent's effort.
    Both are exclusions, so a Claude that stopped marking either kind cannot shift the
    control level silently: the rejoining requests either agree with it or fail this gate.
    Pass the same field as to gate 2, so the two agree on which requests the fixture
    governs.
    """
    if len(set(control_efforts(arms, marker, field))) != 1:
        raise GateFailure(
            "the ungoverned control set is empty or internally inconsistent.",
            "the assertion compares each arm against that set, so it cannot be read.",
            "this is a statement about the run, not about Claude. No record written.",
        )


def control_efforts(arms: Arms, marker: str, field: MarkerField = MarkerField.SYSTEM) -> list[str]:
    """The efforts gate 3 compares: each ungoverned, non-subagent request's, in capture order.

    The effort assertions read the control level from the same set, so the gate and the
    assertion cannot disagree on which requests are the session's own.
    """
    return [
        effort
        for arm in arms.values()
        for r in arm.requests
        if not governed(r, field, marker)
        and not subagent(r)
        and (effort := r.output_config.effort) is not MISSING
    ]


def relative_effort(
    arms: Arms, arm: str, marker: str, field: MarkerField = MarkerField.SYSTEM
) -> JsonValue:
    """The distinct efforts of the arm's governed requests, read against the control level.

    `NO_CONTROL_SET` when `control_efforts` is not one level, `NO_GOVERNED_REQUEST` when the
    arm governed nothing, `SAME_AS_UNGOVERNED` when its efforts are exactly the control
    level, and the efforts themselves otherwise. A governed request declaring no effort
    contributes `None`. Pass the field gate 2 and gate 3 were given.
    """
    control = unique(control_efforts(arms, marker, field))
    efforts = unique(
        [
            None if (effort := r.output_config.effort) is MISSING else effort
            for r in governed_requests(arms[arm].requests, marker, field)
        ]
    )
    if len(control) != 1:
        return NO_CONTROL_SET
    if not efforts:
        return NO_GOVERNED_REQUEST
    if efforts == control:
        return SAME_AS_UNGOVERNED
    return efforts


def deferred_mcp(arms: Arms, marker: str, arm: str, tools: Sequence[str]) -> None:
    """Gate 4: the arm's governed requests list every tool as deferred, beside `ToolSearch`.

    It decides whether the run measured Claude under its default tool search. It also pins
    the listing's format, one bare tool name per line, which the assertions read.
    """
    requests = governed_requests(_requests(arms, arm), marker)
    deferred = {line for r in requests for line in deferred_tools(r)}
    if set(tools) <= deferred and any(
        any(t.name == "ToolSearch" for t in r.tool_list) for r in requests
    ):
        return

    lines = [
        f"the {arm} arm's governed requests do not list {' '.join(tools)} as deferred "
        "tools alongside ToolSearch."
    ]
    if set(tools) <= {t.name for r in requests for t in r.tool_list}:
        lines.append(
            "they sit in tools[] instead: tool search was not enabled for this "
            "subagent — off for this run's model, or Claude Code changed when it defers "
            "MCP tools."
        )
    lines.append(
        "the package measures Claude under default tool search, so this run cannot be "
        "read. No record written."
    )
    raise GateFailure(*lines)


def model(arms: Arms, marker: str, model: str) -> None:
    """Gate 5: every governed request, in every arm, ran on the pinned model.

    The subagent's model decides whether tool search is on, and a main thread passing a
    `model` parameter would move the subagent off the pinned one.
    """
    wrong = [
        (name, r.model)
        for name in arms
        for r in governed_requests(_requests(arms, name), marker)
        if r.model != model
    ]
    if not wrong:
        return
    raise GateFailure(
        f"a governed request ran on a model other than {model}:",
        *(f"  arm {name}: {to_text(found)}" for name, found in wrong),
        "tool search depends on the subagent's model, so this run cannot be read. "
        "No record written.",
    )


def mcp_connected(arms: Arms, marker: str, tools: Sequence[str]) -> None:
    """Gate 6: in every arm, each main-thread request lists every tool, directly or deferred.

    The main thread is the ungoverned requests offering `Agent`. Without this, an arm whose
    expected value is "no MCP tools" reads identically to "the server had not connected
    when the subagent's tool set was built".
    """
    ok = True
    for arm in arms.values():
        main = _main_thread(arm, marker)
        if not (main and all(set(tools) <= set(listed_tools(r)) for r in main)):
            ok = False
            break
    if ok:
        return
    raise GateFailure(
        f"an arm's main thread did not list {' '.join(tools)} on every request "
        "that could delegate.",
        *(
            f"  arm {name}: no ungoverned request offered Agent"
            for name, arm in arms.items()
            if not _main_thread(arm, marker)
        ),
        "the server may not have connected before the subagent spawned, so an empty "
        "tool set describes nothing. No record written.",
    )


def isolated(arms: Arms, ws: Path) -> None:
    """Gate 7: no arm loaded configuration from outside its fixture.

    Each arm's `<arm>/CLAUDE.md` canary must exist, and no string of any of its requests may
    carry the canary's token: the canary sits above the project on Claude Code's search
    path, so a carried token shows instruction files were loaded.

    An MCP server from outside the fixture, such as a claude.ai connector, shows as an
    `mcp__<server>__` prefix. A tool a request offers, in `tools[]` or the deferred listing,
    must come from a server the arm arranged (`_arranged_prefixes`). Any other request text
    may also name a prefix the arm's own files name (`_fixture_named_prefixes`): an agent
    granting a tool of a server the arm does not arrange is a measured condition, and Claude
    Code repeats the grant in the Agent tool's description.
    """
    found: list[str] = []
    for name, arm in arms.items():
        arm_dir = ws / name
        strings = [s for r in arm.requests for s in r.raw.strings()]
        token = _canary_token(arm_dir / "CLAUDE.md")
        if token is None:
            found.append(f"arm {name}: the canary file is missing or holds no token")
        elif any(token in s for s in strings):
            found.append(f"arm {name}: a request carried the canary token")
        arranged = _arranged_prefixes(arm_dir)
        offered = {p for r in arm.requests for tool in listed_tools(r) for p in _prefixes(tool)}
        named = {p for s in strings for p in _prefixes(s)}
        found.extend(
            f"arm {name}: a request offered a tool of {prefix}, a server the arm did not arrange"
            for prefix in sorted(offered - arranged)
        )
        found.extend(
            f"arm {name}: a request named {prefix}, a server the arm did not arrange"
            for prefix in sorted(named - offered - arranged - _fixture_named_prefixes(arm_dir))
        )
    if found:
        raise GateFailure(
            "an arm's run was not isolated from the operator's configuration:",
            *(f"  {line}" for line in found),
            APPARATUS_FAILURE,
        )


def delegation_background(arms: Arms, ws: Path, arm: str, expected: bool) -> None:
    """Gate 8: the arm delegated in the expected mode, as asked and as run.

    Every `Agent` call in the arm's responses set `run_in_background` to <expected>, there
    was at least one, and each was answered by a `tool_result` that opens "Async agent
    launched" exactly when <expected> is true. It reads responses as well as requests: a
    request body carries no earlier assistant turn, so the call appears only in responses.
    """
    bodies = _with_sink(arms, ws, arm, "delegation")
    calls = [c for c in _tool_uses(bodies) if isinstance(c, AgentCall)]
    results = _tool_results(bodies)
    ok = (
        len(calls) > 0
        and all(
            (mode := c.input.run_in_background) is not MISSING and mode == expected for c in calls
        )
        and all(
            any(rid == c.id and asynchronous == expected for rid, asynchronous in results)
            for c in calls
        )
    )
    if not ok:
        text = "true" if expected else "false"
        raise GateFailure(
            f"arm {arm} did not delegate with run_in_background={text}, both as asked "
            "and as run, on every Agent call.",
            "the arm measured a different delegation mode than it claims, so its value "
            "describes nothing. No record written.",
        )


def tool_answered(arms: Arms, ws: Path, arm: str, tool: str) -> None:
    """Gate 9: the arm's model called <tool>, and some `tool_result` answered the call."""
    bodies = _with_sink(arms, ws, arm, "call")
    ids = {c.id for c in _tool_uses(bodies) if c.name == tool}
    if not (ids and any(rid in ids for rid, _ in _tool_results(bodies))):
        raise GateFailure(
            f"arm {arm} has no answered call to {tool}, so it says nothing about "
            "pre-approval. No record written."
        )


def delegation_fork(arms: Arms, ws: Path, arm: str) -> None:
    """Gate 10: fork mode was on in the arm, and the arm delegated under it.

    Fork mode removes the `Agent` tool's `run_in_background` parameter and runs every
    subagent in the background, so the missing parameter in the tool's own schema is the
    positive signal. Every request offering `Agent` must lack it, and every `Agent` call
    must be answered "Async agent launched".
    """
    bodies = _with_sink(arms, ws, arm, "delegation")
    schemas = [
        _declares(t, "run_in_background")
        for r in bodies.requests
        for t in r.tool_list
        if t.name == "Agent"
    ]
    calls = [c.id for c in _tool_uses(bodies) if isinstance(c, AgentCall)]
    results = _tool_results(bodies)
    if not schemas:
        failed = (
            "has no request offering the Agent tool, so whether fork mode was on cannot be read"
        )
    elif any(schemas):
        failed = "has a request whose Agent tool declares run_in_background, so fork mode was off"
    elif not calls:
        failed = "made no Agent call"
    elif any(not any(rid == c and asynchronous for rid, asynchronous in results) for c in calls):
        failed = (
            'has an Agent call not answered with "Async agent launched", so it did '
            "not run in the background"
        )
    else:
        return
    raise GateFailure(
        f"arm {arm} {failed}.",
        "the arm did not measure a fork-mode delegation, so its value describes "
        "nothing. No record written.",
    )


def call_outcome(arm: _Capture, tool: str, reply: str) -> JsonValue:
    """How the arm's calls to <tool> ended, from the results answering them.

    `"called"` when some result carries <reply>, the text the fixture's server returns;
    `"denied"` when there was a result and every one is an error refusing permission; else
    `{"unrecognized": [...]}`, each result's text, so an outcome neither names is visible.
    """
    ids = {call.id for call in _tool_uses(arm) if call.name == tool}
    results = [
        block
        for r in arm.requests
        for message in r.messages
        if message.role == "user" and not isinstance(message.content, str)
        for block in message.content
        if isinstance(block, ToolResultBlock) and block.tool_use_id in ids
    ]
    if any(reply in block.text for block in results):
        return "called"
    if results and all(
        block.is_error is True and _PERMISSION_DENIED in block.text for block in results
    ):
        return "denied"
    texts: list[JsonValue] = [block.text for block in results]
    return {UNRECOGNIZED: texts}


def _canary_token(path: Path) -> str | None:
    """The token in the arm's canary file, or `None` when it is missing or holds none."""
    try:
        found = _CANARY.search(path.read_text(encoding="utf-8"))
    except OSError:
        return None
    return found.group() if found else None


def _arranged_prefixes(arm_dir: Path) -> set[str]:
    """The MCP prefixes of the servers the family arranged for the arm.

    `mcp__<k>__` for each server `<k>` in `<arm>/mcp.json`, and `mcp__plugin_<name>_<k>__` for
    each server in `<arm>/fxp/.mcp.json`, where `<name>` is the plugin manifest's. A plugin
    with no manifest name gets no prefix, so its tools are refused rather than guessed at.
    """
    arranged = {f"mcp__{k}__" for k in _servers(arm_dir / "mcp.json")}
    plugin = arm_dir / "fxp"
    manifest = _json_file(plugin / ".claude-plugin" / "plugin.json")
    if isinstance(manifest, dict) and isinstance(name := manifest.get("name"), str):
        arranged |= {f"mcp__plugin_{name}_{k}__" for k in _servers(plugin / ".mcp.json")}
    return arranged


def _fixture_named_prefixes(arm_dir: Path) -> set[str]:
    """Every MCP prefix occurring in the files `_fixture_files` reads."""
    return {
        prefix
        for path in _fixture_files(arm_dir)
        for prefix in _prefixes(path.read_text(encoding="utf-8", errors="replace"))
    }


def _prefixes(text: str) -> set[str]:
    """Every `mcp__<server>__` prefix in <text>, each ending at the first `__` after `mcp__`."""
    return set(_MCP_PREFIX.findall(text))


def _fixture_files(arm_dir: Path) -> Iterator[Path]:
    """Every file under the arm's project and plugin copies, and its MCP config.

    Read when the arm is judged, so this is the fixture's copy plus whatever `arrange_arm`
    and the run itself wrote there. A fixture copied with its symlinks keeps them, so linked
    directories are followed.
    """
    for tree in (arm_dir / "project", arm_dir / "fxp"):
        found = tree.rglob("*", recurse_symlinks=True)
        yield from (path for path in sorted(found) if path.is_file())
    if (arm_dir / "mcp.json").is_file():
        yield arm_dir / "mcp.json"


def _servers(config: Path) -> list[str]:
    """The keys under `mcpServers` in an MCP config; none when the file is absent."""
    document = _json_file(config)
    servers = document.get("mcpServers") if isinstance(document, dict) else None
    return list(servers) if isinstance(servers, dict) else []


def _json_file(path: Path) -> JsonValue:
    """<path> as JSON, or `None` when it is absent or not JSON."""
    try:
        return json_values.loads(path.read_text(encoding="utf-8"))
    except OSError, ValueError:
        return None


def _requests(arms: Arms, arm: str) -> tuple[ClaudeRequest, ...]:
    bodies = arms.get(arm)
    return bodies.requests if bodies is not None else ()


def _main_thread(arm: _Capture, marker: str) -> list[ClaudeRequest]:
    """The ungoverned requests that offer `Agent`: the requests that can delegate."""
    return [
        r
        for r in arm.requests
        if not governed(r, MarkerField.SYSTEM, marker)
        and any(t.name == "Agent" for t in r.tool_list)
    ]


def _with_sink(arms: Arms, ws: Path, arm: str, reading: str) -> _Capture:
    """The arm's bodies, refusing when it holds no response or no request."""
    bodies = arms[arm]
    if not bodies.responses or not bodies.requests:
        raise GateFailure(
            f"the {arm} arm captured {len(bodies.responses)} response and "
            f"{len(bodies.requests)} request files in {ws / arm / 'sink'}, so its {reading} "
            "cannot be read."
        )
    return bodies


def _tool_results(bodies: _Capture) -> list[tuple[str, bool]]:
    """Each user `tool_result`'s id, and whether its text opens "Async agent launched"."""
    return [
        (block.tool_use_id, block.text.startswith("Async agent launched"))
        for r in bodies.requests
        for message in r.messages
        if message.role == "user" and not isinstance(message.content, str)
        for block in message.content
        if isinstance(block, ToolResultBlock)
    ]


def _tool_uses(bodies: _Capture) -> list[ToolUseBlock]:
    """Every `tool_use` block across the arm's responses; an error response holds none."""
    return [
        block
        for r in bodies.responses
        if r.content is not MISSING
        for block in r.content
        if isinstance(block, ToolUseBlock)
    ]


def _declares(tool: ClaudeTool, parameter: str) -> bool:
    """Whether <tool>'s input schema declares <parameter>; a tool with no schema declares none."""
    if tool.input_schema is MISSING:
        return False
    declared = tool.input_schema.properties.keys()
    return parameter in declared
