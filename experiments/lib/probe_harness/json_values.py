"""The harness's own reading and writing of JSON values.

Plain `json`, with two rules a JSON document needs and Python's defaults do not give: the
loaders refuse `NaN`, `Infinity` and a number too large to be finite, none of which JSON can
carry, and `equal` never counts a boolean as equal to a number, which Python's `True == 1`
would.
"""

import json
from typing import Any

type JsonValue = bool | int | float | str | list["JsonValue"] | dict[str, "JsonValue"] | None

_WHITESPACE = " \t\n\r"


def loads(text: str) -> JsonValue:
    """The one JSON value in <text>; `ValueError` when it is not exactly one."""
    value: JsonValue = json.loads(text, parse_constant=_refuse_constant, parse_float=_finite_float)
    return value


def load_stream(text: str) -> list[JsonValue]:
    """Every whitespace-separated JSON value in <text>; `ValueError` when it is not a stream."""
    decoder = json.JSONDecoder(parse_constant=_refuse_constant, parse_float=_finite_float)
    values: list[JsonValue] = []
    index = 0
    while True:
        while index < len(text) and text[index] in _WHITESPACE:
            index += 1
        if index == len(text):
            return values
        value, index = decoder.raw_decode(text, index)
        values.append(value)


def dumps(value: JsonValue) -> str:
    """<value> pretty-printed with two-space indentation, with no trailing newline."""
    return json.dumps(value, indent=2, ensure_ascii=False, allow_nan=False)


def equal(a: JsonValue, b: JsonValue) -> bool:
    """Structural equality: key order is irrelevant, and a boolean never equals a number."""
    if isinstance(a, bool) or isinstance(b, bool):
        return type(a) is type(b) and a == b
    if isinstance(a, int | float) and isinstance(b, int | float):
        return a == b
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(equal(x, y) for x, y in zip(a, b, strict=True))
    if isinstance(a, dict) and isinstance(b, dict):
        return a.keys() == b.keys() and all(equal(a[k], b[k]) for k in a)
    return type(a) is type(b) and a == b


def to_text(value: Any) -> str:
    """A JSON value as display text: a string as-is, anything else as compact JSON."""
    if isinstance(value, str):
        return value
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False)


def _refuse_constant(name: str) -> None:
    raise ValueError(f"{name} is not a JSON value")


def _finite_float(text: str) -> float:
    value = float(text)
    if value in (float("inf"), float("-inf")):
        raise ValueError(f"{text} is too large to be a finite number")
    return value
