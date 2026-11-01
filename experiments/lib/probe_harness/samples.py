"""Samples: committed, anonymous inputs that prove each of a package's expressions.

A sample is `<package>/samples/<name>.json`, holding an input in the family's sample
format and the result each expression the definition carries yields on it. `probe sample`
writes one from a kept workspace: it builds the input from every body's full source JSON,
anonymizes it, and cuts it by the removal test (`cut`) to the text the expressions need to
yield what they yield on the full capture. It writes the sample only when the anonymized
input still yields those results and the leak scan finds nothing in the cut one.
`--regenerate` recomputes the results from the definition, refusing a changed one unless
told to accept it, and re-cuts every input against them.
"""

import contextlib
import getpass
import json
import re
import subprocess
from collections.abc import Callable, Iterator, Mapping, Sequence
from dataclasses import dataclass, fields, is_dataclass
from enum import Enum
from pathlib import Path
from typing import Any, assert_never

from pydantic import BaseModel, RootModel

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
from probe_harness.gates import GateFailure
from probe_harness.json_values import JsonValue
from probe_harness.models.raw import ProviderModel, RawJson, sample_source
from probe_harness.workspace import workspace_root

SAMPLES_DIR = "samples"

# Lines of an instruction file shorter than this are too generic to call personal.
_INSTRUCTION_LINE_MIN = 30

_SAMPLE_NAME = re.compile(r"[a-z0-9]+(-[a-z0-9]+)*")
_EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
_TEMP_DIR = re.compile(r"(/private)?/var/folders/[^/]+/[^/]+/T/+")
_ENCODED_TEMP_DIR = re.compile(r"-private-var-folders-[^-]+-[^-]+-T-")
_LEAK_PATTERNS = ("/Users/", "/var/folders/", "var-folders")

# Where every sample spells the workspace root, wherever `workspace_root` puts it.
SAMPLE_WORKSPACE_ROOT = "/home/probe/.local/state/agentspec/probe-workspaces"


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
    """<text> with the operator's paths and username replaced by generic ones.

    The workspace root is rewritten first, in its slash form and in the `-`-encoded form
    Claude Code gives a working directory, so it is spelled one way whether or not
    `XDG_STATE_HOME` puts it under the home directory.
    """
    root = str(workspace_root())
    text = text.replace(root, SAMPLE_WORKSPACE_ROOT)
    text = text.replace(_encoded(root), _encoded(SAMPLE_WORKSPACE_ROOT))
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
        if "mcp__claude_ai_" in haystack:
            hits["connector: mcp__claude_ai_"] = None
        for email in _EMAIL.findall(haystack):
            hits[f"email: {email}"] = None
    return list(hits)


def anonymize_document(value: JsonValue, identity: Identity) -> JsonValue:
    """<value> with `anonymize` applied to every string and every key."""
    match value:
        case dict():
            return {
                anonymize(key, identity): anonymize_document(item, identity)
                for key, item in value.items()
            }
        case list():
            return [anonymize_document(item, identity) for item in value]
        case str():
            return anonymize(value, identity)
        case _:
            return value


