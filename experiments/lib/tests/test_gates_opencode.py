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
    return bodies.opencode_arms({arm: SinkValues(requests=entries, responses=[])})[arm].requests


def _system(text: str) -> dict[str, Any]:
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


def test_governed_requests_selects_the_requests_marked_in_a_system_message() -> None:
    marked = {"messages": [_system(f"x {MARKER} y")]}
    title = {"messages": [_system("make a title"), {"role": "user", "content": MARKER}]}
    requests = _arm({"a": [title, marked]}, "a")

    assert gates.governed_requests(requests, MARKER) == [requests[1]]


def _tools(*names: str) -> list[dict[str, Any]]:
    return [{"type": "function", "function": {"name": name}} for name in names]


def test_toolset_is_the_sorted_names_every_governed_request_offers() -> None:
    requests = _arm(
        {
            "a": [
                {"messages": [_system(MARKER)], "tools": _tools("read", "bash")},
                {"messages": [_system(MARKER)], "tools": _tools("bash", "read")},
                # A title-generation request: unmarked, and offering no tools.
                {"messages": [_system("title")]},
            ]
        },
        "a",
    )

    assert gates.toolset(requests, MARKER) == ["bash", "read"]
    assert gates.toolset(requests, MARKER, keep=lambda name: name != "bash") == ["read"]


def test_toolset_reports_governed_requests_that_differ_and_an_arm_without_one() -> None:
    differing = _arm(
        {
            "a": [
                {"messages": [_system(MARKER)], "tools": _tools("read")},
                # A request offering no tools omits the key.
                {"messages": [_system(MARKER)]},
            ]
        },
        "a",
    )
    ungoverned = _arm({"a": [{"messages": [_system("title")], "tools": _tools("read")}]}, "a")

    assert gates.toolset(differing, MARKER) == {"inconsistent": [[], ["read"]]}
    assert gates.toolset(ungoverned, MARKER) == "arm-had-no-governed-request"


def test_toolset_merges_a_list_filtered_to_empty_with_a_request_offering_none() -> None:
    requests = _arm(
        {
            "a": [
                {"messages": [_system(MARKER)], "tools": _tools("read", "bash")},
                {"messages": [_system(MARKER)]},
            ]
        },
        "a",
    )

    assert gates.toolset(requests, MARKER, keep=lambda name: name.startswith("fx_")) == []


def test_toolset_keeps_a_name_one_request_offers_twice() -> None:
    # jq's `sort` keeps duplicates; only `unique` across requests merges.
    requests = _arm(
        {"a": [{"messages": [_system(MARKER)], "tools": _tools("fx_b", "fx_a", "fx_b")}]}, "a"
    )

    assert gates.toolset(requests, MARKER) == ["fx_a", "fx_b", "fx_b"]


def test_tool_results_are_the_governed_requests_tool_messages_in_order() -> None:
    requests = _arm(
        {
            "a": [
                {"messages": [_system(MARKER), {"role": "tool", "content": "first"}]},
                {"messages": [_system("title"), {"role": "tool", "content": "unmarked"}]},
                {"messages": [_system(MARKER), {"role": "tool", "content": "second"}]},
            ]
        },
        "a",
    )

    assert gates.tool_results(requests, MARKER) == ["first", "second"]


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
