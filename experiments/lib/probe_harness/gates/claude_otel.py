"""View assembly and the ten gates of the billed Claude probes.

Each gate is a literal translation of a jq program into Python over the jq-built view and
the arm's sink files, which are only ever read. The jq semantics the translations keep:

- `tostring`: a string as-is, anything else as compact JSON (`jq_tostring`);
- `.. | strings`: every string value, recursively, in pre-order;
- `.x[]?`: nothing when `.x` is absent or not iterable;
- `unique`: sorted and deduplicated, under jq's structural equality;
- array subtraction, and `all`/`any` over empty sets (true and false), short-circuiting.

A jq runtime error made the bash gate fail, so here any error while reading or evaluating
becomes that gate's own `GateFailure`. No gate lets another exception escape.

No gate names an effort level: the effort packages' assertion is relational, comparing
each arm against the requests nothing governed in the same run.
"""

import json
from collections.abc import Iterator, Sequence
from pathlib import Path
from typing import Any

from probe_harness import jq_cli
from probe_harness.gates import GateFailure
from probe_harness.gates import jq_semantics as jq
from probe_harness.workspace import jq_tostring, parse_json_values

DEFERRED_LISTING = "The following deferred tools are now available via ToolSearch"

_EVALUATION_ERRORS = jq.EVALUATION_ERRORS


def assemble_view(ws: Path, out: Path, arms: Sequence[str]) -> None:
    """Gate 1, plus view assembly: `{<arm>: [<request bodies>], …}`.

    Only `*.request.json` files: the sink also writes `<request_id>.response.json`. Files
    are counted rather than size-checked, and never against an expected number, since a
    single turn has been observed producing one, two, and sixteen requests.
    """
    for arm in arms:
        sink = ws / arm / "sink"
        if not any(sink.glob("*.request.json")):
            raise GateFailure(f"probe: the {arm} arm captured no request files in {sink}")
    try:
        jq_cli.assemble_arm_view_claude(ws, arms, out)
    except jq_cli.JqError as e:
        raise GateFailure(*e.stderr.rstrip("\n").split("\n")) from None


def marker(view: Path, marker: str, field: str = ".system") -> None:
    """Gate 2: every arm holds at least one request the fixture governs.

    "Governs" is the marker appearing in one named field, never anywhere in the body: a
    subagent's reply echoes the fixture's text back to the main thread, on a request the
    fixture does not govern. An agent file becomes the system prompt (`.system`); a skill's
    body arrives in `messages[]` (`.messages`).
    """
    try:
        v = _load(view)
        ok = all(
            any(_governed_wrapped(r, field, marker) for r in jq.each(arm)) for arm in jq.each(v)
        )
    except _EVALUATION_ERRORS:
        ok = False
    if not ok:
        raise GateFailure(
            f"probe: an arm captured no request whose {field} carries the fixture marker.",
            "probe: that arm never engaged the fixture, so its value describes nothing.",
        )


def control(view: Path, marker: str, field: str = ".system") -> None:
    """Gate 3: the ungoverned requests declaring an effort exist and agree on one level.

    Requests without an `effort` key are excluded: Claude emits an intermittent
    title-generation sidecar whose `output_config` holds no `effort`. Pass the same field as
    to gate 2, so the two definitions partition the requests between them.
    """
    try:
        v = _load(view)
        efforts = [
            jq.index(jq.index(r, "output_config"), "effort")
            for arm in jq.each(v)
            for r in jq.each(arm)
            if not _governed_wrapped(r, field, marker)
            and jq.has(jq.index(r, "output_config"), "effort")
        ]
        ok = len(efforts) > 0 and len(jq.unique(efforts)) == 1
    except _EVALUATION_ERRORS:
        ok = False
    if not ok:
        raise GateFailure(
            "probe: the ungoverned control set is empty or internally inconsistent.",
            "probe: the assertion compares each arm against that set, so it cannot be read.",
            "probe: this is a statement about the run, not about Claude. No record written.",
        )


