"""Samples: committed, anonymous inputs that prove each of a package's expressions.

A sample is `<package>/samples/<name>.json`, holding an input in the family's sample
format and the result each expression the definition carries yields on it. `probe sample`
writes one from a kept workspace: it builds the input from every body's full source JSON,
anonymizes it, and cuts it by the removal test (`cut`) to the text the expressions need to
yield what they yield on the full capture. It writes the sample only when the anonymized
input still yields those results and the accounting check (`unaccounted`) vouches for every
string the cut one keeps. `--regenerate` recomputes the results from the definition, refusing
a changed one unless told to accept it, and re-cuts every input against them under the same
check.

The accounting check is what keeps an operator's configuration out of a committed sample.
Every kept string must equal text the package's authors wrote (a `probe.py` or `probe.json`
string, a fixture line or file), match an entry of the catalog of provider and harness text
(`provider_text.CATALOG`), or occur in an edited sample's `authored` list. Text that is none
of these, such as a report summarizing the operator's instructions, is refused wherever it
sits, without knowing in advance what the operator's configuration says.
"""

import ast
import contextlib
import getpass
import json
import re
from collections.abc import Callable, Iterable, Iterator, Mapping, Sequence
from dataclasses import dataclass, fields, is_dataclass
from enum import Enum
from pathlib import Path
from typing import Any, Literal, assert_never

from pydantic import MISSING, BaseModel, RootModel

