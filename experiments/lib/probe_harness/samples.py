"""Samples: committed, anonymous inputs that prove each of a package's expressions.

A sample is `<package>/samples/<name>.json`, holding an input in the family's sample
format and the result each expression the definition carries yields on it. `probe sample`
writes one from a kept workspace: it parses the workspace through the family, reduces the
input to what the models declare plus the lines a search reads, anonymizes what is left,
and writes the sample only when every expression yields on the reduced input what it yields
on the full capture, and the leak scan finds nothing. The samples prove the definition's
expressions, and `--regenerate` rewrites their results from the definition.
"""

import contextlib
import getpass
import json
import re
import subprocess
from collections.abc import Callable, Iterator, Sequence
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path
from typing import Any, assert_never

from pydantic import RootModel

from probe_harness import json_values
from probe_harness.contract import (
    Manifest,
    Sample,
    SampleOrigin,
    SampleResults,
    parse_sample_file,
)
from probe_harness.families import ProbeDefinition, expressions, parse_sample, parse_workspace
from probe_harness.families.base import (
    CustomProbe,
    call,
    holds,
    observe,
    run_wait_for,
)
from probe_harness.families.claude_init import ClaudeInitProbe
from probe_harness.families.claude_otel import ClaudeOtelProbe, expression_input
from probe_harness.families.manual import ManualProbe, Unparsed, Unrecognized, recognized
from probe_harness.families.manual import line_document as manual_line_document
from probe_harness.families.opencode_debug import OpenCodeDebugProbe
from probe_harness.families.opencode_fake import OpenCodeFakeProbe
from probe_harness.gates import GateFailure, bodies
from probe_harness.json_values import JsonValue
from probe_harness.models.raw import ProviderModel, RawJson, sample_source


class Span(StrEnum):
    """The lines a search reads around the line carrying its search string."""

    # That line alone.
    LINE = "line"
    # That line and each following line up to, not including, the first empty one.
    PARAGRAPH = "paragraph"


@dataclass(frozen=True)
class Needle:
    """A search string, and the span of lines a sample keeps around each line carrying it."""

    text: str
    span: Span


# Searches a gate or an expression makes of a body, besides a definition's marker. The
# deferred-tools listing names its tools on the lines after its header, so its whole
# paragraph survives reduction. A search reading more of the listing's string than that
# may yield differently on a sample than on the capture, which `write_sample`'s equality
# check refuses, so a written sample proves no expression's result depended on a dropped
# line.
SEARCH_STRINGS: tuple[Needle, ...] = (
    Needle(bodies.DEFERRED_LISTING, Span.PARAGRAPH),
    Needle(bodies.SUBAGENT_FLAG, Span.LINE),
)

# Names of the operator's claude.ai connector tools, which are account configuration rather
# than anything a fixture arranged. Reduction drops a kept line starting with it, and the
# leak scan refuses a sample still carrying it.
CONNECTOR_PREFIX = "mcp__claude_ai_"

SAMPLES_DIR = "samples"

# Lines of an instruction file shorter than this are too generic to call personal.
_INSTRUCTION_LINE_MIN = 30

_SAMPLE_NAME = re.compile(r"[a-z0-9]+(-[a-z0-9]+)*")
_EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
_TEMP_DIR = re.compile(r"(/private)?/var/folders/[^/]+/[^/]+/T/+")
_ENCODED_TEMP_DIR = re.compile(r"-private-var-folders-[^-]+-[^-]+-T-")
_LEAK_PATTERNS = ("/Users/", "/var/folders/", "var-folders")


class SampleRefused(Exception):
    """A refusal: `lines` go to stderr, prefixed by the subcommand."""

    def __init__(self, *lines: str, detail: tuple[str, ...] = ()) -> None:
        super().__init__("\n".join(lines))
        self.lines = lines
        self.detail = detail


@dataclass(frozen=True)
class Identity:
    """What would identify the operator if it reached a committed sample."""

    home: str
    user: str
    email: str | None = None
    name: str | None = None
    # Lines of the operator's user-level instruction files.
    instruction_lines: tuple[str, ...] = ()
    # Names of the operator's own skills and commands.
    skill_names: tuple[str, ...] = ()

    @classmethod
    def current(cls) -> Identity:
        home = Path.home()
        return cls(
            home=str(home),
            user=getpass.getuser(),
            email=_git_config("user.email"),
            name=_git_config("user.name"),
            instruction_lines=tuple(
                dict.fromkeys(
                    stripped
                    for path in (
                        home / ".claude" / "CLAUDE.md",
                        home / ".config/opencode/AGENTS.md",
                    )
                    for stripped in _lines(path)
                    if len(stripped) >= _INSTRUCTION_LINE_MIN
                )
            ),
            skill_names=tuple(
                sorted(
                    {
                        name
                        for root in (
                            home / ".claude" / "skills",
                            home / ".claude" / "commands",
                            home / ".config" / "opencode" / "skill",
                            home / ".config" / "opencode" / "skills",
                            home / ".config" / "opencode" / "command",
                        )
                        for name in _names_under(root)
                        if "-" in name or ":" in name
                    }
                )
            ),
        )

    def values(self) -> Iterator[str]:
        """Every identifying string, for the leak scan."""
        yield from (v for v in (self.home, self.user, self.email, self.name) if v)
        yield from self.instruction_lines
        yield from self.skill_names


