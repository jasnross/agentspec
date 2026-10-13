"""The manifest contract: its enumerated values, its shape, and the gates a manifest passes.

A manifest is hand-authored, and the failures it can express are exactly the ones the
harness exists to prevent: an absent `expected` compares `null == null` and records a
vacuous `confirmed`. `parse_manifest` refuses every malformed shape before anything reads
a value out of the manifest, so `probe run`, `probe record` and `probe replay` refuse the
same manifest the same way, before any workspace exists.

The gates run in a fixed order, each with a message that names the manifest. The order
matters in one place: the driver enum is checked before the rule that an option set needs
a manual driver. The checks test JSON types explicitly rather than relying on Python
truthiness, because `true == 1` in Python and a manifest must not pass on that.
"""

import json
from collections.abc import Callable
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path
from typing import Any, Literal

type JsonValue = Any


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


@dataclass(frozen=True)
class CommandSource:
    argv: tuple[str, ...]


@dataclass(frozen=True)
class CaptureSource:
    jq: str


@dataclass(frozen=True)
class NoSource:
    pass


type VersionSource = CommandSource | CaptureSource | NoSource


@dataclass(frozen=True)
class ProjectionAssertion:
    projection: str
    expected: JsonValue


@dataclass(frozen=True)
class Option:
    id: str
    text: str
    # `None` when the manifest declares no status.
    status: Literal["inconclusive"] | None


@dataclass(frozen=True)
class OptionsAssertion:
    options: tuple[Option, ...]
    expected: JsonValue


type Assertion = ProjectionAssertion | OptionsAssertion


@dataclass(frozen=True)
class Manifest:
    path: Path
    provider: Provider
    driver: Driver
    depth: Depth | None
    question: str
    version_source: VersionSource
    wait_for: str | None
    assertion: Assertion


class ManifestError(Exception):
    """A manifest gate refused. `str(e)` is the message, ending in `: <path>`."""


def parse_manifest(path: str | Path) -> Manifest:
    """Gate <path> and return its typed form, raising `ManifestError` at the first refusal.

    Messages name the path as `Manifest.path` spells it, so every later refusal naming the
    manifest spells it the same way.
    """
    path = Path(path)
    if not path.is_file():
        raise ManifestError(f"manifest not found: {path}")
    root = _load_root(path)

    def gate(ok: bool, message: str) -> None:
        if not ok:
            raise ManifestError(f"{message}: {path}")

    schema_version = root.get("schema_version")
    gate(type(schema_version) is int and schema_version == 1, "manifest schema_version must be 1")
    provider = root.get("provider")
    gate(_member(provider, Provider), "manifest provider must be claude, cursor, or opencode")
    question = root.get("question")
    gate(
        isinstance(question, str) and len(question) > 0,
        "manifest must declare a non-empty question",
    )
    assertion = root.get("assertion")
    gate(isinstance(assertion, dict), "manifest must declare an assertion")
    assert isinstance(assertion, dict)
    gate("expected" in assertion, "manifest assertion declares no expected value")
    gate(
        ("projection" in assertion) != ("options" in assertion),
        "manifest assertion must declare exactly one of projection or options",
    )
    options = assertion.get("options")

    # The driver enum is checked before the options correspondence, and the order is
    # load-bearing: the correspondence gate refuses any driver that is not `manual`, so
    # with it first, a manifest carrying a retired or misspelled driver *and* an option
    # set would be told it "requires a manual driver", implying it picked the wrong valid
    # value when the value is not in the enum at all.
    driver = root.get("driver")
    gate(_member(driver, Driver), "manifest driver must be unattended, billed, or manual")
    gate(
        "options" not in assertion or (isinstance(options, list) and len(options) > 0),
        "manifest assertion options must be a non-empty list",
    )
    gate(
        _option_statuses_inconclusive(options),
        'an option may declare only status "inconclusive"',
    )
    gate(
        "options" not in assertion or driver == Driver.MANUAL,
        "an options assertion requires a manual driver",
    )
    depth = root.get("depth")
    gate(
        depth is None or _member(depth, Depth),
        "manifest depth must be resolved-config, outbound-request, or null",
    )

    parsed: Assertion
    if "projection" in assertion:
        projection = assertion["projection"]
        gate(
            isinstance(projection, str) and len(projection) > 0,
            "manifest assertion projection must be a non-empty string",
        )
        parsed = ProjectionAssertion(projection, assertion["expected"])
    else:
        assert isinstance(options, list)
        parsed_options: list[Option] = []
        for n, option in enumerate(options, start=1):
            gate(
                _well_formed_option(option),
                f"manifest option {n} must be an object with a string id and text, "
                "and no other key but status",
            )
            parsed_options.append(Option(option["id"], option["text"], option.get("status")))
        seen: set[str] = set()
        for option in parsed_options:
            gate(option.id not in seen, f'manifest declares option id "{option.id}" more than once')
            seen.add(option.id)
        parsed = OptionsAssertion(tuple(parsed_options), assertion["expected"])

    version_source = _version_source(root.get("version_source"), gate)

    wait_for = root.get("wait_for")
    gate(
        wait_for is None or (isinstance(wait_for, str) and len(wait_for) > 0),
        "manifest wait_for must be a non-empty string",
    )
    return Manifest(
        path=path,
        provider=Provider(str(provider)),
        driver=Driver(str(driver)),
        depth=Depth(depth) if depth is not None else None,
        question=str(question),
        version_source=version_source,
        wait_for=wait_for,
        assertion=parsed,
    )