from probe_harness import json_values, provider_text
from probe_harness.contract import (
    Manifest,
    Sample,
    SampleOrigin,
    SampleResults,
    parse_sample_file,
)
from probe_harness.families import ProbeDefinition, expressions, parse_sample, parse_workspace
from probe_harness.families.base import (
    CustomCheck,
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
from probe_harness.provider_text import Id, Placeholder, Template
from probe_harness.workspace import SAMPLE_HOME, SAMPLE_WORKSPACE_ROOT, workspace_root

SAMPLES_DIR = "samples"

_SAMPLE_NAME = re.compile(r"[a-z0-9]+(-[a-z0-9]+)*")
_EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
_TEMP_DIR = re.compile(r"(/private)?/var/folders/[^/]+/[^/]+/T/+")
_ENCODED_TEMP_DIR = re.compile(r"-private-var-folders-[^-]+-[^-]+-T-")
_LEAK_PATTERNS = ("/Users/", "/var/folders/", "var-folders")
_CONNECTOR_PREFIX = "mcp__claude_ai_"


class SampleRefused(Exception):
    """A refusal: `lines` go to stderr, prefixed by the subcommand."""

    def __init__(self, *lines: str, detail: tuple[str, ...] = ()) -> None:
        super().__init__("\n".join(lines))
        self.lines = lines
        self.detail = detail


@dataclass(frozen=True)
class Identity:
    """The operator's home directory and username, which `anonymize` rewrites."""

    home: str
    user: str

    @classmethod
    def current(cls) -> Identity:
        return cls(home=str(Path.home()), user=getpass.getuser())


def anonymize(text: str, identity: Identity) -> str:
    """<text> with the operator's paths and username replaced by generic ones.

    The workspace root is rewritten first, in its slash form and in the `-`-encoded form
    Claude Code gives a working directory, so it is spelled one way whether or not
    `XDG_STATE_HOME` puts it under the home directory.
    """
    root = str(workspace_root())
    text = text.replace(root, SAMPLE_WORKSPACE_ROOT)
    text = text.replace(_encoded(root), _encoded(SAMPLE_WORKSPACE_ROOT))
    text = text.replace(identity.home, SAMPLE_HOME)
    text = _TEMP_DIR.sub("/tmp/", text)
    text = text.replace("/private/tmp/", "/tmp/")
    text = _ENCODED_TEMP_DIR.sub("-tmp-", text)
    text = text.replace(f"-Users-{identity.user}-", "-home-probe-")
    if identity.user:
        text = re.sub(
            rf"(?<![A-Za-z0-9_.-]){re.escape(identity.user)}(?![A-Za-z0-9_])", "probe", text
        )
    return text


@dataclass(frozen=True)
class Accounting:
    """What one sample's text is accounted against: its package's authored files, the
    catalog (`provider_text.CATALOG`), and the sample's own `authored` list.

    Built by `accounting`. Every test is equality with a whole member or a whole catalog
    match, never containment, so neither a short identity value inside an ordinary word nor
    prose built from authored words is vouched for. Two forms reach authored text a provider
    or a fixture's own program echoes without its surroundings: a value equal to the value of
    a `key: value` fixture line, as a provider parses frontmatter, and a key equal to a whole
    authored token, as a fixture's capture hook names its envelope's fields.
    """

    # Every `str` constant in `probe.py`, docstrings and f-string parts included, and every
    # string and key in `probe.json`, each whole and each of its lines.
    literals: frozenset[str]
    fixture_lines: frozenset[str]
    fixture_files: frozenset[str]
    # The value of each `key: value` fixture line, surrounding whitespace removed.
    fixture_values: frozenset[str]
    # Every word-bounded token of the fixtures and of `probe.py` and `probe.json` strings,
    # docstrings and the manifest's `question` aside, for `TOKEN` and `vouches_key`.
    tokens: frozenset[str]
    authored: frozenset[str]
    catalog_texts: frozenset[str]
    catalog_pattern: re.Pattern[str] | None

    def vouches(self, text: str) -> bool:
        return (
            not text
            or text in self.literals
            or text in self.fixture_lines
            or text in self.fixture_files
            or text in self.fixture_values
            or text in self.authored
            or text in self.catalog_texts
            or (
                self.catalog_pattern is not None
                and self.catalog_pattern.fullmatch(text) is not None
            )
        )

    def vouches_key(self, key: str) -> bool:
        """Whether an object key is accounted for: as any text is, or as a whole token."""
        return self.vouches(key) or key in self.tokens


def accounting(package: Path, authored: Sequence[str] = ()) -> Accounting:
    """The `Accounting` of a sample of <package> whose `authored` list is <authored>."""
    code, docstrings = _probe_py_strings(package / "probe.py")
    manifest, question = _probe_json_strings(package / "probe.json")
    literals = code | docstrings | manifest
    fixtures = package / "fixtures"
    files: dict[Path, str] = {}
    if fixtures.is_dir():
        for path in sorted(fixtures.rglob("*", recurse_symlinks=True)):
            if path.is_file():
                with contextlib.suppress(OSError, UnicodeDecodeError):
                    files[path.relative_to(fixtures)] = path.read_text(encoding="utf-8")
    tokens = frozenset(
        token
        # Prose an author writes about the probe, its docstrings and question, is no source of
        # words: its vocabulary is too broad to vouch for a word on its own.
        for text in (*code, *(text for text in manifest if text != question), *files.values())
        for pattern in (_WORD, _HYPHENATED_WORD)
        for token in pattern.findall(text)
    )
    path_names = {
        *(part for relative in files for part in relative.parts),
        *(
            part
            for relative in (p.relative_to(fixtures) for p in _directories(fixtures))
            for part in relative.parts
        ),
        *_LAYOUT_DIRS,
        *(literal for literal in literals if _PATH_SEGMENT.fullmatch(literal)),
    }
    variants = {p.name for p in _directories(fixtures, depth=1)}
    catalog_texts = frozenset(e.text for e in provider_text.CATALOG if isinstance(e.text, str))
    templates = [e.text for e in provider_text.CATALOG if isinstance(e.text, Template)]
    context = _TemplateContext(package.name, frozenset(variants), frozenset(path_names), tokens)
    fixture_lines = frozenset(line for text in files.values() for line in text.split("\n"))
    return Accounting(
        # A multi-line literal is vouched for whole, and line by line as a fixture file is.
        literals=frozenset(
            form for literal in literals for form in (literal, *literal.split("\n"))
        ),
        fixture_lines=fixture_lines,
        fixture_files=frozenset(
            form for text in files.values() for form in (text, text.removesuffix("\n"))
        ),
        fixture_values=frozenset(
            match.group(1) for line in fixture_lines if (match := _KEY_VALUE.fullmatch(line))
        ),
        tokens=tokens,
        authored=frozenset(authored),
        catalog_texts=catalog_texts,
        catalog_pattern=(
            re.compile("|".join(f"(?:{_template_regex(t, context)})" for t in templates))
            if templates
            else None
        ),
    )


def unaccounted(
    package: Path, definition: ProbeDefinition, sample: Sample
) -> list[tuple[str, str]]:
    """`(JSON path, text)` for each unit of <sample>'s `input` and `results`, keys included,
    that `accounting` does not vouch for.

    A unit is a single-line string, or each line of a multi-line string unless the whole
    string is vouched for, as a fixture file or a literal is. A key the harness writes is
    exempt: an arm name at the top of the input, the family's format keys under it, and
    each `results` key.
    """
    found = accounting(package, () if sample.authored is MISSING else sample.authored)
    arms, format_keys = _format_keys(definition)

    def exempt(path: _Path, key: str) -> bool:
        return (
            path == ("results",)
            or (path == ("input",) and key in arms)
            or (len(path) == 2 and path[0] == "input" and path[1] in arms and key in format_keys)
        )

    return [
        *_unaccounted_units(sample.input, ("input",), found, exempt),
        *_unaccounted_units(sample.results.model_dump(mode="json"), ("results",), found, exempt),
    ]


def authored_text_leaks(sample: Sample) -> list[str]:
    """Each machine-independent identity kind in <sample>'s `note` and `authored` entries,
    which the accounting check exempts: an email shape, a macOS home or temp path, and an
    account connector's tool prefix."""
    texts: list[tuple[str, str]] = []
    if sample.note is not MISSING:
        texts.append(("note", sample.note))
    if sample.authored is not MISSING:
        texts.extend(("authored", entry) for entry in sample.authored)
    hits: dict[str, None] = {}
    for where, text in texts:
        for pattern in _LEAK_PATTERNS:
            if pattern in text:
                hits[f"{where}: path: {pattern}"] = None
        if _CONNECTOR_PREFIX in text:
            hits[f"{where}: connector: {_CONNECTOR_PREFIX}"] = None
        for email in _EMAIL.findall(text):
            hits[f"{where}: email: {email}"] = None
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
    definition: ProbeDefinition,
    document: JsonValue,
    oracle: SampleResults,
    label: str,
    accounting: Accounting,
) -> JsonValue:
    """<document>, a sample input, cut to the text its expressions need to yield <oracle>.

    The removal test. Raw content is a `RawJson` field's value, or the value of a key a
    modelled object does not declare; it is reachable only through `raw`. Three kinds of
    unit are tried for removal, a pass each, in this order and each in document order:
    every entry of an object inside raw content, key and value together; every string leaf,
    emptied; and every line of a multi-line string. Only text <accounting> cannot vouch for
    is tried, so a sample keeps the authored and provider text its case rests on even where
    no expression's recorded result needs it: an entry is tried whole when its key is
    unaccounted or nothing in it is accounted for, a string leaf when none of its text is
    accounted for, and a line
    when it is unaccounted and its string is not accounted for whole. Passes repeat until one
    removes nothing, so the result is a fixed point even for an expression whose result is
    not monotone in its input. Each pass tries its units in halving batches: a rejected
    batch splits in two, down to single units. A batch may remove units that one-at-a-time
    trials would keep, when their removals compensate, but in the final round every single
    unit's removal is rejected, which is what makes the result a fixed point.

    A removal stands only when the document still parses through the family, the parse
    keeps the same class at every model position and the same `model_fields_set` on every
    model, every expression still yields <oracle>, and every wrong version the definition
    names that catches on <document> (by a value other than <oracle>'s, or by raising) still
    catches the same way. Modelled structure is never touched:
    its keys, list lengths and non-string scalars stay, and so do a model's undeclared
    keys, so `raw.keys()` is unchanged. Refuses when <document> itself does not yield
    <oracle>.
    """
    reference = parse_sample(definition, document, label)
    catches = _catches(definition, reference, oracle)
    if not _yields(definition, document, reference, oracle, catches, label):
        raise SampleRefused(f"{label}: the uncut input does not yield the results it is cut to")
    roots = list(_raw_paths(reference, document, ()))
    current = document
    while True:
        removed = False
        for kind in _UnitKind:
            current, changed = _cut_pass(
                current,
                kind,
                list(_removals(current, kind, roots, accounting)),
                lambda candidate: _yields(definition, candidate, reference, oracle, catches, label),
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
    cut_input = cut(definition, anonymized, oracle, str(path), accounting(manifest.path.parent))
    sample = Sample(origin=origin, input=cut_input, results=oracle)
    refusal = _unaccounted_refusal(manifest.path.parent, definition, [(path, sample)])
    if refusal:
        raise SampleRefused(*refusal)
    text = _sample_text(sample)
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
    checked: list[tuple[Path, Sample]] = []
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
        authored = () if sample.authored is MISSING else sample.authored
        cut_input = cut(
            definition, sample.input, results, str(path), accounting(manifest.path.parent, authored)
        )
        recut = not json_values.equal(cut_input, sample.input)
        if recut:
            strings, removed_lines = _cut_counts(sample.input, cut_input)
            lines.append(f"{path.name}: cut {strings} strings, {removed_lines} lines")
        try:
            # Validated anew, since a cut can remove text an `authored` entry names.
            rewritten_sample = Sample.model_validate(
                {**sample.model_dump(mode="json"), "results": results, "input": cut_input}
            )
        except ValueError as e:
            raise SampleRefused(f"{path} would no longer be a sample once re-cut: {e}") from None
        checked.append((path, rewritten_sample))
        if differing or recut:
            rewritten.append(checked[-1])
    if changed_results and not accept_results:
        raise SampleRefused(
            "regenerating would change these results:",
            *(f"  {change}" for change in changed_results),
            "re-cut the sample from its kept capture with probe sample, re-run the probe, or "
            "pass --accept-results for a change no cut text bears on, such as a hand edit",
        )
    refusal = _unaccounted_refusal(manifest.path.parent, definition, checked)
    if refusal:
        raise SampleRefused(*refusal)
    for path, sample in rewritten:
        path.write_text(_sample_text(sample), encoding="utf-8")
    return [*changed_results, *lines]


def sample_paths(package: Path) -> list[Path]:
    """Every sample a package commits, in name order."""
    return sorted((package / SAMPLES_DIR).glob("*.json"))


def _unaccounted_refusal(
    package: Path, definition: ProbeDefinition, checked: Sequence[tuple[Path, Sample]]
) -> list[str]:
    """The refusal's lines when any of <checked> keeps unaccounted text or carries an
    identity kind in its authored text, else none."""
    lines: list[str] = []
    for path, sample in checked:
        lines += [
            f"  {path.name}: {where}: {json.dumps(text, ensure_ascii=False)}"
            for where, text in unaccounted(package, definition, sample)
        ]
        lines += [f"  {path.name}: {leak}" for leak in authored_text_leaks(sample)]
    if not lines:
        return []
    return [
        "these samples keep text no authored file, catalog entry or authored list accounts "
        "for, or authored text carrying an identity:",
        *lines,
        "add provider text to provider_text.py, list text an author typed in the sample's "
        "authored list, and re-cut or replace anything else",
    ]


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
    catches: Mapping[str, _Catch],
    label: str,
) -> bool:
    """Whether <candidate> parses to <reference>'s shape, yields <oracle>, and keeps each of
    <catches>."""
    try:
        value = parse_sample(definition, candidate, label)
        if not _same_shape(reference, value):
            return False
        if _differing(oracle, evaluate(definition, value)):
            return False
    except GateFailure, ValueError:
        return False
    if not catches:
        return True
    found = _catches(definition, value, oracle)
    return all(found.get(name) == kind for name, kind in catches.items())


