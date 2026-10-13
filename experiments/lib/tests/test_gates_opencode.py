"""The OpenCode fake-provider gates, over fabricated views and workspaces."""

import json
from pathlib import Path
from typing import Any

import pytest
from probe_harness.gates import GateFailure
from probe_harness.gates import opencode as gates

MARKER = "AGENTSPEC-PROBE-MARKER-TEST"


def _view(tmp_path: Path, arms: dict[str, list[dict[str, Any]]]) -> Path:
    view = tmp_path / "view.json"
    view.write_text(json.dumps(arms), encoding="utf-8")
    return view


def _system(text: Any) -> dict[str, Any]:
    return {"role": "system", "content": text}


def test_governed_passes_on_a_marked_system_message(tmp_path: Path) -> None:
    view = _view(tmp_path, {"a": [{"messages": [_system(f"x {MARKER} y")]}]})
    gates.governed(view, "a", MARKER, tmp_path / "a" / "stderr")


def test_governed_fails_on_a_view_whose_marker_sits_only_in_a_user_message(
    tmp_path: Path,
) -> None:
    view = _view(
        tmp_path,
        {"a": [{"messages": [_system("plain"), {"role": "user", "content": MARKER}]}]},
    )
    with pytest.raises(GateFailure) as failure:
        gates.governed(view, "a", MARKER, tmp_path / "a" / "stderr")
    assert failure.value.lines == (
        f"probe: arm a: no request carried {MARKER} in a system message.",
        f"probe: OpenCode may have fallen back to its default agent; see {tmp_path}/a/stderr.",
        gates.APPARATUS_FAILURE,
    )


def test_governed_matches_a_marker_inside_structured_content(tmp_path: Path) -> None:
    view = _view(tmp_path, {"a": [{"messages": [_system([{"type": "text", "text": MARKER}])]}]})
    gates.governed(view, "a", MARKER, tmp_path / "stderr")


def test_governed_fails_for_an_arm_missing_from_the_view(tmp_path: Path) -> None:
    view = _view(tmp_path, {"a": [{"messages": [_system(MARKER)]}]})
    with pytest.raises(GateFailure):
        gates.governed(view, "b", MARKER, tmp_path / "stderr")


def test_tool_result_passes_when_a_governed_request_carries_a_tool_message(
    tmp_path: Path,
) -> None:
    view = _view(
        tmp_path,
        {"a": [{"messages": [_system(MARKER), {"role": "tool", "content": "r"}]}]},
    )
    gates.tool_result(view, "a", MARKER, tmp_path / "stderr")


def test_tool_result_fails_when_no_governed_request_has_a_tool_message(tmp_path: Path) -> None:
    view = _view(
        tmp_path,
        {
            "a": [
                {"messages": [_system(MARKER), {"role": "user", "content": "hi"}]},
                # A tool message on an unmarked request does not count.
                {"messages": [_system("title"), {"role": "tool", "content": "r"}]},
            ]
        },
    )
    with pytest.raises(GateFailure) as failure:
        gates.tool_result(view, "a", MARKER, tmp_path / "a" / "stderr")
    assert failure.value.lines[0] == "probe: arm a: no marked request carried a tool result."
    assert "the scripted call" in failure.value.lines[1]


def test_stamp_fails_for_a_missing_stamp(tmp_path: Path) -> None:
    (tmp_path / "a").mkdir()
    with pytest.raises(GateFailure) as failure:
        gates.stamp(tmp_path, "a", "fx_extra")
    assert failure.value.lines == (
        "probe: arm a: the fx_extra server never answered tools/list, so the arm describes "
        "nothing.",
        gates.APPARATUS_FAILURE,
    )


def test_stamp_passes_when_the_stamp_exists(tmp_path: Path) -> None:
    (tmp_path / "a").mkdir()
    (tmp_path / "a" / "fx.stamp").write_text("tools/list\n", encoding="utf-8")
    gates.stamp(tmp_path, "a", "fx")


@pytest.mark.parametrize(
    "requests",
    [
        # `.role` on a string message is a jq error, so the gate refuses.
        [{"messages": [MARKER]}],
        # `.messages[]?` does not suppress indexing a non-object request.
        [MARKER],
        # A non-iterable `messages` yields nothing, so nothing is governed.
        [{"messages": MARKER}],
    ],
    ids=["string-message", "string-request", "string-messages"],
)
def test_governed_refuses_where_jq_would_error_or_find_nothing(
    tmp_path: Path, requests: list[Any]
) -> None:
    view = _view(tmp_path, {"a": requests})
    with pytest.raises(GateFailure):
        gates.governed(view, "a", MARKER, tmp_path / "stderr")


def test_governed_iterates_an_object_shaped_messages_field(tmp_path: Path) -> None:
    view = _view(tmp_path, {"a": [{"messages": {"first": _system(MARKER)}}]})
    gates.governed(view, "a", MARKER, tmp_path / "stderr")
