"""The manifest-and-definition agreement table: each disagreement is refused by `probe run`
before any workspace exists, since each could otherwise fail only once a run had begun."""

import json
import textwrap
from pathlib import Path
from typing import Any

import pytest

from tests.conftest import Cli, workspaces

MANUAL = textwrap.dedent("""\
    from probe_harness.families.manual import ManualProbe
    from probe_harness.models.raw import ProviderModel


    class Line(ProviderModel):
        hook_event_name: str


    PROBE = ManualProbe(instructions="", {fields})
""")

DEBUG = textwrap.dedent("""\
    from probe_harness.families.base import CustomCheck, JqCustomCheck
    from probe_harness.families.opencode_debug import OpenCodeDebugProbe
    from probe_harness.models.raw import ProviderModel


    class Doc(ProviderModel):
        variant: str


    PROBE = OpenCodeDebugProbe(argv=("agent", "x"), output=Doc, {fields})
""")

PAYLOAD = "payload=Line"
WAIT_FOR = "wait_for=lambda lines: True"
ASSERTION = "assertion=lambda value: 1"
CAPTURE_VERSION = "capture_version=lambda lines: None"
CHECK = 'custom_checks=(CustomCheck("c", lambda d: True, ("no",)),)'
JQ_CHECK = 'custom_checks=(JqCustomCheck("c", "true", ("no",)),)'
TWO_CHECKS = (
    'custom_checks=(CustomCheck("c", lambda d: True, ("no",)), '
    'CustomCheck("c", lambda d: True, ("no",)))'
)

V1_MANUAL = {
    "schema_version": 1,
    "wait_for": "true",
    "assertion": {"projection": ".", "expected": 1},
}
V2_MANUAL = {"schema_version": 2, "assertion": {"expected": 1}}
V2_OPTIONS = {
    "schema_version": 2,
    "assertion": {"options": [{"id": "a", "text": "A"}], "expected": "a"},
}
V1_DEBUG = {"schema_version": 1, "assertion": {"projection": ".", "expected": 1}}
V2_DEBUG = {"schema_version": 2, "assertion": {"expected": 1}}

CASES: list[tuple[str, str, list[str], dict[str, Any], str]] = [
    (
        "v1 forbids an assertion",
        MANUAL,
        [ASSERTION],
        V1_MANUAL,
        "pkg's definition carries an assertion, but its manifest is schema 1",
    ),
    (
        "v1 forbids a definition wait_for",
        MANUAL,
        [PAYLOAD, WAIT_FOR],
        V1_MANUAL,
        "pkg's definition carries a wait_for, but its manifest is schema 1",
    ),
    (
        "v1 forbids a capture_version",
        MANUAL,
        [CAPTURE_VERSION],
        V1_MANUAL,
        "pkg's definition carries a capture_version, but its manifest is schema 1",
    ),
    (
        "v1 forbids a Python custom check",
        DEBUG,
        [CHECK],
        V1_DEBUG,
        "pkg's definition carries a Python custom check, but its manifest is schema 1",
    ),
    (
        "v1 manual requires a manifest wait_for",
        MANUAL,
        [],
        {**V1_MANUAL, "wait_for": None},
        "manifest declares no wait_for filter",
    ),
    (
        "v2 expected requires an assertion",
        DEBUG,
        [],
        V2_DEBUG,
        "pkg's manifest is schema 2 and declares expected, but an OpenCodeDebugProbe",
    ),
    (
        "v2 options forbids an assertion",
        MANUAL,
        [PAYLOAD, WAIT_FOR, ASSERTION],
        V2_OPTIONS,
        "pkg's manifest declares options, which a person answers, but its definition carries",
    ),
    (
        "v2 manual requires a payload",
        MANUAL,
        [WAIT_FOR, ASSERTION],
        V2_MANUAL,
        "pkg's manifest is schema 2, but its definition carries no payload",
    ),
    (
        "v2 manual requires a wait_for",
        MANUAL,
        [PAYLOAD, ASSERTION],
        V2_MANUAL,
        "pkg's manifest is schema 2, but its definition carries no wait_for",
    ),
    (
        "v2 capture source requires a capture_version",
        MANUAL,
        [PAYLOAD, WAIT_FOR, ASSERTION],
        {**V2_MANUAL, "version_source": {"kind": "capture"}},
        "pkg's manifest declares a capture version source, but its definition carries no "
        "capture_version",
    ),
    (
        "v2 other source forbids a capture_version",
        MANUAL,
        [PAYLOAD, WAIT_FOR, ASSERTION, CAPTURE_VERSION],
        V2_MANUAL,
        "pkg's definition carries a capture_version, but its manifest's version source is not "
        "a capture",
    ),
    (
        "v2 forbids a jq custom check",
        DEBUG,
        [ASSERTION, JQ_CHECK],
        V2_DEBUG,
        "pkg's manifest is schema 2, but its definition carries the jq custom check c",
    ),
    (
        "custom check names are unique",
        DEBUG,
        [ASSERTION, TWO_CHECKS],
        V2_DEBUG,
        "pkg's definition declares custom check c more than once",
    ),
]


@pytest.mark.parametrize(
    ("template", "fields", "manifest", "message"),
    [case[1:] for case in CASES],
    ids=[case[0] for case in CASES],
)
def test_a_disagreement_is_refused_before_any_workspace(
    cli: Cli,
    tree: Path,
    opencode_debug_stub: Path,
    template: str,
    fields: list[str],
    manifest: dict[str, Any],
    message: str,
) -> None:
    package = tree / "pkg"
    (package / "fixtures" / "assertion").mkdir(parents=True)
    manual = template is MANUAL
    if manual:
        (package / "fixtures" / "capture").mkdir()
        (package / "fixtures" / "capture" / "dump-hook.sh").write_text("#!/bin/sh\n")
    document: dict[str, Any] = {
        "provider": "cursor" if manual else "opencode",
        "driver": "manual" if manual else "unattended",
        "depth": None,
        "question": "q",
        "version_source": {"kind": "none"},
        **manifest,
    }
    # A `wait_for` of None stands for one the manifest leaves out.
    if document.get("wait_for", "") is None:
        del document["wait_for"]
    (package / "probe.json").write_text(json.dumps(document), encoding="utf-8")
    (package / "probe.py").write_text(template.format(fields=", ".join(fields)), encoding="utf-8")

    result = cli("run", "pkg")

    assert result.returncode == 1, result.output
    assert f"probe run: {message}" in result.stderr
    assert workspaces("pkg") == []