def deferred_mcp(view: Path, marker: str, arm: str, tools: Sequence[str]) -> None:
    """Gate 4: the arm's governed requests list every tool as deferred, beside `ToolSearch`.

    It decides whether the run measured Claude under its default tool search. It also pins
    the listing's format, one bare tool name per line, which the projections read.
    """
    try:
        v = _load(view)
        governed = [
            r for r in jq.each(jq.index(v, arm)) if marker in jq_tostring(jq.index(r, "system"))
        ]
        deferred = jq.unique([line for r in governed for line in _deferred_lines(r)])
        ok = jq.subtract(tools, deferred) == [] and any(
            any(jq.index(t, "name") == "ToolSearch" for t in jq.each_opt(jq.index(r, "tools")))
            for r in governed
        )
    except _EVALUATION_ERRORS:
        ok = False
    if ok:
        return

    lines = [
        f"probe: the {arm} arm's governed requests do not list {' '.join(tools)} as deferred "
        "tools alongside ToolSearch."
    ]
    try:
        v = _load(view)
        direct = [
            jq.index(t, "name")
            for r in jq.each(jq.index(v, arm))
            if marker in jq_tostring(jq.index(r, "system"))
            for t in jq.each_opt(jq.index(r, "tools"))
        ]
        if jq.subtract(tools, direct) == []:
            lines.append(
                "probe: they sit in tools[] instead: tool search was not enabled for this "
                "subagent — off for this run's model, or Claude Code changed when it defers "
                "MCP tools."
            )
    except _EVALUATION_ERRORS:
        pass
    lines.append(
        "probe: the package measures Claude under default tool search, so this run cannot be "
        "read. No record written."
    )
    raise GateFailure(*lines)


def model(view: Path, marker: str, model: str) -> None:
    """Gate 5: every governed request, in every arm, ran on the pinned model.

    The subagent's model decides whether tool search is on, and a main thread passing a
    `model` parameter would move the subagent off the pinned one.
    """
    try:
        v = _load(view)
        ok = all(
            all(
                jq.index(r, "model") == model
                for r in jq.each(arm)
                if marker in jq_tostring(jq.index(r, "system"))
            )
            for arm in jq.each(v)
        )
    except _EVALUATION_ERRORS:
        ok = False
    if ok:
        return

    lines = [f"probe: a governed request ran on a model other than {model}:"]
    try:
        v = _load(view)
        lines += [
            f"probe:   arm {name}: {jq_tostring(jq.index(r, 'model'))}"
            for name, requests in jq.entries(v)
            for r in jq.each(requests)
            if marker in jq_tostring(jq.index(r, "system")) and jq.index(r, "model") != model
        ]
    except _EVALUATION_ERRORS:
        pass
    lines.append(
        "probe: tool search depends on the subagent's model, so this run cannot be read. "
        "No record written."
    )
    raise GateFailure(*lines)


def mcp_connected(view: Path, marker: str, tools: Sequence[str]) -> None:
    """Gate 6: in every arm, each main-thread request lists every tool, directly or deferred.

    The main thread is the ungoverned requests offering `Agent`. Without this, an arm whose
    expected value is "no MCP tools" reads identically to "the server had not connected
    when the subagent's tool set was built".
    """
    try:
        v = _load(view)
        ok = True
        for arm in jq.each(v):
            main = _main_thread(arm, marker)
            if not (main and all(jq.subtract(tools, _listed(r)) == [] for r in main)):
                ok = False
                break
    except _EVALUATION_ERRORS:
        ok = False
    if ok:
        return

    lines = [
        f"probe: an arm's main thread did not list {' '.join(tools)} on every request "
        "that could delegate."
    ]
    try:
        v = _load(view)
        lines += [
            f"probe:   arm {name}: no ungoverned request offered Agent"
            for name, requests in jq.entries(v)
            if not _main_thread(requests, marker)
        ]
    except _EVALUATION_ERRORS:
        pass
    lines.append(
        "probe: the server may not have connected before the subagent spawned, so an empty "
        "tool set describes nothing. No record written."
    )
    raise GateFailure(*lines)


def mcp_absent(view: Path, marker: str, prefix: str) -> None:
    """Gate 7: no main-thread request, in any arm, lists a tool starting with <prefix>.

    A package keeping names by prefix needs the session to hold no server that prefix could
    match other than the fixture's, such as a user-scope server in `~/.claude.json`.
    """
    try:
        v = _load(view)
        ok = all(
            all(
                not jq.startswith(name, prefix)
                for r in _main_thread(arm, marker)
                for name in _listed(r)
            )
            for arm in jq.each(v)
        )
    except _EVALUATION_ERRORS:
        ok = False
    if ok:
        return

    lines = [f"probe: a main-thread request listed a tool starting with {prefix}:"]
    try:
        v = _load(view)
        for name, requests in jq.entries(v):
            found = [
                n
                for r in _main_thread(requests, marker)
                for n in _listed(r)
                if jq.startswith(n, prefix)
            ]
            lines += [f"probe:   arm {name}: {jq_tostring(n)}" for n in jq.unique(found)]
    except _EVALUATION_ERRORS:
        pass
    lines.append(
        "probe: a server outside the fixture shares that name, likely a user-scope server in "
        "~/.claude.json, so the projection cannot tell its tools from the fixture's. "
        "No record written."
    )
    raise GateFailure(*lines)


