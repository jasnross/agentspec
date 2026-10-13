"""The views a projection reads, built in `views.py` from what a run left on disk."""

import json
from pathlib import Path

import pytest
from probe_harness import views
from probe_harness.families import claude_otel
from probe_harness.families.claude_otel import ClaudeArm, ClaudeOtelProbe
from probe_harness.gates import GateFailure

PROBE = ClaudeOtelProbe(
    model="m",
    budget_usd="0.10",
    marker="M",
    arms=(ClaudeArm("a", "tree", "p"), ClaudeArm("b", "tree", "p")),
)


def _sink(ws: Path, arm: str, **files: str) -> Path:
    sink = ws / arm / "sink"
    sink.mkdir(parents=True)
    for name, text in files.items():
        (sink / name.replace("_", ".")).write_text(text, encoding="utf-8")
    return sink


def test_a_sink_with_no_request_file_is_the_family_s_refusal(tmp_path: Path) -> None:
    _sink(tmp_path, "a", one_request_json='{"x":1}\n')
    _sink(tmp_path, "b")

    with pytest.raises(GateFailure) as exc:
        claude_otel.judge(tmp_path, PROBE)
    assert exc.value.lines == (f"the b arm captured no request files in {tmp_path}/b/sink",)


def test_only_request_json_files_count_as_requests(tmp_path: Path) -> None:
    # The sink writes `<uuid>.response.json` beside each request. A size check or a bare
    # `*.json` glob would read a response-only sink as a populated arm.
    sink = _sink(tmp_path, "a", one_response_json='{"x":1}\n')

    values = views.read_sink(sink)
    assert values.requests == []
    assert values.responses == [{"x": 1}]
    _sink(tmp_path, "b", two_request_json="{}\n")
    with pytest.raises(GateFailure) as exc:
        claude_otel.judge(tmp_path, PROBE)
    assert "a arm captured no request files" in exc.value.lines[0]


def test_the_view_maps_each_arm_to_its_parsed_bodies(tmp_path: Path) -> None:
    a = _sink(tmp_path, "a", one_request_json='{"n":1}\n', two_request_json='{"n":2}\n')
    b = _sink(tmp_path, "b", three_request_json='{"n":3}\n', three_response_json='{"n":99}\n')
    out = tmp_path / "view.json"

    views.write_arm_view(out, {"a": views.read_sink(a).requests, "b": views.read_sink(b).requests})

    # Parsed bodies, not strings — the projection indexes into them.
    assert json.loads(out.read_text(encoding="utf-8")) == {
        "a": [{"n": 1}, {"n": 2}],
        "b": [{"n": 3}],
    }


def test_a_sink_file_holding_two_concatenated_values_contributes_both(tmp_path: Path) -> None:
    sink = _sink(tmp_path, "a", one_request_json='{"n":1}{"n":2}\n', two_request_json='{"n":3}')

    assert views.read_sink(sink).requests == [{"n": 1}, {"n": 2}, {"n": 3}]


def test_a_sink_file_that_is_not_json_is_refused_naming_the_file(tmp_path: Path) -> None:
    sink = _sink(tmp_path, "a", one_request_json="{not json\n")

    with pytest.raises(GateFailure) as exc:
        views.read_sink(sink)
    assert exc.value.lines[0].startswith(f"{sink}/one.request.json is not valid JSON: ")


def test_a_capture_view_is_the_list_of_payloads(tmp_path: Path) -> None:
    payloads = tmp_path / "payloads.jsonl"
    payloads.write_text('{"a":1}\n{"b":2}\n', encoding="utf-8")
    out = tmp_path / "view.json"

    views.write_capture_view(payloads, out)
    assert json.loads(out.read_text(encoding="utf-8")) == [{"a": 1}, {"b": 2}]


def test_a_capture_view_over_payloads_that_are_not_json_is_refused(tmp_path: Path) -> None:
    payloads = tmp_path / "payloads.jsonl"
    payloads.write_text('{"a":1}\n{oops\n', encoding="utf-8")

    with pytest.raises(GateFailure) as exc:
        views.write_capture_view(payloads, tmp_path / "view.json")
    assert exc.value.lines[0].startswith(
        f"the captured payloads at {payloads} are not valid JSON: "
    )


def test_a_view_carries_an_escaped_lone_surrogate_through(tmp_path: Path) -> None:
    sink = _sink(tmp_path, "a", one_request_json='{"s":"cut \\ud83d"}\n')
    out = tmp_path / "view.json"

    views.write_arm_view(out, {"a": views.read_sink(sink).requests})
    assert "\\ud83d" in out.read_text(encoding="utf-8")
