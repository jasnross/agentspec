"""The gates of the OpenCode fake-provider probes, over each arm's validated requests."""

from collections.abc import Callable, Sequence
from pathlib import Path

from probe_harness.assertions import consistent, sorted_values, unique
from probe_harness.gates import APPARATUS_FAILURE, GateFailure, stamp
from probe_harness.json_values import JsonValue
from probe_harness.models.opencode_request import OpenCodeRequest

__all__ = [
    "APPARATUS_FAILURE",
    "governed",
    "governed_requests",
    "governed_tool_names",
    "stamp",
    "tool_result",
    "tool_results",
    "toolset",
]


def governed(requests: Sequence[OpenCodeRequest], arm: str, marker: str, stderr: Path) -> None:
    """Some request in the arm carries the marker in a system message.

    OpenCode warns rather than fails on an unknown `--agent` and runs its default agent,
    whose requests carry no marker, so an arm without one measured another agent.
    """
    if not governed_requests(requests, marker):
        raise GateFailure(
            f"arm {arm}: no request carried {marker} in a system message.",
            f"OpenCode may have fallen back to its default agent; see {stderr}.",
            APPARATUS_FAILURE,
        )


def tool_result(requests: Sequence[OpenCodeRequest], arm: str, marker: str, stderr: Path) -> None:
    """Some governed request in the arm carries a `tool` message.

    The oracle is the tool result in the follow-up request. Without one the arm says nothing
    about permissions: OpenCode never executed the scripted call, or the session stopped at
    the rejection before sending a follow-up.
    """
    if not any(
        any(m.role == "tool" for m in r.messages) for r in governed_requests(requests, marker)
    ):
        raise GateFailure(
            f"arm {arm}: no marked request carried a tool result.",
            "either OpenCode never executed the scripted call, or the session stopped "
            "after the rejection —",
            "a release that no longer honors experimental.continue_loop_on_deny does "
            f"that. See {stderr}.",
            APPARATUS_FAILURE,
        )


def governed_requests(requests: Sequence[OpenCodeRequest], marker: str) -> list[OpenCodeRequest]:
    """The requests carrying <marker> in a system message."""
    return [
        r for r in requests if any(m.role == "system" and marker in m.content for m in r.messages)
    ]


def governed_tool_names(requests: Sequence[OpenCodeRequest], marker: str) -> set[str]:
    """Every tool name some governed request offers."""
    return {name for r in governed_requests(requests, marker) for name in r.tool_names}


def toolset(
    requests: Sequence[OpenCodeRequest],
    marker: str,
    keep: Callable[[str], bool] = lambda name: True,
) -> JsonValue:
    """The tool names the governed requests offer, kept by <keep>: one sorted list when every
    governed request offers the same, `{"inconsistent": [...]}` of each distinct list when
    they differ, and `NO_GOVERNED_REQUEST` when the arm had none."""
    offered: list[JsonValue] = [
        sorted_values([name for name in r.tool_names if keep(name)])
        for r in governed_requests(requests, marker)
    ]
    return consistent(unique(offered))


def tool_results(requests: Sequence[OpenCodeRequest], marker: str) -> list[str]:
    """The content of every `tool` message in the governed requests, in request order."""
    return [
        m.content
        for r in governed_requests(requests, marker)
        for m in r.messages
        if m.role == "tool"
    ]