def anonymize(text: str, identity: Identity) -> str:
    """<text> with the operator's paths and username replaced by generic ones."""
    text = text.replace(identity.home, "/home/probe")
    text = _TEMP_DIR.sub("/tmp/", text)
    text = text.replace("/private/tmp/", "/tmp/")
    text = _ENCODED_TEMP_DIR.sub("-tmp-", text)
    text = text.replace(f"-Users-{identity.user}-", "-home-probe-")
    if identity.user:
        text = re.sub(
            rf"(?<![A-Za-z0-9_.-]){re.escape(identity.user)}(?![A-Za-z0-9_])", "probe", text
        )
    return text


def leaks(text: str, identity: Identity) -> list[str]:
    """Every identifying string or personal-looking path in <text>, each named once.

    A JSON text is scanned as written and as each of its decoded strings, so a value whose
    serialization escapes a character is still found.
    """
    haystacks = [text]
    with contextlib.suppress(ValueError):
        haystacks += list(RawJson(json_values.loads(text)).strings())
    hits: dict[str, None] = {}
    for haystack in haystacks:
        for value in identity.values():
            if value in haystack:
                hits[f"identity: {value}"] = None
        for pattern in _LEAK_PATTERNS:
            if pattern in haystack:
                hits[f"path: {pattern}"] = None
        if CONNECTOR_PREFIX in haystack:
            hits[f"connector: {CONNECTOR_PREFIX}"] = None
        for email in _EMAIL.findall(haystack):
            hits[f"email: {email}"] = None
    return list(hits)


def needles(definition: ProbeDefinition) -> tuple[Needle, ...]:
    """Each search a sample of <definition> must still answer: the search string, and the
    lines a sample keeps around each line carrying it. The definition's marker is a line."""
    marker = _marker(definition)
    return SEARCH_STRINGS if marker is None else (Needle(marker, Span.LINE), *SEARCH_STRINGS)


def reduce(value: object, needles: Sequence[Needle], identity: Identity) -> JsonValue:
    """<value>, a parsed input or any part of one, as the JSON a sample keeps.

    A model keeps every key it was parsed from, so `raw.keys()` is unchanged: a declared
    key that was set keeps its reduced value, and any other keeps its value only when a
    string in it carries a needle, and is `null` otherwise. A declared string is kept whole,
    since an expression reads it by name. A string kept under an undeclared key or in a
    `RawJson` value keeps only the lines its needles' spans cover, less any line starting
    with a claude.ai connector tool name. A string kept is anonymized.
    """
    match value:
        case ProviderModel():
            source = sample_source(value)
            assert isinstance(source, dict), "a provider model is validated from an object"
            declared = type(value).model_fields
            return {
                key: (
                    reduce(getattr(value, key), needles, identity)
                    if key in declared and key in value.model_fields_set
                    else (_scrub(raw, needles, identity) if _carries(raw, needles) else None)
                )
                for key, raw in source.items()
            }
        case RootModel():
            return reduce(value.root, needles, identity)
        case RawJson():
            return _scrub(sample_source(value), needles, identity)
        case tuple() | list():
            return [reduce(item, needles, identity) for item in value]
        case dict():
            return {
                anonymize(str(key), identity): reduce(item, needles, identity)
                for key, item in value.items()
            }
        case str():
            return anonymize(value, identity)
        case bool() | int() | float() | None:
            return value
    raise TypeError(f"a {type(value).__name__} has no sample form")


def document(
    definition: ProbeDefinition, value: Any, transform: Callable[[object], JsonValue]
) -> JsonValue:
    """<value>, a parsed input, in the family's sample format, each body through <transform>."""
    match definition:
        case ClaudeOtelProbe():
            return {
                arm: {
                    "requests": [transform(r) for r in capture.requests],
                    "responses": [transform(r) for r in capture.responses],
                }
                for arm, capture in value.items()
            }
        case OpenCodeFakeProbe():
            return {
                arm: {"requests": [transform(r) for r in capture.requests]}
                for arm, capture in value.items()
            }
        case ClaudeInitProbe():
            return {
                arm: {"init": transform(capture.init), "result": transform(capture.result)}
                for arm, capture in value.items()
            }
        case OpenCodeDebugProbe():
            return transform(value)
        case ManualProbe():
            return [_line(line, transform) for line in value]
        case CustomProbe():
            raise SampleRefused("a CustomProbe has no sample format")
        case _:
            assert_never(definition)