type _Catch = Literal["value", "raise"]


def _catches(definition: ProbeDefinition, value: Any, oracle: SampleResults) -> dict[str, _Catch]:
    """How each wrong version of <definition> catches on <value>, a parsed input whose
    expressions yield <oracle>: by a value other than the recorded one, or by raising. A
    wrong version yielding the recorded value is absent. Each is run as `evaluate` runs the
    expression it stands in for, so a value `evaluate` would refuse, such as a non-string
    capture version, is a catch by raising."""
    found = expressions(definition)
    if not found.wrong_versions:
        return {}
    recorded = oracle.model_dump(mode="json")
    expression_value = _expression_input(definition, value)
    checks = {check.name: check for check in found.custom_checks}
    carried = found.names()
    catches: dict[str, _Catch] = {}
    for wrong in found.wrong_versions:
        if wrong.stands_in_for not in carried:
            raise SampleRefused(
                f"the wrong version {wrong.name} stands in for {wrong.stands_in_for}, which "
                "the definition does not carry"
            )
        try:
            match wrong.stands_in_for:
                case "assertion":
                    got: Any = observe(wrong.function, expression_value).value
                    want = recorded["assertion"]
                case "wait_for":
                    got = run_wait_for(wrong.function, value)
                    want = recorded["wait_for"]
                case "capture_version":
                    got = call("capture_version", wrong.function, expression_value)
                    if got is not None and type(got) is not str:
                        raise GateFailure("the capture_version yielded neither a string nor None")
                    want = recorded["capture_version"]
                case name:
                    stood = checks[name]
                    got = holds(CustomCheck(name, wrong.function, stood.failure), expression_value)
                    want = recorded["custom_checks"][name]
        except GateFailure:
            catches[wrong.name] = "raise"
            continue
        if not json_values.equal(got, want):
            catches[wrong.name] = "value"
    return catches


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


