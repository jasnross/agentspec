"""The manifest contract: its enumerated values, its shape, and the gates a manifest passes.

A manifest is hand-authored, and the failures it can express are exactly the ones the
harness exists to prevent: an absent `expected` compares `null == null` and records a
vacuous `confirmed`. Every gate therefore runs before anything reads a value out of the
manifest, in a fixed order, with a message that names the manifest.

Each gate reproduces the jq expression the recorder has always applied, so a value on
which that expression would error or yield false fails the gate. That is why the checks
below test JSON types explicitly rather than relying on Python truthiness: in jq, `true`
is not `1`, and indexing a non-object root is an error rather than a missing key.
"""

import json
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path
from typing import Any

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


@dataclass(frozen=True)
class UnknownSource:
    """A kind no branch recognizes. Refused by the recorder at its point, not at parse."""

    kind: str


type VersionSource = CommandSource | CaptureSource | NoSource | UnknownSource


@dataclass(frozen=True)
class ProjectionAssertion:
    projection: str
    expected: JsonValue


@dataclass(frozen=True)
class OptionsAssertion:
    options: tuple[JsonValue, ...]
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

    Messages name the path exactly as given.
    """
    if not Path(path).is_file():
        raise ManifestError(f"manifest not found: {path}")
    root = _load_root(Path(path), path)

    def gate(ok: bool, message: str) -> None:
        if not ok:
            raise ManifestError(f"{message}: {path}")

    # Indexing any non-object root is a jq error, so every such root fails here.
    gate(
        isinstance(root, dict) and _is_one(root.get("schema_version")),
        "manifest schema_version must be 1",
    )
    assert isinstance(root, dict)
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

    # The driver enum is checked before the options correspondence, and the order is
    # load-bearing: the correspondence gate refuses any driver that is not `manual`, so
    # with it first, a manifest carrying a retired or misspelled driver *and* an option
    # set would be told it "requires a manual driver", implying it picked the wrong valid
    # value when the value is not in the enum at all.
    driver = root.get("driver")
    gate(_member(driver, Driver), "manifest driver must be unattended, billed, or manual")
    gate(
        _option_statuses_inconclusive(assertion.get("options")),
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
        parsed = ProjectionAssertion(jq_raw(assertion["projection"]), assertion["expected"])
    else:
        parsed = OptionsAssertion(_iterate(assertion["options"]), assertion["expected"])

    wait_for = _alternative(root.get("wait_for"), "")
    return Manifest(
        path=Path(path),
        provider=Provider(str(provider)),
        driver=Driver(str(driver)),
        depth=Depth(depth) if depth is not None else None,
        question=str(question),
        version_source=_version_source(root.get("version_source")),
        wait_for=jq_raw(wait_for) or None,
        assertion=parsed,
    )


def jq_raw(value: JsonValue) -> str:
    """What `jq -r` prints for one value, without its trailing newline."""
    if isinstance(value, str):
        return value
    return json.dumps(value, indent=2, ensure_ascii=False)


def _load_root(path: Path, display: str | Path) -> JsonValue:
    """The manifest's single JSON value. A `null` or `false` root fails `jq -e .`."""
    try:
        root = json.loads(path.read_text(encoding="utf-8"))
    except ValueError, OSError:
        raise ManifestError(f"manifest is not valid JSON: {display}") from None
    if root is None or root is False:
        raise ManifestError(f"manifest is not valid JSON: {display}")
    return root


def _member(value: JsonValue, enum: type[StrEnum]) -> bool:
    """String equality against an enum's values; any non-string is simply not a member."""
    return isinstance(value, str) and value in {member.value for member in enum}


def _is_one(value: JsonValue) -> bool:
    return isinstance(value, int | float) and not isinstance(value, bool) and value == 1


def _iterate(value: JsonValue) -> tuple[JsonValue, ...]:
    """`.[]?`: an array's elements, an object's values, and nothing for anything else."""
    if isinstance(value, list):
        return tuple(value)
    if isinstance(value, dict):
        return tuple(value.values())
    return ()


def _option_statuses_inconclusive(options: JsonValue) -> bool:
    """A declared option status may only be `inconclusive`.

    Any other declared status would fix the verdict in advance. `[]?` is vacuously true
    wherever there is nothing to iterate, which covers a projection manifest; a non-object
    option falls through to the clearer refusal downstream.
    """
    return all(
        option["status"] == "inconclusive"
        for option in _iterate(options)
        if isinstance(option, dict) and "status" in option
    )


def _alternative(value: JsonValue, default: JsonValue) -> JsonValue:
    """jq's `//`: the default when the value is `null` or `false`."""
    return default if value is None or value is False else value


def _version_source(source: JsonValue) -> VersionSource:
    if source is not None and not isinstance(source, dict):
        # `.version_source.kind` is a jq error here; no recognized kind can follow.
        return UnknownSource(jq_raw(source))
    fields = source or {}
    kind = jq_raw(_alternative(fields.get("kind"), "none"))
    match kind:
        case "command":
            # Split into argv and exec directly rather than through a shell: a manifest is
            # reviewed as data, so it must not be able to express arbitrary code. Like
            # bash's `read -a`, only the first line is read and it splits on blanks.
            first_line = jq_raw(fields.get("command")).split("\n", 1)[0]
            return CommandSource(tuple(w for w in first_line.replace("\t", " ").split(" ") if w))
        case "capture":
            return CaptureSource(jq_raw(fields.get("jq")))
        case "none":
            return NoSource()
        case _:
            return UnknownSource(kind)