def expression_names(definition: ProbeDefinition) -> set[str]:
    """The result keys a sample of <definition> declares."""
    found = expressions(definition)
    names: set[str] = set()
    if found.assertion is not None:
        names.add("assertion")
    if found.wait_for is not None:
        names.add("wait_for")
    if found.capture_version is not None:
        names.add("capture_version")
    if found.custom_checks:
        names.add("custom_checks")
    return names


def evaluate(definition: ProbeDefinition, value: Any) -> SampleResults:
    """What each expression the definition carries yields over a parsed input, run over
    <value> as the judge hands it to them."""
    found = expressions(definition)
    expression_value = _expression_input(definition, value)
    results: dict[str, Any] = {}
    if found.assertion is not None:
        results["assertion"] = observe(found.assertion, expression_value).value
    if found.wait_for is not None:
        results["wait_for"] = run_wait_for(found.wait_for, value)
    if found.capture_version is not None:
        version = call("capture_version", found.capture_version, expression_value)
        if version is not None and type(version) is not str:
            raise GateFailure(
                f"the capture_version yielded a {type(version).__name__}, not a string or None"
            )
        results["capture_version"] = version
    if found.custom_checks:
        results["custom_checks"] = {
            check.name: holds(check, expression_value) for check in found.custom_checks
        }
    return SampleResults.model_validate(results)


