"""What an assertion needs to reproduce jq's view of JSON in Python.

jq orders every JSON value totally, and `sort`, `unique` and `keys` all lean on that order.
A Python assertion replacing a jq projection must order values the same way, or it yields
the same elements in a different order and compares unequal to a record written by jq.
"""

import re
from collections.abc import Sequence
from functools import cmp_to_key
from typing import Any

from probe_harness.json_values import JsonValue

# The value an arm with no governed request yields, so such an arm reads as itself rather
# than as an empty tool set.
NO_GOVERNED_REQUEST = "arm-had-no-governed-request"


def sort_key(value: JsonValue) -> Any:
    """A key ordering JSON values as jq does.

    `null < false < true < numbers < strings < arrays < objects`; arrays compare
    elementwise, and objects by their sorted key lists, then by their values in key order.
    """
    return cmp_to_key(_compare)(value)


def sorted_values(values: Sequence[JsonValue]) -> list[JsonValue]:
    """jq's `sort`: every value, duplicates kept, in jq's order."""
    return sorted(values, key=sort_key)


def unique(values: Sequence[JsonValue]) -> list[JsonValue]:
    """jq's `unique`: the distinct values, in jq's order."""
    distinct: list[JsonValue] = []
    for value in sorted_values(values):
        if not distinct or _compare(distinct[-1], value) != 0:
            distinct.append(value)
    return distinct


def consistent(values: Sequence[JsonValue]) -> JsonValue:
    """`NO_GOVERNED_REQUEST` for no values, the value for one, else `{"inconsistent": values}`."""
    if not values:
        return NO_GOVERNED_REQUEST
    if len(values) == 1:
        return values[0]
    return {"inconsistent": list(values)}


def _rank(value: JsonValue) -> int:
    match value:
        case None:
            return 0
        case False:
            return 1
        case True:
            return 2
        case int() | float():
            return 3
        case str():
            return 4
        case list():
            return 5
        case dict():
            return 6
    raise TypeError(f"a {type(value).__name__} is not a JSON value")


def _compare(a: JsonValue, b: JsonValue) -> int:
    rank_a, rank_b = _rank(a), _rank(b)
    if rank_a != rank_b:
        return -1 if rank_a < rank_b else 1
    if isinstance(a, list) and isinstance(b, list):
        for x, y in zip(a, b, strict=False):
            if (order := _compare(x, y)) != 0:
                return order
        return (len(a) > len(b)) - (len(a) < len(b))
    if isinstance(a, dict) and isinstance(b, dict):
        keys_a, keys_b = sorted(a), sorted(b)
        if (order := _compare(list(keys_a), list(keys_b))) != 0:
            return order
        for key in keys_a:
            if (order := _compare(a[key], b[key])) != 0:
                return order
        return 0
    if isinstance(a, int | float) and isinstance(b, int | float) and not isinstance(a, bool):
        return (a > b) - (a < b)
    if isinstance(a, str) and isinstance(b, str):
        return _compare_strings(a, b)
    return 0


def _compare_strings(a: str, b: str) -> int:
    """jq compares strings by their UTF-8 bytes, which orders astral characters as code points.

    jq reads a lone surrogate (a string cut between the halves of an emoji) as U+FFFD, so it
    is compared as one.
    """
    left, right = _jq_text(a).encode("utf-8"), _jq_text(b).encode("utf-8")
    return (left > right) - (left < right)


_SURROGATE = re.compile("[\ud800-\udfff]")


def _jq_text(text: str) -> str:
    return _SURROGATE.sub("\ufffd", text)