def delegation_background(ws: Path, arm: str, expected: bool) -> None:
    """Gate 8: the arm delegated in the expected mode, as asked and as run.

    Every `Agent` call in the arm's responses set `run_in_background` to <expected>, there
    was at least one, and each was answered by a `tool_result` that opens "Async agent
    launched" exactly when <expected> is true. It reads the sink, not the view: a request
    body carries no earlier assistant turn, so the call appears only in responses.
    """
    response_files, request_files = _sink_files(ws, arm, "delegation")
    want = expected
    try:
        responses, requests = _concatenated(response_files), _concatenated(request_files)
        calls = [
            (jq.index(c, "id"), jq.index(jq.index(c, "input"), "run_in_background"))
            for c in _tool_uses(responses)
            if jq.index(c, "name") == "Agent"
        ]
        results = _tool_results(requests, guard_messages=False)
        ok = (
            len(calls) > 0
            and all(jq.equal(mode, want) for _, mode in calls)
            and all(
                any(
                    jq.equal(rid, cid) and jq.equal(asynchronous, want)
                    for rid, asynchronous in results
                )
                for cid, _ in calls
            )
        )
    except _EVALUATION_ERRORS:
        ok = False
    if not ok:
        text = "true" if expected else "false"
        raise GateFailure(
            f"probe: arm {arm} did not delegate with run_in_background={text}, both as asked "
            "and as run, on every Agent call.",
            "probe: the arm measured a different delegation mode than it claims, so its value "
            "describes nothing. No record written.",
        )


def tool_answered(ws: Path, arm: str, tool: str) -> None:
    """Gate 9: the arm's model called <tool>, and some `tool_result` answered the call."""
    response_files, request_files = _sink_files(ws, arm, "call")
    try:
        responses, requests = _concatenated(response_files), _concatenated(request_files)
        ids = [jq.index(c, "id") for c in _tool_uses(responses) if jq.index(c, "name") == tool]
        ok = len(ids) > 0 and any(
            jq.type_of(block) == "object"
            and jq.index(block, "type") == "tool_result"
            and any(jq.equal(i, jq.index(block, "tool_use_id")) for i in ids)
            for block in _user_content_blocks(requests, guard_messages=False)
        )
    except _EVALUATION_ERRORS:
        ok = False
    if not ok:
        raise GateFailure(
            f"probe: arm {arm} has no answered call to {tool}, so it says nothing about "
            "pre-approval. No record written."
        )


def delegation_fork(ws: Path, arm: str) -> None:
    """Gate 10: fork mode was on in the arm, and the arm delegated under it.

    Fork mode removes the `Agent` tool's `run_in_background` parameter and runs every
    subagent in the background, so the missing parameter in the tool's own schema is the
    positive signal. Every request offering `Agent` must lack it, and every `Agent` call
    must be answered "Async agent launched".
    """
    response_files, request_files = _sink_files(ws, arm, "delegation")
    try:
        responses, requests = _concatenated(response_files), _concatenated(request_files)
        schemas = [
            jq.has(
                jq.alternative(jq.try_index(jq.index(t, "input_schema"), "properties"), {}),
                "run_in_background",
            )
            for r in requests
            for t in jq.each_opt(jq.index(r, "tools"))
            if jq.type_of(t) == "object" and jq.index(t, "name") == "Agent"
        ]
        calls = [jq.index(c, "id") for c in _tool_uses(responses) if jq.index(c, "name") == "Agent"]
        results = _tool_results(requests, guard_messages=True)
        if not schemas:
            failed = (
                "has no request offering the Agent tool, so whether fork mode was on cannot be read"
            )
        elif any(schemas):
            failed = (
                "has a request whose Agent tool declares run_in_background, so fork mode was off"
            )
        elif not calls:
            failed = "made no Agent call"
        elif any(
            not any(jq.equal(rid, c) and asynchronous is True for rid, asynchronous in results)
            for c in calls
        ):
            failed = (
                'has an Agent call not answered with "Async agent launched", so it did '
                "not run in the background"
            )
        else:
            return
    except _EVALUATION_ERRORS:
        raise GateFailure(
            f"probe: jq could not evaluate the {arm} arm sink. No record written."
        ) from None
    raise GateFailure(
        f"probe: arm {arm} {failed}.",
        "probe: the arm did not measure a fork-mode delegation, so its value describes "
        "nothing. No record written.",
    )


