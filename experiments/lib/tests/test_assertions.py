"""`assertions.py` orders JSON values exactly as jq does, checked against jq itself."""

import json
import subprocess

import pytest
from probe_harness import json_values
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


def _jq(program: str, value: JsonValue) -> JsonValue:
    """jq's `-c <program>` over <value>: the reference these orderings are checked against."""
    completed = subprocess.run(
        ["jq", "-c", program],
        input=json.dumps(value),
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=True,
    )
    return json_values.loads(completed.stdout)


def test_sorted_values_is_jq_s_sort() -> None:
    assert sorted_values(VALUES) == _jq("sort", VALUES)


def test_unique_is_jq_s_unique() -> None:
    assert unique(VALUES) == _jq("unique", VALUES)


def test_object_keys_sort_as_jq_s_keys() -> None:
    keys = ["b", "a", "é", "Z", "😀", "￿", "aa"]
    document: dict[str, JsonValue] = {k: 1 for k in keys}
    assert sorted(document) == _jq("keys", document)


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