def _removals(
    doc: JsonValue, kind: _UnitKind, roots: Sequence[_Path], found: Accounting
) -> Iterator[_Unit]:
    """The units a pass of <kind> tries on <doc>, in document order: only those holding text
    <found> cannot vouch for."""
    match kind:
        case _UnitKind.ENTRY:
            for root in roots:
                yield from (
                    _Unit(path)
                    for path, node in _nodes(_at(doc, root), root)
                    if _is_entry(path, root)
                    and (not found.vouches_key(str(path[-1])) or _nothing_accounted(node, found))
                )
        case _UnitKind.STRING:
            yield from (
                _Unit(path)
                for path, node in _nodes(doc, ())
                if isinstance(node, str) and node != "" and _none_accounted(node, found)
            )
        case _UnitKind.LINE:
            for path, node in _nodes(doc, ()):
                if isinstance(node, str) and "\n" in node and not found.vouches(node):
                    yield from (
                        _Unit(path, i)
                        for i, line in enumerate(node.split("\n"))
                        if not found.vouches(line)
                    )
        case _:
            assert_never(kind)


def _nothing_accounted(value: JsonValue, found: Accounting) -> bool:
    """Whether <found> vouches for no key, and no non-empty string or line, in <value>.
    Removing an entry holding such a value removes no accounted text but the entry's key.
    An entry holding accounted text is left to the passes that remove only the unaccounted
    parts of it."""
    match value:
        case dict():
            return all(
                not found.vouches_key(key) and _nothing_accounted(item, found)
                for key, item in value.items()
            )
        case list():
            return all(_nothing_accounted(item, found) for item in value)
        case str():
            return value == "" or _none_accounted(value, found)
        case _:
            return True


