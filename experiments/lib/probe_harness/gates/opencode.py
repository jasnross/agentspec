"""The gates of the OpenCode fake-provider probes, over each arm's typed requests."""

from collections.abc import Sequence
from pathlib import Path

from probe_harness.gates import APPARATUS_FAILURE, GateFailure, stamp
from probe_harness.gates.bodies import OpenCodeRequest, contains

__all__ = ["APPARATUS_FAILURE", "GOVERNED_JQ", "governed", "stamp", "tool_result"]


# The `any(…)` arguments selecting a governed request (the marker in a system message), for a
# definition's custom check. `$m` is the marker; `_governed_requests` is the same in Python.
GOVERNED_JQ = '.messages[]?; .role == "system" and (.content | tostring | contains($m))'


def governed(requests: Sequence[OpenCodeRequest], arm: str, marker: str, stderr: Path) -> None:
    """Some request in the arm carries the marker in a system message.

    OpenCode warns rather than fails on an unknown `--agent` and runs its default agent,
    whose requests carry no marker, so an arm without one measured another agent.
    """
    if not _governed_requests(requests, marker):
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
        any(m.role == "tool" for m in r.messages) for r in _governed_requests(requests, marker)
    ):
        raise GateFailure(
            f"arm {arm}: no marked request carried a tool result.",
            "either OpenCode never executed the scripted call, or the session stopped "
            "after the rejection —",
            "a release that no longer honors experimental.continue_loop_on_deny does "
            f"that. See {stderr}.",
            APPARATUS_FAILURE,
        )


def _governed_requests(requests: Sequence[OpenCodeRequest], marker: str) -> list[OpenCodeRequest]:
    """The requests carrying <marker> in a system message."""
    return [
        r
        for r in requests
        if any(m.role == "system" and contains(m.content, marker) for m in r.messages)
    ]
