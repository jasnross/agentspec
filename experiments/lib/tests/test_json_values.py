"""The harness's JSON reading, writing and comparison in `json_values.py`."""

import pytest
from probe_harness import json_values


def test_a_boolean_never_equals_a_number() -> None:
    assert not json_values.equal(True, 1)
    assert not json_values.equal(1, True)
    assert not json_values.equal([False], [0])


def test_equality_ignores_key_order() -> None:
    assert json_values.equal({"a": 1, "b": [2]}, {"b": [2], "a": 1})


def test_an_integer_equals_the_same_float() -> None:
    assert json_values.equal(1, 1.0)


def test_equality_compares_structure() -> None:
    assert not json_values.equal({"a": 1}, {"a": 1, "b": None})
    assert not json_values.equal([1, 2], [2, 1])
    assert not json_values.equal("1", 1)
    assert json_values.equal(None, None)


@pytest.mark.parametrize("text", ["NaN", "Infinity", "-Infinity", '{"a": NaN}'])
def test_loads_refuses_a_non_finite_constant(text: str) -> None:
    with pytest.raises(ValueError):
        json_values.loads(text)


def test_load_stream_reads_concatenated_objects_without_whitespace() -> None:
    assert json_values.load_stream('{"a":1}{"b":2}') == [{"a": 1}, {"b": 2}]


def test_load_stream_reads_concatenated_objects_with_whitespace() -> None:
    assert json_values.load_stream('{"a":1}\n  {"b":2}\n') == [{"a": 1}, {"b": 2}]


def test_load_stream_refuses_trailing_garbage() -> None:
    with pytest.raises(ValueError):
        json_values.load_stream('{"a":1}\n}')


def test_dumps_pretty_prints_without_escaping_non_ascii() -> None:
    assert json_values.dumps({"a": ["é"]}) == '{\n  "a": [\n    "é"\n  ]\n}'


def test_to_text_leaves_a_string_and_compacts_anything_else() -> None:
    assert json_values.to_text("é") == "é"
    assert json_values.to_text({"a": [1, "é"]}) == '{"a":[1,"é"]}'


def test_load_stream_refuses_a_non_finite_constant_mid_stream() -> None:
    with pytest.raises(ValueError):
        json_values.load_stream('{"a":1}\nNaN\n')


@pytest.mark.parametrize("text", ["1e400", "-1E+400", "[1e999]"])
def test_the_loaders_refuse_a_number_too_large_to_be_finite(text: str) -> None:
    with pytest.raises(ValueError):
        json_values.loads(text)
    with pytest.raises(ValueError):
        json_values.load_stream(text)