def cut(
    definition: ProbeDefinition, document: JsonValue, oracle: SampleResults, label: str
) -> JsonValue:
    """<document>, a sample input, cut to the text its expressions need to yield <oracle>.

    The removal test. Raw content is a `RawJson` field's value, or the value of a key a
    modelled object does not declare; it is reachable only through `raw`. Three kinds of
    unit are tried for removal, a pass each, in this order and each in document order:
    every entry of an object inside raw content, key and value together; every string leaf,
    emptied; and every line of a multi-line string. Passes repeat until one removes nothing,
    so the result is a fixed point even for an expression whose result is not monotone in
    its input. Each pass tries its units in halving batches: a rejected batch splits in two,
    down to single units. A batch may remove units that one-at-a-time trials would keep,
    when their removals compensate, but in the final round every single unit's removal is
    rejected, which is what makes the result a fixed point.

    A removal stands only when the document still parses through the family, the parse
    keeps the same class at every model position and the same `model_fields_set` on every
    model, and every expression still yields <oracle>. Modelled structure is never touched:
    its keys, list lengths and non-string scalars stay, and so do a model's undeclared
    keys, so `raw.keys()` is unchanged. Refuses when <document> itself does not yield
    <oracle>.
    """
    reference = parse_sample(definition, document, label)
    if not _yields(definition, document, reference, oracle, label):
        raise SampleRefused(f"{label}: the uncut input does not yield the results it is cut to")
    roots = list(_raw_paths(reference, document, ()))
    current = document
    while True:
        removed = False
        for kind in _UnitKind:
            current, changed = _cut_pass(
                current,
                kind,
                list(_removals(current, kind, roots)),
                lambda candidate: _yields(definition, candidate, reference, oracle, label),
            )
            removed = removed or changed
        if not removed:
            return current


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
    oracle = evaluate(definition, value)
    anonymized = document(
        definition, value, lambda body: anonymize_document(_source_of(body), identity)
    )
    differing = _differing(
        oracle, evaluate(definition, parse_sample(definition, anonymized, str(path)))
    )
    if differing:
        raise SampleRefused(
            "anonymizing the capture changed what these expressions yield, so the sample would "
            "not prove them:",
            *(
                f"  {key}: full {full_text} · anonymized {anonymized_text}"
                for key, full_text, anonymized_text in differing
            ),
        )
    origin = SampleOrigin.PREFIX if lines is not None else SampleOrigin.CAPTURE
    sample = Sample(
        origin=origin, input=cut(definition, anonymized, oracle, str(path)), results=oracle
    )
    text = _sample_text(sample)
    found_leaks = leaks(text, identity)
    if found_leaks:
        raise SampleRefused(
            "the cut sample still carries personal data:",
            *(f"  {leak}" for leak in found_leaks),
        )
    path.parent.mkdir(exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def regenerate(
    definition: ProbeDefinition, manifest: Manifest, *, accept_results: bool
) -> list[str]:
    """Recompute every sample's `results` from its `input`, and re-cut every input.

    A changed result is refused unless <accept_results>: regenerate cannot tell whether an
    expression changed in a way that needs text an earlier cut removed, and recording a
    result computed on gutted input would agree with the code by construction. Each input
    is then cut (`cut`) against its recorded results, or the accepted ones. A sample is
    rewritten only when its results or its input changed; its `origin` and `note` keep
    their values, though the whole file is re-serialized, so `just fmt` restores its
    formatting. Every sample is evaluated and cut before any is rewritten, so a refusal
    leaves them all as they were.

    Returns `<sample>: <key>: <before> → <after>` per changed result, and
    `<sample>: cut <n> strings, <m> lines` per re-cut input.
    """
    changed_results: list[str] = []
    lines: list[str] = []
    rewritten: list[tuple[Path, Sample]] = []
    for path in sample_paths(manifest.path.parent):
        try:
            sample = parse_sample_file(path)
        except ValueError as e:
            raise SampleRefused(f"{path} is not a sample: {e}") from None
        try:
            value = parse_sample(definition, sample.input, str(path))
            results = evaluate(definition, value)
        except GateFailure as e:
            # Named as `parse_sample` names an input it refuses, so an author can tell which
            # of several samples an expression could not read.
            first, *rest = e.lines or ("",)
            raise GateFailure(f"{path}: {first}", *rest, detail=e.detail) from None
        differing = _differing(sample.results, results)
        for key, before, after in differing:
            changed_results.append(f"{path.name}: {key}: {before} → {after}")
        if differing and not accept_results:
            continue
        cut_input = cut(definition, sample.input, results, str(path))
        recut = not json_values.equal(cut_input, sample.input)
        if recut:
            strings, removed_lines = _cut_counts(sample.input, cut_input)
            lines.append(f"{path.name}: cut {strings} strings, {removed_lines} lines")
        if differing or recut:
            rewritten.append(
                (path, sample.model_copy(update={"results": results, "input": cut_input}))
            )
    if changed_results and not accept_results:
        raise SampleRefused(
            "regenerating would change these results:",
            *(f"  {change}" for change in changed_results),
            "re-cut the sample from its kept capture with probe sample, re-run the probe, or "
            "pass --accept-results for a change no cut text bears on, such as a hand edit",
        )
    for path, sample in rewritten:
        path.write_text(_sample_text(sample), encoding="utf-8")
    return [*changed_results, *lines]


def sample_paths(package: Path) -> list[Path]:
    """Every sample a package commits, in name order."""
    return sorted((package / SAMPLES_DIR).glob("*.json"))


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


def _encoded(path: str) -> str:
    """<path> as Claude Code encodes a working directory: every `/` and `.` written `-`."""
    return path.replace("/", "-").replace(".", "-")


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


class _UnitKind(Enum):
    """A pass of the removal test, in the order passes run."""

    ENTRY = "entry"
    STRING = "string"
    LINE = "line"


type _Key = str | int
type _Path = tuple[_Key, ...]


@dataclass(frozen=True)
class _Unit:
    """One removal: the entry or string at <path>, or line <line> of the string there."""

    path: _Path
    line: int = -1


class _Deleted:
    """An edit removing an object entry."""


_DELETED = _Deleted()
_ABSENT = object()


def _yields(
    definition: ProbeDefinition,
    candidate: JsonValue,
    reference: object,
    oracle: SampleResults,
    label: str,
) -> bool:
    """Whether <candidate> parses to <reference>'s shape and yields <oracle>."""
    try:
        value = parse_sample(definition, candidate, label)
        if not _same_shape(reference, value):
            return False
        return not _differing(oracle, evaluate(definition, value))
    except GateFailure, ValueError:
        return False


def _raw_paths(value: object, doc: JsonValue, path: _Path) -> Iterator[_Path]:
    """The JSON paths of raw content in <doc>, walked beside <value>, its parse."""
    match value:
        case RootModel():
            yield from _raw_paths(value.root, doc, path)
        case BaseModel():
            if not isinstance(doc, dict):
                return
            declared = type(value).model_fields
            for key, item in doc.items():
                if key in declared and key in value.model_fields_set:
                    yield from _raw_paths(getattr(value, key), item, (*path, key))
                else:
                    yield (*path, key)
        case RawJson():
            yield path
        case tuple() | list():
            if isinstance(doc, list):
                for i, (item, element) in enumerate(zip(value, doc, strict=False)):
                    yield from _raw_paths(item, element, (*path, i))
        case dict():
            if isinstance(doc, dict):
                for key, element in doc.items():
                    if key in value:
                        yield from _raw_paths(value[key], element, (*path, key))
        case _ if is_dataclass(value) and isinstance(doc, dict):
            # A family's arm capture, whose sample keys are its field names.
            names = {f.name for f in fields(value)}
            for key, element in doc.items():
                if key in names:
                    yield from _raw_paths(getattr(value, key), element, (*path, key))
        case _:
            return


def _same_shape(a: object, b: object) -> bool:
    """Whether <a> and <b> hold the same class at every model position, and the same
    `model_fields_set` on every model. Raw content and scalars may differ."""
    if type(a) is not type(b):
        return False
    match a:
        case RootModel():
            return _same_shape(a.root, getattr(b, "root", None))
        case BaseModel():
            assert isinstance(b, BaseModel)
            return a.model_fields_set == b.model_fields_set and all(
                _same_shape(getattr(a, key), getattr(b, key)) for key in a.model_fields_set
            )
        case RawJson():
            return True
        case tuple() | list():
            assert isinstance(b, tuple | list)
            return len(a) == len(b) and all(map(_same_shape, a, b, strict=True))
        case dict():
            assert isinstance(b, dict)
            return a.keys() == b.keys() and all(_same_shape(a[k], b[k]) for k in a)
        case _ if is_dataclass(a):
            return all(_same_shape(getattr(a, f.name), getattr(b, f.name)) for f in fields(a))
        case _:
            return True


def _removals(doc: JsonValue, kind: _UnitKind, roots: Sequence[_Path]) -> Iterator[_Unit]:
    """The units a pass of <kind> tries on <doc>, in document order."""
    match kind:
        case _UnitKind.ENTRY:
            for root in roots:
                yield from (
                    _Unit(path) for path, _ in _nodes(_at(doc, root), root) if _is_entry(path, root)
                )
        case _UnitKind.STRING:
            yield from (
                _Unit(path)
                for path, node in _nodes(doc, ())
                if node != "" and isinstance(node, str)
            )
        case _UnitKind.LINE:
            for path, node in _nodes(doc, ()):
                if isinstance(node, str) and "\n" in node:
                    yield from (_Unit(path, i) for i in range(node.count("\n") + 1))
        case _:
            assert_never(kind)


def _cut_pass(
    doc: JsonValue,
    kind: _UnitKind,
    units: Sequence[_Unit],
    accepts: Callable[[JsonValue], bool],
) -> tuple[JsonValue, bool]:
    """<doc> after one pass over <units>, and whether any removal stood.

    A batch of units is tried at once; a rejected batch is split in halves, down to single
    units, so a unit stays only when removing it alone is rejected.
    """
    # A line pass removes lines from each string as it stood when the pass began.
    original = {u.path: _at(doc, u.path) for u in units} if kind is _UnitKind.LINE else {}
    removed_lines: dict[_Path, set[int]] = {}
    removed = False

    def edits(batch: Sequence[_Unit]) -> dict[_Path, JsonValue | _Deleted]:
        found: dict[_Path, JsonValue | _Deleted] = {}
        match kind:
            case _UnitKind.ENTRY:
                found = {u.path: _DELETED for u in batch if _at(doc, u.path) is not _ABSENT}
            case _UnitKind.STRING:
                found = {u.path: "" for u in batch if _at(doc, u.path) not in ("", _ABSENT)}
            case _UnitKind.LINE:
                by_path: dict[_Path, set[int]] = {}
                for u in batch:
                    by_path.setdefault(u.path, set()).add(u.line)
                for path, indices in by_path.items():
                    already = removed_lines.get(path, set())
                    text = original[path]
                    if not indices - already or not isinstance(text, str):
                        continue
                    gone = already | indices
                    found[path] = "\n".join(
                        line for i, line in enumerate(text.split("\n")) if i not in gone
                    )
            case _:
                assert_never(kind)
        return found

    def attempt(batch: Sequence[_Unit]) -> None:
        nonlocal doc, removed
        found = edits(batch)
        if not found:
            return
        candidate = _edited(doc, found)
        if accepts(candidate):
            doc, removed = candidate, True
            if kind is _UnitKind.LINE:
                for u in batch:
                    removed_lines.setdefault(u.path, set()).add(u.line)
            return
        if len(batch) > 1:
            middle = len(batch) // 2
            attempt(batch[:middle])
            attempt(batch[middle:])

    attempt(units)
    return doc, removed


def _nodes(value: JsonValue, path: _Path) -> Iterator[tuple[_Path, JsonValue]]:
    """Every node under <value>, itself first, in document order."""
    yield path, value
    match value:
        case dict():
            for key, item in value.items():
                yield from _nodes(item, (*path, key))
        case list():
            for i, item in enumerate(value):
                yield from _nodes(item, (*path, i))
        case _:
            pass


def _is_entry(path: _Path, root: _Path) -> bool:
    """Whether <path>, under raw root <root>, names an object entry rather than the root or a
    list element."""
    return len(path) > len(root) and isinstance(path[-1], str)


def _at(doc: JsonValue, path: _Path) -> Any:
    """The node at <path> in <doc>, or `_ABSENT`."""
    node: Any = doc
    for key in path:
        found = (isinstance(key, str) and isinstance(node, dict) and key in node) or (
            isinstance(key, int) and isinstance(node, list) and key < len(node)
        )
        if not found:
            return _ABSENT
        node = node[key]
    return node


def _edited(doc: JsonValue, edits: Mapping[_Path, JsonValue | _Deleted]) -> JsonValue:
    """<doc> with each edit applied, sharing every subtree no edit reaches."""
    trie: dict[Any, Any] = {}
    for path, edit in edits.items():
        node = trie
        for key in path:
            node = node.setdefault(key, {})
        node[_LEAF] = edit
    return _rebuilt(doc, trie)


_LEAF = object()


def _rebuilt(value: JsonValue, trie: dict[Any, Any]) -> JsonValue:
    if _LEAF in trie:
        edit = trie[_LEAF]
        assert not isinstance(edit, _Deleted), "a deletion is applied by its parent"
        return edit  # type: ignore[no-any-return]
    match value:
        case dict():
            rebuilt: dict[str, JsonValue] = {}
            for key, item in value.items():
                sub = trie.get(key)
                if sub is None:
                    rebuilt[key] = item
                elif sub.get(_LEAF) is not _DELETED:
                    rebuilt[key] = _rebuilt(item, sub)
            return rebuilt
        case list():
            return [_rebuilt(item, trie[i]) if i in trie else item for i, item in enumerate(value)]
        case _:
            return value


def _cut_counts(before: JsonValue, after: JsonValue) -> tuple[int, int]:
    """How many of <before>'s strings a cut removed or emptied, and how many lines it removed
    from the strings it kept."""
    strings = lines = 0
    for path, node in _nodes(before, ()):
        if not isinstance(node, str) or node == "":
            continue
        kept = _at(after, path)
        if kept is _ABSENT or kept == "":
            strings += 1
        elif kept != node and isinstance(kept, str):
            lines += node.count("\n") - kept.count("\n")
    return strings, lines


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