def _load_root(path: Path) -> dict[str, JsonValue]:
    """The manifest's single JSON value, which must be an object."""
    try:
        root = json.loads(path.read_text(encoding="utf-8"))
    except ValueError, OSError:
        raise ManifestError(f"manifest is not valid JSON: {path}") from None
    if not isinstance(root, dict):
        raise ManifestError(f"manifest must be a JSON object: {path}")
    return root


def _member(value: JsonValue, enum: type[StrEnum]) -> bool:
    """String equality against an enum's values; any non-string is simply not a member."""
    return isinstance(value, str) and value in {member.value for member in enum}


def _option_statuses_inconclusive(options: JsonValue) -> bool:
    """A declared option status may only be `inconclusive`.

    Any other declared status would fix the verdict in advance. A projection manifest
    declares no options, and an option that is not an object falls through to the shape
    gate that follows.
    """
    if not isinstance(options, list):
        return True
    return all(
        option["status"] == "inconclusive"
        for option in options
        if isinstance(option, dict) and "status" in option
    )


def _well_formed_option(option: JsonValue) -> bool:
    return (
        isinstance(option, dict)
        and set(option) <= {"id", "text", "status"}
        and isinstance(option.get("id"), str)
        and len(option["id"]) > 0
        and isinstance(option.get("text"), str)
    )


def _version_source(source: JsonValue, gate: Callable[[bool, str], None]) -> VersionSource:
    gate(source is None or isinstance(source, dict), "manifest version_source must be an object")
    fields = source or {}
    kind = fields.get("kind")
    gate(
        kind is None or kind in ("command", "capture", "none"),
        "manifest version_source.kind must be command, capture, or none",
    )
    match kind:
        case "command":
            # Split into argv and exec directly rather than through a shell: a manifest is
            # reviewed as data, so it must not be able to express arbitrary code.
            command = fields.get("command")
            gate(
                isinstance(command, str)
                and len(command.splitlines()) == 1
                and len(command.split()) > 0,
                "manifest version_source.command must be a non-empty, single-line string",
            )
            return CommandSource(tuple(str(command).split()))
        case "capture":
            expr = fields.get("jq")
            gate(
                isinstance(expr, str) and len(expr) > 0,
                "manifest version_source.jq must be a non-empty string",
            )
            return CaptureSource(str(expr))
        case _:
            return NoSource()
