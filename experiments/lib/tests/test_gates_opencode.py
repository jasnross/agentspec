"""The OpenCode fake-provider gates, over fabricated request bodies and workspaces."""

from pathlib import Path
from typing import Any

import pytest
from probe_harness.gates import GateFailure, bodies
from probe_harness.gates import opencode as gates
from probe_harness.models.opencode_request import OpenCodeRequest
from probe_harness.views import SinkEntry, SinkValues

MARKER = "AGENTSPEC-PROBE-MARKER-TEST"


def _arm(view: dict[str, list[Any]], arm: str) -> tuple[OpenCodeRequest, ...]:
    """The arm's fabricated request bodies, validated as the judge validates them."""
    entries = [
        SinkEntry(Path(arm) / "sink" / f"{n:04}.request.json", value)
        for n, value in enumerate(view.get(arm, []))
    ]
    return bodies.opencode_arms({arm: SinkValues(requests=entries, responses=[])}, Path())[arm]


def _system(text: Any) -> dict[str, Any]:
    return {"role": "system", "content": text}


def test_governed_passes_on_a_marked_system_message(tmp_path: Path) -> None:
    view = {"a": [{"messages": [_system(f"x {MARKER} y")]}]}
    gates.governed(_arm(view, "a"), "a", MARKER, tmp_path / "a" / "stderr")


def test_governed_fails_on_a_view_whose_marker_sits_only_in_a_user_message(
    tmp_path: Path,
) -> None:
    view = {"a": [{"messages": [_system("plain"), {"role": "user", "content": MARKER}]}]}
    with pytest.raises(GateFailure) as failure:
        gates.governed(_arm(view, "a"), "a", MARKER, tmp_path / "a" / "stderr")
    assert failure.value.lines == (
        f"arm a: no request carried {MARKER} in a system message.",
        f"OpenCode may have fallen back to its default agent; see {tmp_path}/a/stderr.",
        gates.APPARATUS_FAILURE,
    )


def test_governed_matches_a_marker_inside_structured_content(tmp_path: Path) -> None:
    view = {"a": [{"messages": [_system([{"type": "text", "text": MARKER}])]}]}
    gates.governed(_arm(view, "a"), "a", MARKER, tmp_path / "stderr")


def test_governed_fails_for_an_arm_missing_from_the_view(tmp_path: Path) -> None:
    view = {"a": [{"messages": [_system(MARKER)]}]}
    with pytest.raises(GateFailure):
        gates.governed(_arm(view, "b"), "b", MARKER, tmp_path / "stderr")


def test_tool_result_passes_when_a_governed_request_carries_a_tool_message(
    tmp_path: Path,
) -> None:
    view = {"a": [{"messages": [_system(MARKER), {"role": "tool", "content": "r"}]}]}
    gates.tool_result(_arm(view, "a"), "a", MARKER, tmp_path / "stderr")


def test_tool_result_fails_when_no_governed_request_has_a_tool_message(tmp_path: Path) -> None:
    view = {
        "a": [
            {"messages": [_system(MARKER), {"role": "user", "content": "hi"}]},
            # A tool message on an unmarked request does not count.
            {"messages": [_system("title"), {"role": "tool", "content": "r"}]},
        ]
    }
    with pytest.raises(GateFailure) as failure:
        gates.tool_result(_arm(view, "a"), "a", MARKER, tmp_path / "a" / "stderr")
    assert failure.value.lines[0] == "arm a: no marked request carried a tool result."
    assert "the scripted call" in failure.value.lines[1]


def test_stamp_fails_for_a_missing_stamp(tmp_path: Path) -> None:
    (tmp_path / "a").mkdir()
    with pytest.raises(GateFailure) as failure:
        gates.stamp(tmp_path, "a", "fx_extra")
    assert failure.value.lines == (
        "arm a: the fx_extra server never answered tools/list, so the arm describes nothing.",
        gates.APPARATUS_FAILURE,
    )


def test_stamp_passes_when_the_stamp_exists(tmp_path: Path) -> None:
    (tmp_path / "a").mkdir()
    (tmp_path / "a" / "fx.stamp").write_text("tools/list\n", encoding="utf-8")
    gates.stamp(tmp_path, "a", "fx")


@pytest.mark.parametrize(
    "requests",
    [
        [{"messages": [MARKER]}],
        [MARKER],
        [{"messages": MARKER}],
        [{"messages": {"first": _system(MARKER)}}],
    ],
    ids=["string-message", "string-request", "string-messages", "object-messages"],
)
def test_a_request_body_of_the_wrong_shape_is_refused(requests: list[Any]) -> None:
    with pytest.raises(GateFailure) as exc:
        _arm({"a": requests}, "a")
    assert exc.value.lines[0].startswith(f"arm a: {Path('a') / 'sink' / '0000.request.json'}: ")
