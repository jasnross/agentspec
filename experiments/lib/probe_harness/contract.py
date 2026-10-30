"""The harness's own formats, the manifest and the record, each declared once.

A manifest is hand-authored, and the failures it can express are exactly the ones the
harness exists to prevent: an absent `expected` compares `null == null` and records a
vacuous `confirmed`. `parse_manifest` validates the whole document against the manifest
version its `schema_version` names before anything reads a value out of it, so every
subcommand refuses the same manifest the same way, before any workspace exists. The
refusal lists every invalid field by path; the rules relating fields to each other (an
option set needs a manual driver, option ids are unique) run once every field is valid, and
name no single path.
A record is read back through `RecordV1` or `RecordV2`, the same declarations its writer
serializes.

Every harness format derives from `HarnessModel`: unknown keys are refused, and validation
is strict, because `true == 1` in Python and a document must not pass on that. Four rules
keep that strictness honest, each measured against pydantic 2.14:

- A number or bool field is a constrained `int` or `bool`, never a `Literal`: strict
  `Literal[1]` accepts `true` and `1.0`, and `Literal[True, False]` accepts `1`.
- An enum field opts out of strict mode with `Field(strict=False)`, because strict
  Python-mode validation refuses the string a document carries. A string outside the enum
  is still refused.
- A tuple field opts out of strict mode the same way, because strict mode refuses the list
  a document carries. Its elements stay strict.
- A document is loaded with `json_values.loads` and validated in Python mode, because
  pydantic's JSON mode accepts `NaN` and `1e400` into a `JsonValue`.
"""

from enum import StrEnum
from pathlib import Path
from typing import Annotated, Any, Literal, Self

from pydantic import (
    MISSING,
    BaseModel,
    ConfigDict,
    Discriminator,
    Field,
    JsonValue,
    PrivateAttr,
    Tag,
    TypeAdapter,
    ValidationError,
    field_validator,
    model_validator,
)
from pydantic_core import ErrorDetails

from probe_harness import json_values


class Provider(StrEnum):
    CLAUDE = "claude"
    CURSOR = "cursor"
    OPENCODE = "opencode"


class Driver(StrEnum):
    """What `probe run` must have before it can execute a package.

    Nothing, an explicit opt-in because the run spends model quota, or a person present.
    Ordered by escalating requirement, and mutually exclusive because only the strictest
    requirement governs scheduling. It deliberately does not encode who *answers* the
    question; that is the assertion's shape.
    """

    UNATTENDED = "unattended"
    BILLED = "billed"
    MANUAL = "manual"


# The drivers in escalating order. The run loop derives both its authorization set and
# its per-driver reporting from this, so a fourth driver is one entry here plus its flag.
DRIVERS: tuple[Driver, ...] = (Driver.UNATTENDED, Driver.BILLED, Driver.MANUAL)

# The driver a run authorizes for free. Every other driver costs something, money or a
# person's session, so it has to be asked for by name.
FREE_DRIVER = Driver.UNATTENDED


class Depth(StrEnum):
    """The depths a probe can gather evidence for.

    `emitted-bytes` is the hop agentspec's own tests cover, and `provider-parses` would
    assert that no error occurred, which the contract forbids. Neither is a depth, so a
    record can never claim one.
    """

    RESOLVED_CONFIG = "resolved-config"
    OUTBOUND_REQUEST = "outbound-request"


class Status(StrEnum):
    CONFIRMED = "confirmed"
    REFUTED = "refuted"
    INCONCLUSIVE = "inconclusive"


class HarnessModel(BaseModel):
    """The base of every format the harness itself defines; see the module docstring."""

    model_config = ConfigDict(extra="forbid", strict=True, frozen=True, allow_inf_nan=False)


type NonEmpty = Annotated[str, Field(min_length=1)]
# Not `Literal[1]`: strict `Literal` accepts `true` and `1.0`.
type SchemaV1 = Annotated[int, Field(ge=1, le=1)]
type SchemaV2 = Annotated[int, Field(ge=2, le=2)]
# A git object id: SHA-1, or SHA-256 in a repository that uses it.
type BlobId = Annotated[str, Field(pattern=r"^[0-9a-f]{40}([0-9a-f]{24})?$")]


