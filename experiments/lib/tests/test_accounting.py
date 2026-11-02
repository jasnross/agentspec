"""The accounting check (`samples.unaccounted`, `Accounting.vouches`) over a stand-in package.

The coverage tests show that the accounting check flags everything the identity scan it
replaced would have flagged in captured content, in each place a capture can carry it, and
that `authored_text_leaks` flags the kinds needing no knowledge of the machine in a sample's
`note` and `authored` list.
"""

import json
import os
import textwrap
from pathlib import Path
from typing import Any

import pytest
from probe_harness import provider_text
from probe_harness.contract import Sample, SampleOrigin
from probe_harness.families import ProbeDefinition, load_package, parse_workspace
from probe_harness.json_values import JsonValue
from probe_harness.provider_text import Entry, Id, Placeholder, Template
from probe_harness.samples import (
    Identity,
    _source_of,
    accounting,
    anonymize_document,
    authored_text_leaks,
    document,
    evaluate,
    unaccounted,
)
from probe_harness.workspace import SAMPLE_WORKSPACE_ROOT

from tests.conftest import EXPERIMENTS_DIR

PACKAGE = "acct-pkg"
WORKSPACE = f"{SAMPLE_WORKSPACE_ROOT}/{PACKAGE}/2026-11-01T000000-assertion"
ENCODED_WORKSPACE = WORKSPACE.replace("/", "-").replace(".", "-")

PROBE_PY = textwrap.dedent("""\
    '''A stand-in probe whose docstring mentions a review.'''

    from probe_harness.families.opencode_debug import OpenCodeDebugProbe
    from probe_harness.families.opencode_env import OpenCodeIsolation
    from probe_harness.models.raw import ProviderModel, RawJson

    _PHRASE = "across the board"
    _ARRANGED = "first arranged line\\nsecond arranged line"


    class Doc(ProviderModel):
        variant: str
        body: RawJson


    PROBE = OpenCodeDebugProbe(
        argv=("x",), output=Doc, isolation=OpenCodeIsolation(), assertion=lambda d: d.variant
    )
""")

FIXTURE_FILE = "first line\nsecond line\n"

# One synthetic value of each kind the identity scan matched.
SCAN_KINDS = {
    "home path": "/Users/tester",
    "username": "tester",
    "email": "tester@example.org",
    "name": "Tess Tester",
    "instruction line": "Always answer in the voice of a pirate captain.",
    "skill name": "my-own-skill",
    "macOS home": "/Users/",
    "macOS temp": "/var/folders/",
    "encoded macOS temp": "var-folders",
    "connector": "mcp__claude_ai_x",
}

# Each form a value can take where a capture spells it inside a path.
FORMS = {
    "alone": "{}",
    "in a workspace path": f"{WORKSPACE}/project/{{}}",
    "under /tmp": "/tmp/{}",
    "in an encoded workspace": f"{ENCODED_WORKSPACE}-project-{{}}",
}

# The machine-independent kinds `authored_text_leaks` checks a note and an authored list for.
AUTHORED_KINDS = {
    "email": "tester@example.org",
    "macOS home": "/Users/x",
    "macOS temp": "/var/folders/x",
    "encoded macOS temp": "-private-var-folders-x",
    "connector": "mcp__claude_ai_x",
}


@pytest.fixture
def package(tree: Path) -> Path:
    package = tree / PACKAGE
    project = package / "fixtures" / "assertion" / "project"
    project.mkdir(parents=True)
    (project / "notes.txt").write_text("alpha beta\n", encoding="utf-8")
    (project / "AGENTS.md").write_text(FIXTURE_FILE, encoding="utf-8")
    # The keys of the stand-in document, which a sample keeps.
    (project / "opencode.json").write_text('{"variant": "", "body": {"note": ""}}\n')
    manifest = {
        "schema_version": 2,
        "provider": "opencode",
        "driver": "unattended",
        "depth": "resolved-config",
        "question": "q",
        "version_source": {"kind": "none"},
        "assertion": {"expected": "high"},
    }
    (package / "probe.json").write_text(json.dumps(manifest), encoding="utf-8")
    (package / "probe.py").write_text(PROBE_PY, encoding="utf-8")
    return package


def _flagged(package: Path, input: JsonValue, results: dict[str, Any] | None = None) -> list[str]:
    definition, _ = load_package(package)
    sample = Sample.model_validate(
        {"origin": "capture", "input": input, "results": results or {"assertion": "high"}}
    )
    return [text for _, text in unaccounted(package, definition, sample)]


@pytest.mark.parametrize("form", FORMS.values(), ids=list(FORMS))
@pytest.mark.parametrize("kind", SCAN_KINDS.values(), ids=list(SCAN_KINDS))
def test_every_scan_kind_is_unaccounted_wherever_a_capture_carries_it(
    package: Path, kind: str, form: str
) -> None:
    placed = form.format(kind)

    assert _flagged(package, {"variant": placed, "body": {}}) == [placed], "a model string"
    assert _flagged(package, {"variant": "", "body": {"note": placed}}) == [placed], "a raw string"
    assert _flagged(package, {"variant": "", "body": {placed: ""}}) == [placed], "a raw key"
    assert _flagged(package, {"variant": "", "body": {}}, {"assertion": placed}) == [placed], (
        "a result"
    )


@pytest.mark.parametrize("kind", AUTHORED_KINDS.values(), ids=list(AUTHORED_KINDS))
def test_every_machine_independent_kind_is_flagged_in_authored_text(kind: str) -> None:
    in_note = Sample.model_validate(
        {
            "origin": "edited",
            "note": f"replaced by {kind}",
            "input": {"variant": ""},
            "results": {"assertion": ""},
        }
    )
    in_authored = Sample.model_validate(
        {
            "origin": "edited",
            "note": "replaced by hand",
            "authored": [kind],
            "input": {"variant": kind},
            "results": {"assertion": ""},
        }
    )

    assert [hit.split(":")[0] for hit in authored_text_leaks(in_note)] == ["note"]
    assert [hit.split(":")[0] for hit in authored_text_leaks(in_authored)] == ["authored"]


