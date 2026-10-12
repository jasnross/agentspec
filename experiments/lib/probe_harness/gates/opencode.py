"""The gates of the OpenCode fake-provider probes.

Each translates the jq program the bash runners applied to the assembled view, over the
jq-built view read once with `json`. Any error while reading or evaluating becomes the
gate's own `GateFailure`, as a jq runtime error made the bash gate fail.
"""

import json
from pathlib import Path
from typing import Any

from probe_harness.gates import APPARATUS_FAILURE, GateFailure, stamp
from probe_harness.gates import jq_semantics as jq
from probe_harness.workspace import jq_tostring

__all__ = ["APPARATUS_FAILURE", "GOVERNED_JQ", "governed", "stamp", "tool_result"]


# The `any(…)` arguments selecting a governed request (the marker in a system message), for a
# definition's custom check. `$m` is the marker; `_governed_requests` is the same in Python.
GOVERNED_JQ = '.messages[]?; .role == "system" and (.content | tostring | contains($m))'


def governed(view: Path, arm: str, marker: str, stderr: Path) -> None:
    """Some request in the arm carries the marker in a system message.

    OpenCode warns rather than fails on an unknown `--agent` and runs its default agent,
    whose requests carry no marker, so an arm without one measured another agent.
    """
    try:
        ok = len(_governed_requests(_load(view), arm, marker)) > 0
    except jq.EVALUATION_ERRORS:
        ok = False
    if not ok:
        raise GateFailure(
            f"probe: arm {arm}: no request carried {marker} in a system message.",
            f"probe: OpenCode may have fallen back to its default agent; see {stderr}.",
            APPARATUS_FAILURE,
        )


def tool_result(view: Path, arm: str, marker: str, stderr: Path) -> None:
    """Some governed request in the arm carries a `tool` message.

    The oracle is the tool result in the follow-up request. Without one the arm says nothing
    about permissions: OpenCode never executed the scripted call, or the session stopped at
    the rejection before sending a follow-up.
    """
    try:
        ok = any(
            any(jq.index(m, "role") == "tool" for m in jq.each_opt(jq.index(r, "messages")))
            for r in _governed_requests(_load(view), arm, marker)
        )
    except jq.EVALUATION_ERRORS:
        ok = False
    if not ok:
        raise GateFailure(
            f"probe: arm {arm}: no marked request carried a tool result.",
            "probe: either OpenCode never executed the scripted call, or the session stopped "
            "after the rejection —",
            "probe: a release that no longer honors experimental.continue_loop_on_deny does "
            f"that. See {stderr}.",
            APPARATUS_FAILURE,
        )


def _governed_requests(view: Any, arm: str, marker: str) -> list[Any]:
    """`.[$arm] | [.[] | select(any(.messages[]?; .role == "system" and (… | contains($m))))]`."""
    return [
        r
        for r in jq.each(jq.index(view, arm))
        if any(
            jq.index(m, "role") == "system" and marker in jq_tostring(jq.index(m, "content"))
            for m in jq.each_opt(jq.index(r, "messages"))
        )
    ]


def _load(view: Path) -> Any:
    return json.loads(view.read_text(encoding="utf-8"))