class CommandSource(HarnessModel):
    kind: Literal["command"]
    # Split into argv and exec'd directly rather than through a shell: a manifest is
    # reviewed as data, so it must not be able to express arbitrary code.
    command: NonEmpty

    @field_validator("command")
    @classmethod
    def _one_line_of_words(cls, command: str) -> str:
        if len(command.splitlines()) != 1 or not command.split():
            raise ValueError("must be a non-empty, single-line string")
        return command

    @property
    def argv(self) -> tuple[str, ...]:
        return tuple(self.command.split())


class CaptureSourceV1(HarnessModel):
    kind: Literal["capture"]
    jq: NonEmpty


class CaptureSource(HarnessModel):
    """The capture names its own version, read by the definition's `capture_version`."""

    kind: Literal["capture"]


class NoSource(HarnessModel):
    kind: Literal["none"] = "none"


def _version_kind(value: Any) -> str | None:
    """The tag of a version source; a missing `kind` means `none`."""
    if isinstance(value, dict):
        kind = value.get("kind", "none")
        return kind if kind in ("command", "capture", "none") else None
    if isinstance(value, CommandSource | CaptureSourceV1 | CaptureSource | NoSource):
        return value.kind
    return None


_VERSION_SOURCE_DISCRIMINATOR = Discriminator(
    _version_kind,
    custom_error_type="version_source",
    custom_error_message="must be an object whose kind is command, capture, or none",
)

type VersionSourceV1 = Annotated[
    Annotated[CommandSource, Tag("command")]
    | Annotated[CaptureSourceV1, Tag("capture")]
    | Annotated[NoSource, Tag("none")],
    _VERSION_SOURCE_DISCRIMINATOR,
]

type VersionSource = Annotated[
    Annotated[CommandSource, Tag("command")]
    | Annotated[CaptureSource, Tag("capture")]
    | Annotated[NoSource, Tag("none")],
    _VERSION_SOURCE_DISCRIMINATOR,
]


class Option(HarnessModel):
    id: NonEmpty
    text: str
    # Any other declared status would fix the verdict in advance.
    status: Literal["inconclusive"] | MISSING = MISSING


class ProjectionAssertionV1(HarnessModel):
    projection: NonEmpty
    expected: JsonValue


class ExpectedAssertion(HarnessModel):
    """The value the definition's assertion must yield for the record to be `confirmed`."""

    expected: JsonValue


class OptionsAssertion(HarnessModel):
    options: Annotated[tuple[Option, ...], Field(strict=False)]
    expected: JsonValue

    # Not `Field(min_length=1)`: that also reports a list whose only option is invalid as
    # empty, beside the option's own error. An after-validator runs only on valid options.
    @field_validator("options")
    @classmethod
    def _non_empty(cls, options: tuple[Option, ...]) -> tuple[Option, ...]:
        if not options:
            raise ValueError("must be a non-empty list")
        return options


def _assertion_shape_v1(value: Any) -> str | None:
    """The tag of a v1 assertion: whichever one of `projection` and `options` it declares."""
    if isinstance(value, dict):
        projection, options = "projection" in value, "options" in value
        if projection != options:
            return "projection" if projection else "options"
        return None
    if isinstance(value, ProjectionAssertionV1):
        return "projection"
    if isinstance(value, OptionsAssertion):
        return "options"
    return None


def _assertion_shape(value: Any) -> str | None:
    """The tag of a v2 assertion: `options` when it declares them, else `expected`.

    A stray `projection` is therefore refused at its own path, as an unknown key of the
    `expected` shape.
    """
    if isinstance(value, dict):
        return "options" if "options" in value else "expected"
    if isinstance(value, ExpectedAssertion):
        return "expected"
    if isinstance(value, OptionsAssertion):
        return "options"
    return None


