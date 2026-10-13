"""jq's evaluation semantics, in Python, for the gates that read a view or a sink.

The gates translate jq programs literally, so these helpers reproduce what jq does where it
differs from Python: `.key` on a non-object is an error but on `null` is `null`; `.[]?`
yields nothing where `.[]` would fail; booleans never equal numbers; `unique` sorts. A jq
runtime error made a bash gate fail, so each helper raises where jq would, and the gate
turns any of `EVALUATION_ERRORS` into its own refusal.
"""

import re
from collections.abc import Iterator, Sequence
from typing import Any

from probe_harness.json_values import to_text

# What evaluating a gate can raise: a jq type error's equivalents, a malformed file, an
# unreadable one, or a document nested past Python's recursion limit.
EVALUATION_ERRORS = (KeyError, IndexError, TypeError, ValueError, OSError, RecursionError)


_PATH_STEP = re.compile(r"\.([A-Za-z_][A-Za-z0-9_]*)|\[\]")


def path_outputs(value: Any, path: str) -> list[Any]:
    """The outputs of a jq path built from `.key` and `[]` steps, such as `.messages[].text`."""
    outputs = [value]
    position = 0
    while position < len(path):
        step = _PATH_STEP.match(path, position)
        if step is None:
            raise ValueError(f"unsupported field path: {path}")
        key = step.group(1)
        if key is not None:
            outputs = [index(o, key) for o in outputs]
        else:
            outputs = [item for o in outputs for item in each(o)]
        position = step.end()
    return outputs


def entries(value: Any) -> list[tuple[str, Any]]:
    """`to_entries[]` over an object, in key order."""
    if not isinstance(value, dict):
        raise TypeError("to_entries needs an object")
    return list(value.items())


def index(value: Any, key: str) -> Any:
    """`.key`: `null` through `null`, a member of an object, an error otherwise."""
    if value is None:
        return None
    if isinstance(value, dict):
        return value.get(key)
    raise TypeError(f"cannot index {type_of(value)} with {key!r}")


def try_index(value: Any, key: str) -> Any:
    """`.key?`: as `index`, but an error yields nothing, which `//` then replaces."""
    try:
        return index(value, key)
    except TypeError:
        return None


def each(value: Any) -> list[Any]:
    """`.[]`: an array's elements or an object's values; an error otherwise."""
    if isinstance(value, list):
        return value
    if isinstance(value, dict):
        return list(value.values())
    raise TypeError(f"cannot iterate over {type_of(value)}")


def each_opt(value: Any) -> list[Any]:
    """`.[]?`: as `each`, but nothing where iteration would fail."""
    try:
        return each(value)
    except TypeError:
        return []


def has(value: Any, key: str) -> bool:
    if isinstance(value, dict):
        return key in value
    raise TypeError(f"cannot check whether {type_of(value)} has a key")


def alternative(value: Any, default: Any) -> Any:
    return default if value is None or value is False else value


def startswith(value: Any, prefix: str) -> bool:
    if not isinstance(value, str):
        raise TypeError("startswith() requires string inputs")
    return value.startswith(prefix)


def strings(value: Any) -> Iterator[str]:
    """`.. | strings`: every string value, recursively, in pre-order. Keys are not values."""
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)
    elif isinstance(value, dict):
        for item in value.values():
            yield from strings(item)


def flatten(values: list[Any]) -> list[Any]:
    out: list[Any] = []
    for value in values:
        if isinstance(value, list):
            out.extend(flatten(value))
        else:
            out.append(value)
    return out


def join(items: list[Any]) -> str:
    """`join("")`: `null` contributes nothing, scalars their JSON text; containers fail."""
    parts = []
    for item in items:
        if item is None:
            continue
        if isinstance(item, str):
            parts.append(item)
        elif isinstance(item, list | dict):
            raise TypeError("cannot join a container")
        else:
            parts.append(to_text(item))
    return "".join(parts)


def subtract(left: Sequence[Any], right: Sequence[Any]) -> list[Any]:
    """Array subtraction: every element of <left> equal to none of <right>."""
    return [a for a in left if not any(equal(a, b) for b in right)]


def unique(values: Sequence[Any]) -> list[Any]:
    """`unique`, by jq's structural equality. Order is irrelevant to every caller but one,
    which sorts strings, where jq's order is codepoint order."""
    out: list[Any] = []
    for value in values:
        if not any(equal(value, seen) for seen in out):
            out.append(value)
    if all(isinstance(v, str) for v in out):
        out.sort()
    return out


def type_of(value: Any) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "boolean"
    if isinstance(value, int | float):
        return "number"
    if isinstance(value, str):
        return "string"
    if isinstance(value, list):
        return "array"
    if isinstance(value, dict):
        return "object"
    raise ValueError("not a JSON value")


def equal(a: Any, b: Any) -> bool:
    """jq's `==`: structural, with booleans never equal to numbers."""
    kind = type_of(a)
    if kind != type_of(b):
        return False
    if kind == "array":
        return len(a) == len(b) and all(equal(x, y) for x, y in zip(a, b, strict=True))
    if kind == "object":
        return a.keys() == b.keys() and all(equal(a[k], b[k]) for k in a)
    return bool(a == b)
