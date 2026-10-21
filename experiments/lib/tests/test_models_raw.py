"""`RawJson` and `ProviderModel`, the one route to a provider body's source JSON."""

from typing import Any

from probe_harness.models.raw import ProviderModel, RawJson
from pydantic import JsonValue


class _Body(ProviderModel):
    name: str


def test_contains_matches_inside_a_nested_string() -> None:
    assert RawJson({"a": [{"b": "x NEEDLE y"}]}).contains("NEEDLE")


def test_contains_matches_inside_a_key() -> None:
    assert RawJson({"a": {"NEEDLE": 1}}).contains("NEEDLE")


def test_contains_misses_what_is_absent() -> None:
    assert not RawJson({"a": "b"}).contains("NEEDLE")


def test_strings_yields_the_leaves_in_document_order_and_no_keys() -> None:
    value: JsonValue = {"k1": "a", "k2": ["b", {"k3": "c"}, 1, None, True], "k4": "d"}
    assert list(RawJson(value).strings()) == ["a", "b", "c", "d"]


def test_strings_survives_nesting_past_the_recursion_limit() -> None:
    nested: Any = "deep"
    for _ in range(5000):
        nested = [nested]
    assert list(RawJson(nested).strings()) == ["deep"]


def test_keys_are_an_object_s_keys() -> None:
    assert RawJson({"a": 1, "b": 2}).keys() == frozenset({"a", "b"})


def test_keys_are_empty_for_a_non_object() -> None:
    assert RawJson([{"a": 1}]).keys() == frozenset()
    assert RawJson("a").keys() == frozenset()


def test_raw_json_cannot_be_read_by_key() -> None:
    assert not hasattr(RawJson, "__getitem__")


def test_a_provider_model_keeps_its_source_reachable_only_as_raw() -> None:
    model = _Body.model_validate({"name": "n", "undeclared": "kept"})
    assert model.raw.contains("undeclared")
    assert list(model.raw.strings()) == ["n", "kept"]
    assert not hasattr(model, "source")
    assert not hasattr(model, "undeclared")