type AssertionV1 = Annotated[
    Annotated[ProjectionAssertionV1, Tag("projection")]
    | Annotated[OptionsAssertion, Tag("options")],
    Discriminator(
        _assertion_shape_v1,
        custom_error_type="assertion_shape",
        custom_error_message="must be an object declaring exactly one of projection or options",
    ),
]

type Assertion = Annotated[
    Annotated[ExpectedAssertion, Tag("expected")] | Annotated[OptionsAssertion, Tag("options")],
    Discriminator(
        _assertion_shape,
        custom_error_type="assertion_shape",
        custom_error_message="must be an object declaring expected, and options for an option set",
    ),
]

# The manifest fields typed as a tagged union. pydantic puts the chosen tag into the
# location of every error inside one, after the field name; a refusal leaves it out, so it
# names the path an author wrote.
_TAGGED_FIELDS = frozenset({"assertion", "version_source"})

# The tags of the union `parse_manifest` and `parse_record` validate against, which pydantic
# puts first in every error's location.
_SCHEMA_VERSIONS = {1: "1", 2: "2"}


def _options_cohere(assertion: object, driver: Driver) -> None:
    """An option set needs a manual driver and unique option ids."""
    if not isinstance(assertion, OptionsAssertion):
        return
    if driver != Driver.MANUAL:
        raise ValueError("an options assertion requires a manual driver")
    seen: set[str] = set()
    for option in assertion.options:
        if option.id in seen:
            raise ValueError(f'manifest declares option id "{option.id}" more than once')
        seen.add(option.id)


class _ManifestFields(HarnessModel):
    """What every manifest version declares."""

    provider: Annotated[Provider, Field(strict=False)]
    driver: Annotated[Driver, Field(strict=False)]
    depth: Annotated[Depth | None, Field(strict=False)]
    question: NonEmpty
    _path: Path = PrivateAttr()

    @property
    def path(self) -> Path:
        """The manifest's path, as `parse_manifest` was given it."""
        return self._path


class ManifestV1(_ManifestFields):
    """A v1 manifest: its projection, `wait_for` and capture version are jq."""

    schema_version: SchemaV1
    version_source: VersionSourceV1 = NoSource()
    wait_for: NonEmpty | None = None
    assertion: AssertionV1

    @model_validator(mode="after")
    def _cohere(self) -> Self:
        _options_cohere(self.assertion, self.driver)
        return self


class ManifestV2(_ManifestFields):
    """A v2 manifest: every expression it is judged by is Python on the package's definition."""

    schema_version: SchemaV2
    version_source: VersionSource = NoSource()
    assertion: Assertion

    @model_validator(mode="after")
    def _cohere(self) -> Self:
        _options_cohere(self.assertion, self.driver)
        return self


type Manifest = ManifestV1 | ManifestV2


class RecordAssertionV1(HarnessModel):
    projection: NonEmpty | MISSING = MISSING
    options: Annotated[tuple[Option, ...] | MISSING, Field(strict=False)] = MISSING
    expected: JsonValue
    observed: JsonValue

    @model_validator(mode="after")
    def _one_shape(self) -> Self:
        if (self.projection is MISSING) == (self.options is MISSING):
            raise ValueError("must declare exactly one of projection or options")
        return self