def write_sample(
    definition: ProbeDefinition,
    manifest: Manifest,
    ws: Path,
    name: str,
    *,
    lines: int | None = None,
    identity: Identity | None = None,
) -> Path:
    """Write `samples/<name>.json` from workspace <ws>, refusing with `SampleRefused`."""
    identity = identity or Identity.current()
    if not _SAMPLE_NAME.fullmatch(name):
        raise SampleRefused(f"a sample name is lowercase letters, digits and -, not {name!r}")
    path = manifest.path.parent / SAMPLES_DIR / f"{name}.json"
    if path.exists():
        raise SampleRefused(f"{path} exists; samples are never overwritten")
    value = parse_workspace(definition, ws, lines)
    if isinstance(definition, ManualProbe):
        unrecognized = [line for line in value if isinstance(line, Unrecognized)]
        if unrecognized:
            raise SampleRefused(
                "the capture holds lines the payload model does not recognize:",
                *(f"  {line.problem}" for line in unrecognized),
            )
    full = evaluate(definition, value)
    found = needles(definition)
    reduced_document = document(definition, value, lambda body: reduce(body, found, identity))
    reduced_value = parse_sample(definition, reduced_document, str(path))
    reduced = evaluate(definition, reduced_value)
    differing = _differing(full, reduced)
    if differing:
        raise SampleRefused(
            "reducing the capture changed what these expressions yield, so the sample would "
            "not prove them:",
            *(
                f"  {key}: full {full_text} · reduced {reduced_text}"
                for key, full_text, reduced_text in differing
            ),
        )
    origin = SampleOrigin.PREFIX if lines is not None else SampleOrigin.CAPTURE
    sample = Sample(origin=origin, input=reduced_document, results=full)
    text = _sample_text(sample)
    found_leaks = leaks(text, identity)
    if found_leaks:
        raise SampleRefused(
            "the reduced sample still carries personal data:",
            *(f"  {leak}" for leak in found_leaks),
        )
    path.parent.mkdir(exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def regenerate(definition: ProbeDefinition, manifest: Manifest) -> list[str]:
    """Recompute every sample's `results` from its `input`; one line per changed result.

    The results are what the definition's expressions yield over each input. Only the
    samples whose results changed are rewritten; their `origin`, `note` and `input` keep
    their values, though the whole file is re-serialized, so `just fmt` restores its
    formatting. Every sample is evaluated before any is rewritten, so a refusal leaves them
    all as they were.
    """
    changes: list[str] = []
    rewritten: list[tuple[Path, Sample]] = []
    for path in sample_paths(manifest.path.parent):
        try:
            sample = parse_sample_file(path)
        except ValueError as e:
            raise SampleRefused(f"{path} is not a sample: {e}") from None
        value = parse_sample(definition, sample.input, str(path))
        try:
            results = evaluate(definition, value)
        except GateFailure as e:
            # Named as `parse_sample` names an input it refuses, so an author can tell which
            # of several samples an expression could not read.
            first, *rest = e.lines or ("",)
            raise GateFailure(f"{path}: {first}", *rest, detail=e.detail) from None
        differing = _differing(sample.results, results)
        if not differing:
            # Rewriting an unchanged sample would replace its prettier-formatted bytes.
            continue
        for key, before, after in differing:
            changes.append(f"{path.name}: {key}: {before} → {after}")
        rewritten.append((path, sample.model_copy(update={"results": results})))
    for path, sample in rewritten:
        path.write_text(_sample_text(sample), encoding="utf-8")
    return changes


def sample_paths(package: Path) -> list[Path]:
    """Every sample a package commits, in name order."""
    return sorted((package / SAMPLES_DIR).glob("*.json"))


def _marker(definition: ProbeDefinition) -> str | None:
    """The definition's marker, which `needles` keeps as a line."""
    match definition:
        case ClaudeOtelProbe(marker=marker) | OpenCodeFakeProbe(marker=marker):
            return marker
        case ManualProbe() | OpenCodeDebugProbe() | ClaudeInitProbe() | CustomProbe():
            return None
        case _:
            assert_never(definition)


def _expression_input(definition: ProbeDefinition, value: Any) -> Any:
    """<value> as the judge hands it to an assertion or a custom check."""
    match definition:
        case ClaudeOtelProbe():
            return expression_input(value, definition)
        case ManualProbe():
            return recognized(value)
        case OpenCodeFakeProbe() | OpenCodeDebugProbe() | ClaudeInitProbe() | CustomProbe():
            return value
        case _:
            assert_never(definition)


def _differing(a: SampleResults, b: SampleResults) -> list[tuple[str, str, str]]:
    """Each result key whose value differs between <a> and <b>, with both as compact text."""
    left, right = a.model_dump(mode="json"), b.model_dump(mode="json")
    return [
        (
            key,
            json_values.to_text(left.get(key, "absent")),
            json_values.to_text(right.get(key, "absent")),
        )
        for key in sorted(left.keys() | right.keys())
        if key not in left or key not in right or not json_values.equal(left[key], right[key])
    ]


def _line(line: object, transform: Callable[[object], JsonValue]) -> JsonValue:
    if isinstance(line, Unparsed | Unrecognized):
        return manual_line_document(line)
    return transform(line)


def _sample_text(sample: Sample) -> str:
    """A sample file's text: readable UTF-8, ASCII-escaped only when a lone surrogate needs it."""
    document = sample.model_dump(mode="json")
    text = json_values.dumps(document) + "\n"
    try:
        text.encode("utf-8")
    except UnicodeEncodeError:
        return json.dumps(document, indent=2, ensure_ascii=True) + "\n"
    return text


def _source_of(value: object) -> JsonValue:
    match value:
        case ProviderModel() | RawJson():
            return sample_source(value)
        case RootModel():
            return _source_of(value.root)
        case tuple() | list():
            return [_source_of(item) for item in value]
    raise TypeError(f"a {type(value).__name__} has no source")


def _carries(value: JsonValue, needles: Sequence[Needle]) -> bool:
    return any(needle.text in s for s in RawJson(value).strings() for needle in needles)


def _scrub(value: JsonValue, needles: Sequence[Needle], identity: Identity) -> JsonValue:
    """<value>'s shape, with every string not carrying a needle blanked, and the rest cut to
    the lines its needles' spans cover and anonymized."""
    match value:
        case dict():
            return {key: _scrub(item, needles, identity) for key, item in value.items()}
        case list():
            return [_scrub(item, needles, identity) for item in value]
        case str():
            if not any(needle.text in value for needle in needles):
                return ""
            return anonymize(_kept_lines(value, needles), identity)
        case _:
            return value


def _kept_lines(text: str, needles: Sequence[Needle]) -> str:
    """The lines of <text> its needles' spans cover, in order and each once, less any line
    starting with a claude.ai connector tool name."""
    lines = text.split("\n")
    kept: set[int] = set()
    for i, line in enumerate(lines):
        for needle in needles:
            if needle.text not in line:
                continue
            kept.add(i)
            if needle.span is Span.PARAGRAPH:
                end = i + 1
                while end < len(lines) and lines[end] != "":
                    kept.add(end)
                    end += 1
    return "\n".join(lines[i] for i in sorted(kept) if not lines[i].startswith(CONNECTOR_PREFIX))


def _git_config(key: str) -> str | None:
    try:
        completed = subprocess.run(
            ["git", "config", "--get", key],
            capture_output=True,
            text=True,
            stdin=subprocess.DEVNULL,
            check=False,
        )
    except OSError:
        return None
    value = completed.stdout.strip()
    return value or None


def _lines(path: Path) -> Iterator[str]:
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return
    yield from (line.strip() for line in text.splitlines())


def _names_under(root: Path) -> Iterator[str]:
    """Each skill or command name <root> defines: every directory's name, every markdown
    file's stem, and a nested command's `<dir>:<stem>`."""
    if not root.is_dir():
        return
    for path in root.rglob("*"):
        if path.is_dir():
            yield path.name
        elif path.suffix == ".md":
            yield path.stem
            if path.parent != root:
                yield f"{path.parent.name}:{path.stem}"