def test_prose_about_the_operator_s_configuration_is_unaccounted(package: Path) -> None:
    report = (
        "I read the user's global instructions file before starting.\n"
        "It also lists their claude.ai connectors."
    )

    assert _flagged(package, {"variant": "", "body": {"note": report}}) == report.split("\n")


@pytest.mark.parametrize(
    ("part", "shape", "word"),
    [
        (Placeholder.DIGITS, "123", "hello"),
        (Placeholder.HEX, "0123abcd", "hello"),
        (Placeholder.FINGERPRINT, "2cb", "hello"),
        (Placeholder.UUID, "e8ed806a-de39-413c-b479-b4d88fca5c50", "hello"),
        (Placeholder.TIMESTAMP, "2026-11-01T12:00:00.5Z", "hello"),
        (Placeholder.WORKSPACE, f"{WORKSPACE}/project/notes.txt", f"{WORKSPACE}/hello"),
        (
            Placeholder.ENCODED_WORKSPACE,
            f"{ENCODED_WORKSPACE}-project-notes-txt",
            f"{ENCODED_WORKSPACE}-hello",
        ),
        (Placeholder.TEMP, "/tmp/12345/0123abcd", "/tmp/hello"),
        (Placeholder.TOKEN, "alpha", "hello"),
        (Id("toolu_"), "toolu_01AbC", "other_01AbC"),
    ],
    ids=repr,
)
def test_each_placeholder_admits_its_shape_and_no_free_word(
    package: Path,
    monkeypatch: pytest.MonkeyPatch,
    part: Placeholder | Id,
    shape: str,
    word: str,
) -> None:
    monkeypatch.setattr(
        provider_text, "CATALOG", (Entry("harness", "a test entry", Template(("<", part, ">"))),)
    )
    found = accounting(package)

    assert found.vouches(f"<{shape}>")
    assert not found.vouches(f"<{word}>")


def test_vouches_tests_equality_with_whole_authored_text(package: Path) -> None:
    found = accounting(package)

    assert found.vouches("across the board")
    assert found.vouches("alpha beta")
    for text in ("ross", "alpha", "beta", "beta alpha"):
        assert not found.vouches(text), text


def test_a_whole_fixture_file_is_one_unit_and_a_changed_line_is_flagged_alone(
    package: Path,
) -> None:
    assert _flagged(package, {"variant": "", "body": {"note": FIXTURE_FILE}}) == []

    changed = FIXTURE_FILE.replace("second line", "second line, edited")
    definition, _ = load_package(package)
    sample = Sample.model_validate(
        {
            "origin": "capture",
            "input": {"variant": "", "body": {"note": changed}},
            "results": {"assertion": "high"},
        }
    )
    assert unaccounted(package, definition, sample) == [
        (".input.body.note line 2", "second line, edited")
    ]


def test_a_key_value_fixture_line_vouches_for_its_value(package: Path) -> None:
    agent = package / "fixtures" / "assertion" / "project" / "agent.md"
    agent.write_text("---\ndescription: a probe agent that reads files\n---\n", encoding="utf-8")

    found = accounting(package)

    assert found.vouches("a probe agent that reads files")
    assert not found.vouches("a probe agent")


def test_a_key_is_vouched_for_as_a_whole_authored_word_and_a_value_is_not(package: Path) -> None:
    found = accounting(package)

    assert found.vouches_key("alpha")
    assert not found.vouches("alpha")
    assert not found.vouches_key("alph")


def test_the_kept_report_is_flagged() -> None:
    """The subagent report that reached `claude-agent-effort`'s refuting sample, summarizing the
    operator's configuration, is flagged line by line. Its kept capture is on one machine only,
    so this runs only when `PROBE_KEPT_REPORT_WS` names that workspace."""
    ws = os.environ.get("PROBE_KEPT_REPORT_WS")
    if not ws:
        pytest.skip("PROBE_KEPT_REPORT_WS names no kept claude-agent-effort workspace")
    package = EXPERIMENTS_DIR / "claude-agent-effort"
    definition: ProbeDefinition
    definition, _ = load_package(package)
    value = parse_workspace(definition, Path(ws), None)
    identity = Identity.current()
    uncut = document(definition, value, lambda body: anonymize_document(_source_of(body), identity))
    sample = Sample(origin=SampleOrigin.CAPTURE, input=uncut, results=evaluate(definition, value))

    report = [text for text in _strings(uncut) if text.startswith("[Subagent hand-back]")]
    assert report, "the workspace holds no subagent report"
    flagged = {text for _, text in unaccounted(package, definition, sample)}
    for text in report:
        lines = [line for line in text.split("\n") if line.strip()]
        assert lines
        assert all(line in flagged for line in lines)


def _strings(value: JsonValue) -> list[str]:
    match value:
        case str():
            return [value]
        case list():
            return [text for item in value for text in _strings(item)]
        case dict():
            return [text for item in value.values() for text in _strings(item)]
        case _:
            return []


def test_each_line_of_a_multi_line_literal_is_vouched_for(package: Path) -> None:
    found = accounting(package)

    assert found.vouches("second arranged line")
    assert found.vouches("first arranged line\nsecond arranged line")


def test_a_docstring_word_vouches_for_no_key(package: Path) -> None:
    found = accounting(package)

    assert found.vouches("A stand-in probe whose docstring mentions a review.")
    assert not found.vouches_key("review")