class RecordV1(HarnessModel):
    """A v1 record. Field order is the key order the recorder writes."""

    schema_version: SchemaV1
    provider: Annotated[Provider, Field(strict=False)]
    status: Annotated[Status, Field(strict=False)]
    depth: Annotated[Depth | None, Field(strict=False)]
    date: Annotated[str, Field(pattern=r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$")]
    tool_version: str | None
    assertion: RecordAssertionV1


class RecordAssertionV2(HarnessModel):
    options: Annotated[tuple[Option, ...] | MISSING, Field(strict=False)] = MISSING
    expected: JsonValue
    observed: JsonValue


class RecordV2(HarnessModel):
    """A v2 record. Field order is the key order the recorder writes.

    `source` maps each repository-relative path that judged the workspace to its blob id;
    `arranged` is present only when the code that arranged the workspace differed.
    """

    schema_version: SchemaV2
    provider: Annotated[Provider, Field(strict=False)]
    status: Annotated[Status, Field(strict=False)]
    depth: Annotated[Depth | None, Field(strict=False)]
    date: Annotated[str, Field(pattern=r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$")]
    tool_version: str | None
    source: dict[NonEmpty, BlobId]
    arranged: dict[NonEmpty, BlobId] | MISSING = MISSING
    assertion: RecordAssertionV2


type Record = RecordV1 | RecordV2


def _schema_version(value: Any) -> str | None:
    """The tag of a manifest or record: its `schema_version`, when that is exactly 1 or 2."""
    if isinstance(value, dict):
        version = value.get("schema_version")
        # `type(...) is int`: `True` and `1.0` both equal 1.
        return _SCHEMA_VERSIONS.get(version) if type(version) is int else None
    if isinstance(value, ManifestV1 | ManifestV2 | RecordV1 | RecordV2):
        return _SCHEMA_VERSIONS[value.schema_version]
    return None


_SCHEMA_VERSION_DISCRIMINATOR = Discriminator(
    _schema_version,
    custom_error_type="schema_version",
    custom_error_message="schema_version must be 1 or 2",
)

_MANIFEST: TypeAdapter[ManifestV1 | ManifestV2] = TypeAdapter(
    Annotated[
        Annotated[ManifestV1, Tag("1")] | Annotated[ManifestV2, Tag("2")],
        _SCHEMA_VERSION_DISCRIMINATOR,
    ]
)

_RECORD: TypeAdapter[RecordV1 | RecordV2] = TypeAdapter(
    Annotated[
        Annotated[RecordV1, Tag("1")] | Annotated[RecordV2, Tag("2")],
        _SCHEMA_VERSION_DISCRIMINATOR,
    ]
)


class ManifestError(Exception):
    """A manifest refusal, shaped as a `console.Refusal`.

    `lines[0]` names the manifest; for an invalid one, each later line is one invalid field
    as `<path>: <message>`. `str(e)` is the lines joined by newlines.
    """

    def __init__(self, *lines: str) -> None:
        super().__init__("\n".join(lines))
        self.lines = lines
        self.detail: tuple[str, ...] = ()


def parse_manifest(path: str | Path) -> Manifest:
    """Validate <path> and return its typed form, raising `ManifestError` when it is invalid.

    Messages name the path as `Manifest.path` spells it, so every later refusal naming the
    manifest spells it the same way.
    """
    path = Path(path)
    if not path.is_file():
        raise ManifestError(f"manifest not found: {path}")
    try:
        document = json_values.loads(path.read_text(encoding="utf-8"))
    except ValueError, OSError:
        raise ManifestError(f"manifest is not valid JSON: {path}") from None
    if not isinstance(document, dict):
        raise ManifestError(f"manifest must be a JSON object: {path}")
    try:
        manifest = _MANIFEST.validate_python(document)
    except ValidationError as e:
        raise ManifestError(
            f"manifest is invalid: {path}", *(f"  {_error_line(error)}" for error in e.errors())
        ) from None
    manifest._path = path
    return manifest


def parse_record(path: Path) -> Record | None:
    """The record at <path>, or `None` when it cannot be read, is not JSON, or is no record."""
    try:
        return _RECORD.validate_python(json_values.loads(path.read_text(encoding="utf-8")))
    except ValueError, OSError:
        # `ValidationError` is a `ValueError`.
        return None


def _error_line(error: ErrorDetails) -> str:
    loc = list(error["loc"])
    # Every error inside a valid version is located under that version's tag first.
    if loc and loc[0] in _SCHEMA_VERSIONS.values():
        del loc[0]
    if len(loc) > 1 and loc[0] in _TAGGED_FIELDS:
        del loc[1]
    message = error["msg"]
    if error["type"] == "value_error":
        # A validator's own words, without pydantic's "Value error, " prefix.
        message = str(error.get("ctx", {}).get("error", message))
    if not loc:
        return message
    return f"{'.'.join(map(str, loc))}: {message}"
