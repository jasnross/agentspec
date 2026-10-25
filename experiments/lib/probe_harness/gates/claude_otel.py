"""The ten gates of the billed Claude probes, over each arm's typed bodies.

A gate reads `bodies.ClaudeArmBodies`, validated once per arm by the family's judge, so a
body that does not fit its model has already refused the run before any gate runs.

No gate names an effort level: the effort packages' assertion is relational, comparing
each arm against the run's ungoverned main-thread requests.
"""

from collections.abc import Mapping, Sequence

from pydantic import MISSING

from probe_harness.gates import GateFailure
from probe_harness.gates.bodies import (
    ClaudeArmBodies,
    MarkerField,
    deferred_tools,
    governed,
    listed_tools,
    subagent,
)
from probe_harness.json_values import to_text
from probe_harness.models.claude_request import ClaudeRequest, ClaudeTool, ToolResultBlock
from probe_harness.models.claude_response import AgentCall, ToolUseBlock

type Arms = Mapping[str, ClaudeArmBodies]


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
    efforts = {
        effort
        for arm in arms.values()
        for r in arm.requests
        if not governed(r, field, marker)
        and not subagent(r)
        and (effort := r.output_config.effort) is not MISSING
    }
    if len(efforts) != 1:
        raise GateFailure(
            "the ungoverned control set is empty or internally inconsistent.",
            "the assertion compares each arm against that set, so it cannot be read.",
            "this is a statement about the run, not about Claude. No record written.",
        )


def deferred_mcp(arms: Arms, marker: str, arm: str, tools: Sequence[str]) -> None:
    """Gate 4: the arm's governed requests list every tool as deferred, beside `ToolSearch`.

    It decides whether the run measured Claude under its default tool search. It also pins
    the listing's format, one bare tool name per line, which the projections read.
    """
    requests = _governed(arms, arm, marker)
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
        (name, r.model) for name in arms for r in _governed(arms, name, marker) if r.model != model
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


def mcp_absent(arms: Arms, marker: str, prefix: str) -> None:
    """Gate 7: no main-thread request, in any arm, lists a tool starting with <prefix>.

    A package keeping names by prefix needs the session to hold no server that prefix could
    match other than the fixture's, such as a user-scope server in `~/.claude.json`.
    """
    found = {
        name: sorted(
            {n for r in _main_thread(arm, marker) for n in listed_tools(r) if n.startswith(prefix)}
        )
        for name, arm in arms.items()
    }
    if not any(found.values()):
        return
    raise GateFailure(
        f"a main-thread request listed a tool starting with {prefix}:",
        *(f"  arm {name}: {n}" for name, names in found.items() for n in names),
        "a server outside the fixture shares that name, likely a user-scope server in "
        "~/.claude.json, so the projection cannot tell its tools from the fixture's. "
        "No record written.",
    )


def delegation_background(arms: Arms, arm: str, expected: bool) -> None:
    """Gate 8: the arm delegated in the expected mode, as asked and as run.

    Every `Agent` call in the arm's responses set `run_in_background` to <expected>, there
    was at least one, and each was answered by a `tool_result` that opens "Async agent
    launched" exactly when <expected> is true. It reads responses as well as requests: a
    request body carries no earlier assistant turn, so the call appears only in responses.
    """
    bodies = _with_sink(arms, arm, "delegation")
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


def tool_answered(arms: Arms, arm: str, tool: str) -> None:
    """Gate 9: the arm's model called <tool>, and some `tool_result` answered the call."""
    bodies = _with_sink(arms, arm, "call")
    ids = {c.id for c in _tool_uses(bodies) if c.name == tool}
    if not (ids and any(rid in ids for rid, _ in _tool_results(bodies))):
        raise GateFailure(
            f"arm {arm} has no answered call to {tool}, so it says nothing about "
            "pre-approval. No record written."
        )


def delegation_fork(arms: Arms, arm: str) -> None:
    """Gate 10: fork mode was on in the arm, and the arm delegated under it.

    Fork mode removes the `Agent` tool's `run_in_background` parameter and runs every
    subagent in the background, so the missing parameter in the tool's own schema is the
    positive signal. Every request offering `Agent` must lack it, and every `Agent` call
    must be answered "Async agent launched".
    """
    bodies = _with_sink(arms, arm, "delegation")
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


def _requests(arms: Arms, arm: str) -> tuple[ClaudeRequest, ...]:
    bodies = arms.get(arm)
    return bodies.requests if bodies is not None else ()


def _governed(arms: Arms, arm: str, marker: str) -> list[ClaudeRequest]:
    """The arm's requests whose system prompt carries the marker."""
    return [r for r in _requests(arms, arm) if governed(r, MarkerField.SYSTEM, marker)]


def _main_thread(arm: ClaudeArmBodies, marker: str) -> list[ClaudeRequest]:
    """The ungoverned requests that offer `Agent`: the requests that can delegate."""
    return [
        r
        for r in arm.requests
        if not governed(r, MarkerField.SYSTEM, marker)
        and any(t.name == "Agent" for t in r.tool_list)
    ]


def _with_sink(arms: Arms, arm: str, reading: str) -> ClaudeArmBodies:
    """The arm's bodies, refusing when it holds no response or no request."""
    bodies = arms[arm]
    if not bodies.responses or not bodies.requests:
        raise GateFailure(
            f"the {arm} arm captured {len(bodies.responses)} response and "
            f"{len(bodies.requests)} request files in {bodies.sink}, so its {reading} cannot "
            "be read."
        )
    return bodies


def _tool_results(bodies: ClaudeArmBodies) -> list[tuple[str, bool]]:
    """Each user `tool_result`'s id, and whether its text opens "Async agent launched"."""
    return [
        (block.tool_use_id, block.text.startswith("Async agent launched"))
        for r in bodies.requests
        for message in r.messages
        if message.role == "user" and not isinstance(message.content, str)
        for block in message.content
        if isinstance(block, ToolResultBlock)
    ]


def _tool_uses(bodies: ClaudeArmBodies) -> list[ToolUseBlock]:
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
