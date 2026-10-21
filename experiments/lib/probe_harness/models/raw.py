"""The base of every provider format, and the one route to a body's source JSON.

A provider model declares the fields a gate reads, and nothing reads a provider body any
other way. Two rules follow:

- **Raw access goes through `RawJson`.** A model keeps the JSON it was validated from, and
  exposes it only as `raw`, which can be searched but never read by key. A field a gate
  needs by name gets declared, so a misspelling is a validation error rather than a silent
  absence.
- **A field is required unless a provider omits it.** Only where real captures, or a
  documented provider behaviour, show a field missing does it default to `MISSING`, with a
  comment naming the evidence. A body that fails its model is refused, naming its file and
  the field's path, and the run's workspace is kept.

Unknown keys are ignored: a provider adds fields freely, and no gate reads one it has not
declared.
"""

import json
from collections.abc import Iterator, Sequence
from typing import Any, Self, final

from pydantic import (
    BaseModel,
    ConfigDict,
    Discriminator,
    GetCoreSchemaHandler,
    JsonValue,
    ModelWrapValidatorHandler,
    PrivateAttr,
    Tag,
    model_validator,
)
from pydantic_core import CoreSchema, core_schema


@final
class RawJson:
    """A JSON value that can be searched but never read by key.

    Also a pydantic field type, for a value a gate searches without reading any part of it
    by name: a request's `system`, a message's `content`, a tool's `properties`.
    """

    def __init__(self, value: JsonValue) -> None:
        self._value = value

    def __repr__(self) -> str:
        return f"RawJson({self._value!r})"

    def contains(self, needle: str) -> bool:
        """Whether <needle> appears in the value's compact serialization.

        Keys and JSON punctuation match too, so this is not the marker search: a marker
        counts only in a string value, which is what `strings()` yields. A value nested past
        Python's recursion limit raises `RecursionError`.
        """
        return needle in json.dumps(self._value, separators=(",", ":"), ensure_ascii=False)

    def strings(self) -> Iterator[str]:
        """Every string leaf, in document order; keys are not leaves.

        Walked with an explicit stack, so a body nested past Python's recursion limit is
        searched like any other.
        """
        stack: list[JsonValue] = [self._value]
        while stack:
            match stack.pop():
                case str() as text:
                    yield text
                case list() as items:
                    stack.extend(reversed(items))
                case dict() as fields:
                    stack.extend(reversed(list(fields.values())))
                case _:
                    pass

    def keys(self) -> frozenset[str]:
        """An object's keys; empty for any other value."""
        return frozenset(self._value) if isinstance(self._value, dict) else frozenset()

    @classmethod
    def __get_pydantic_core_schema__(cls, source: Any, handler: GetCoreSchemaHandler) -> CoreSchema:
        return core_schema.no_info_plain_validator_function(cls)


class ProviderModel(BaseModel):
    """The base of every provider format; see the module docstring for its two rules."""

    model_config = ConfigDict(extra="ignore", strict=True, frozen=True)

    _source: JsonValue = PrivateAttr()

    @property
    def raw(self) -> RawJson:
        """The JSON this model was validated from, searchable but not readable by key."""
        return RawJson(self._source)

    @model_validator(mode="wrap")
    @classmethod
    def _keep_source(cls, data: Any, handler: ModelWrapValidatorHandler[Self]) -> Self:
        model = handler(data)
        model._source = data
        return model


def tag(name: str) -> Tag:
    """A union member's tag, bracketed so `field_path` can drop it from an error location."""
    return Tag(f"<{name}>")


def type_is(name: str) -> Discriminator:
    """Tag a block whose `type` is <name> as `tag(name)`, and any other as `tag("other")`.

    For a block list where a gate reads one kind of block and only steps over the rest.
    """

    def kind(value: Any) -> str:
        block_type = value.get("type") if isinstance(value, dict) else getattr(value, "type", None)
        return f"<{name}>" if block_type == name else "<other>"

    return Discriminator(kind)


def _string_or_list(value: Any) -> str:
    return "<string>" if isinstance(value, str) else "<list>"


# Tags a field that is a string or a list as `tag("string")` or `tag("list")`. Discriminated
# rather than a plain union, so a body failing one branch is refused for that branch alone.
STRING_OR_LIST = Discriminator(_string_or_list)


def field_path(loc: Sequence[int | str]) -> str:
    """A validation error's location as the body spells it: union tags dropped, `.`-joined."""
    return ".".join(
        str(part) for part in loc if not (isinstance(part, str) and part.startswith("<"))
    )