def _none_accounted(text: str, found: Accounting) -> bool:
    """Whether <found> vouches for none of <text>: not for the whole, and, for a multi-line
    string, for none of its non-empty lines. Emptying such a string removes no accounted
    text."""
    if found.vouches(text):
        return False
    return "\n" not in text or not any(found.vouches(line) for line in text.split("\n") if line)


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


@dataclass(frozen=True)
class _TemplateContext:
    """What the package-dependent placeholders admit."""

    package: str
    variants: frozenset[str]
    path_names: frozenset[str]
    tokens: frozenset[str]


_UUID = r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"
# A segment `TEMP` admits: a UUID, `HEX` or `DIGITS`.
_ID_SEGMENT = rf"(?:{_UUID}|[0-9a-f]{{8,}}|[0-9]+)"

_FIXED_PATTERNS = {
    Placeholder.DIGITS: r"[0-9]+",
    Placeholder.HEX: r"[0-9a-f]{8,}",
    Placeholder.FINGERPRINT: r"[0-9a-f]{3}",
    Placeholder.UUID: _UUID,
    Placeholder.TIMESTAMP: (
        r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?"
        r"(?:Z|[+-][0-9]{2}:[0-9]{2})?"
    ),
    Placeholder.TEMP: rf"/tmp/(?:{_ID_SEGMENT}(?:/{_ID_SEGMENT})*/?)?",
}