def _load(view: Path) -> Any:
    return json.loads(view.read_text(encoding="utf-8"))


def _sink_files(ws: Path, arm: str, reading: str) -> tuple[list[Path], list[Path]]:
    """An arm's response and request files, refusing when either set is empty."""
    sink = ws / arm / "sink"
    responses = sorted(sink.glob("*.response.json"))
    requests = sorted(sink.glob("*.request.json"))
    if not responses or not requests:
        raise GateFailure(
            f"probe: the {arm} arm captured {len(responses)} response and {len(requests)} "
            f"request files in {sink}, so its {reading} cannot be read."
        )
    return responses, requests


def _concatenated(paths: Sequence[Path]) -> list[Any]:
    """Every JSON value in <paths>, concatenated as `cat` concatenated them for jq."""
    return parse_json_values("".join(p.read_text(encoding="utf-8") for p in paths))


def _tool_uses(responses: Sequence[Any]) -> Iterator[Any]:
    """`$res[] | .content[]? | select(type == "object" and .type == "tool_use")`."""
    for response in responses:
        for block in jq.each_opt(jq.index(response, "content")):
            if jq.type_of(block) == "object" and jq.index(block, "type") == "tool_use":
                yield block


def _user_content_blocks(requests: Sequence[Any], *, guard_messages: bool) -> Iterator[Any]:
    """`$req[] | .messages[]? | select(… .role == "user") | .content | if array then .[]`."""
    for request in requests:
        for message in jq.each_opt(jq.index(request, "messages")):
            if guard_messages and jq.type_of(message) != "object":
                continue
            if jq.index(message, "role") != "user":
                continue
            content = jq.index(message, "content")
            if isinstance(content, list):
                yield from content


def _tool_results(requests: Sequence[Any], *, guard_messages: bool) -> list[tuple[Any, bool]]:
    """Each `tool_result`'s id, and whether its text opens "Async agent launched"."""
    results = []
    for block in _user_content_blocks(requests, guard_messages=guard_messages):
        if jq.type_of(block) == "object" and jq.index(block, "type") == "tool_result":
            text = jq.join(
                [
                    jq.alternative(jq.index(item, "text"), "")
                    if jq.type_of(item) == "object"
                    else jq_tostring(item)
                    for item in jq.flatten([jq.index(block, "content")])
                ]
            )
            results.append(
                (jq.index(block, "tool_use_id"), text.startswith("Async agent launched"))
            )
    return results


def _main_thread(requests: Any, marker: str) -> list[Any]:
    """The ungoverned requests that offer `Agent`: the requests that can delegate."""
    return [
        r
        for r in jq.each(requests)
        if marker not in jq_tostring(jq.index(r, "system"))
        and any(jq.index(t, "name") == "Agent" for t in jq.each_opt(jq.index(r, "tools")))
    ]


def _listed(request: Any) -> list[Any]:
    """jq's `listed`: the request's direct tool names, then its deferred-listing lines."""
    direct = [jq.index(t, "name") for t in jq.each_opt(jq.index(request, "tools"))]
    return direct + _deferred_lines(request)


def _deferred_lines(value: Any) -> list[str]:
    """`.. | strings | select(contains(<listing>)) | split("\\n")[]`."""
    return [line for s in jq.strings(value) if DEFERRED_LISTING in s for line in s.split("\n")]


def _governed_wrapped(request: Any, field: str, marker: str) -> bool:
    """`[<field>] | tostring | contains($m)`: every output of the field, inside one array.

    The collapse matters for a multi-output field such as `.messages[].content`: without
    it a governed request would be judged once per output, and gate 3 would admit it to
    the control set through whichever outputs lack the marker.
    """
    return marker in jq_tostring(jq.path_outputs(request, field))
