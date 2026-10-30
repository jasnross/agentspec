"""`assertions.py` orders JSON values exactly as jq does, checked against jq itself."""

import json
from pathlib import Path

import pytest
from probe_harness import jq_cli, json_values
from probe_harness.assertions import NO_GOVERNED_REQUEST, consistent, sorted_values, unique
from probe_harness.json_values import JsonValue

VALUES: list[JsonValue] = [
    {"b": 1},
    "b",
    None,
    [1, None],
    True,
    {"a": 2, "b": 0},
    2,
    "a",
    False,
    [1],
    1.5,
    {"a": 1},
    "é",
    "z",
    [],
    {"a": 1},
    "a",
    [1, None],
    {},
    -3,
    "Ab",
    [None, 1],
    {"a": [1]},
    "😀",
    "￿",
]


def _jq(expr: str, values: list[JsonValue], tmp_path: Path) -> JsonValue:
    path = tmp_path / "values.json"
    path.write_text(json.dumps(values), encoding="utf-8")
    return json_values.loads(jq_cli.evaluate(expr, path, compact=True))


def test_sorted_values_is_jq_s_sort(tmp_path: Path) -> None:
    assert sorted_values(VALUES) == _jq("sort", VALUES, tmp_path)


def test_unique_is_jq_s_unique(tmp_path: Path) -> None:
    assert unique(VALUES) == _jq("unique", VALUES, tmp_path)


def test_object_keys_sort_as_jq_s_keys(tmp_path: Path) -> None:
    keys = ["b", "a", "é", "Z", "😀", "￿", "aa"]
    document = {k: 1 for k in keys}
    path = tmp_path / "doc.json"
    path.write_text(json.dumps(document), encoding="utf-8")
    assert sorted(document) == json_values.loads(jq_cli.evaluate("keys", path, compact=True))


@pytest.mark.parametrize(
    ("values", "want"),
    [
        ([], NO_GOVERNED_REQUEST),
        ([["a"]], ["a"]),
        ([["a"], []], {"inconsistent": [["a"], []]}),
    ],
    ids=repr,
)
def test_consistent(values: list[JsonValue], want: JsonValue) -> None:
    assert consistent(values) == want