# The directories a family lays out under a workspace, beside its arms' fixture trees.
_LAYOUT_DIRS = (
    "project",
    "sink",
    "fxp",
    "home",
    "capture",
    "xdg",
    "config",
    "cache",
    "data",
    "state",
)

# A workspace directory's name: `workspace.create_workspace`'s timestamp, then the variant.
_WORKSPACE_STAMP = r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{6}"

_WORD = re.compile(r"\w+")
_HYPHENATED_WORD = re.compile(r"\w+(?:-\w+)+")
_PATH_SEGMENT = re.compile(r"[\w.-]+")
# A YAML-style `key: value` line, as frontmatter writes one; group 1 is the value.
_KEY_VALUE = re.compile(r"\s*[\w-]+:\s+(\S(?:.*\S)?)\s*")


def _template_regex(template: Template, context: _TemplateContext) -> str:
    """<template> as a regular expression, each placeholder admitting only its shape."""
    parts: list[str] = []
    for part in template.parts:
        match part:
            case str():
                parts.append(re.escape(part))
            case Id():
                parts.append(re.escape(part.prefix) + r"[A-Za-z0-9]+")
            case Placeholder.WORKSPACE:
                parts.append(_workspace_regex(context, encoded=False))
            case Placeholder.ENCODED_WORKSPACE:
                parts.append(_workspace_regex(context, encoded=True))
            case Placeholder.TOKEN:
                parts.append(rf"(?<![\w-])(?:{_alternation(context.tokens)})(?![\w-])")
            case Placeholder():
                parts.append(_FIXED_PATTERNS[part])
            case _:
                assert_never(part)
    return "".join(parts)


def _workspace_regex(context: _TemplateContext, *, encoded: bool) -> str:
    """A path under the sample workspace root naming only <context>'s own package, variants
    and path names, in slash form or as Claude Code encodes a working directory."""
    spell = _encoded if encoded else (lambda text: text)
    separator = "-" if encoded else "/"
    segment = _alternation({spell(name) for name in context.path_names})
    return (
        re.escape(spell(f"{SAMPLE_WORKSPACE_ROOT}/{context.package}/"))
        + _WORKSPACE_STAMP
        + "-"
        + f"(?:{_alternation({spell(v) for v in context.variants})})"
        # `create_workspace`'s suffix for a second workspace in the same second.
        + r"(?:-[0-9]+)?"
        + f"(?:{re.escape(separator)}(?:{segment}))*"
        + (r"/?" if not encoded else "")
    )


