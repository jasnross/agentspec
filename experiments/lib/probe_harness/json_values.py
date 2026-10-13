"""The harness's own reading and writing of JSON values."""

import json
from typing import Any


def to_text(value: Any) -> str:
    """A JSON value as display text: a string as-is, anything else as compact JSON."""
    if isinstance(value, str):
        return value
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False)