def _alternation(texts: Iterable[str]) -> str:
    """A pattern matching exactly one of <texts>, longest first; none when it is empty."""
    ordered = sorted(texts, key=lambda text: (-len(text), text))
    return "|".join(map(re.escape, ordered)) or r"(?!)"


def _directories(root: Path, depth: int | None = None) -> list[Path]:
    """Every directory under <root>, or only those <depth> levels down."""
    if not root.is_dir():
        return []
    found = [p for p in root.rglob("*", recurse_symlinks=True) if p.is_dir()]
    if depth is None:
        return found
    return [p for p in found if len(p.relative_to(root).parts) == depth]


def _probe_py_strings(path: Path) -> tuple[set[str], set[str]]:
    """Every `str` constant of the module at <path>, f-string parts included: those in code,
    and the docstrings."""
    try:
        tree = ast.parse(path.read_text(encoding="utf-8"))
    except OSError:
        return set(), set()
    docstring_nodes = {
        id(node.body[0].value)
        for node in ast.walk(tree)
        if isinstance(node, ast.Module | ast.ClassDef | ast.FunctionDef | ast.AsyncFunctionDef)
        and node.body
        and isinstance(node.body[0], ast.Expr)
        and isinstance(node.body[0].value, ast.Constant)
    }
    code: set[str] = set()
    docstrings: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            (docstrings if id(node) in docstring_nodes else code).add(node.value)
    return code, docstrings


def _probe_json_strings(path: Path) -> tuple[set[str], str | None]:
    """Every string value and key of the manifest at <path>, and its `question`."""
    try:
        document = json_values.loads(path.read_text(encoding="utf-8"))
    except OSError, ValueError:
        return set(), None
    question = document.get("question") if isinstance(document, dict) else None
    found: set[str] = set()
    for _, node in _nodes(document, ()):
        match node:
            case str():
                found.add(node)
            case dict():
                found.update(node)
            case _:
                pass
    return found, question if isinstance(question, str) else None


def _format_keys(definition: ProbeDefinition) -> tuple[frozenset[str], frozenset[str]]:
    """The arm names a sample of <definition> is keyed by, and the keys the family writes
    under each arm."""
    match definition:
        case ClaudeOtelProbe():
            return frozenset(a.name for a in definition.arms), frozenset({"requests", "responses"})
        case ClaudeInitProbe():
            return frozenset(a.name for a in definition.arms), frozenset({"init", "result"})
        case OpenCodeFakeProbe():
            return frozenset(a.name for a in definition.arms), frozenset({"requests"})
        case OpenCodeDebugProbe() | ManualProbe() | CustomProbe():
            return frozenset(), frozenset()
        case _:
            assert_never(definition)


def _unaccounted_units(
    value: JsonValue,
    path: _Path,
    found: Accounting,
    exempt: Callable[[_Path, str], bool],
) -> Iterator[tuple[str, str]]:
    match value:
        case dict():
            for key, item in value.items():
                if not exempt(path, key) and not found.vouches_key(key):
                    yield _path_text((*path, key)) + " (key)", key
                yield from _unaccounted_units(item, (*path, key), found, exempt)
        case list():
            for i, item in enumerate(value):
                yield from _unaccounted_units(item, (*path, i), found, exempt)
        case str() if "\n" not in value or found.vouches(value):
            if not found.vouches(value):
                yield _path_text(path), value
        case str():
            for number, line in enumerate(value.split("\n"), start=1):
                if not found.vouches(line):
                    yield f"{_path_text(path)} line {number}", line
        case _:
            pass


def _path_text(path: _Path) -> str:
    """<path> as a jq path: `.input.arm.requests[0]`."""
    text = ""
    for key in path:
        if isinstance(key, int):
            text += f"[{key}]"
        elif re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", key):
            text += f".{key}"
        else:
            text += f"[{json.dumps(key)}]"
    return text
