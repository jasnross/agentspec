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
    from probe_harness.families.base import CustomCheck
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
TWO_CHECKS = (
    'custom_checks=(CustomCheck("c", lambda d: True, ("no",)), '
    'CustomCheck("c", lambda d: True, ("no",)))'
)

MANUAL_EXPECTED = {"schema_version": 2, "assertion": {"expected": 1}}
MANUAL_OPTIONS = {
    "schema_version": 2,
    "assertion": {"options": [{"id": "a", "text": "A"}], "expected": "a"},
}
DEBUG_EXPECTED = {"schema_version": 2, "assertion": {"expected": 1}}

CASES: list[tuple[str, str, list[str], dict[str, Any], str]] = [
    (
        "expected requires an assertion",
        DEBUG,
        [],
        DEBUG_EXPECTED,
        "pkg's manifest declares expected, but an OpenCodeDebugProbe",
    ),
    (
        "options forbids an assertion",
        MANUAL,
        [PAYLOAD, WAIT_FOR, ASSERTION],
        MANUAL_OPTIONS,
        "pkg's manifest declares options, which a person answers, but its definition carries",
    ),
    (
        "capture source requires a capture_version",
        MANUAL,
        [PAYLOAD, WAIT_FOR, ASSERTION],
        {**MANUAL_EXPECTED, "version_source": {"kind": "capture"}},
        "pkg's manifest declares a capture version source, but its definition carries no "
        "capture_version",
    ),
    (
        "other source forbids a capture_version",
        MANUAL,
        [PAYLOAD, WAIT_FOR, ASSERTION, CAPTURE_VERSION],
        MANUAL_EXPECTED,
        "pkg's definition carries a capture_version, but its manifest's version source is not "
        "a capture",
    ),
    (
        "custom check names are unique",
        DEBUG,
        [ASSERTION, TWO_CHECKS],
        DEBUG_EXPECTED,
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
    (package / "probe.json").write_text(json.dumps(document), encoding="utf-8")
    (package / "probe.py").write_text(template.format(fields=", ".join(fields)), encoding="utf-8")

    result = cli("run", "pkg")

    assert result.returncode == 1, result.output
    assert f"probe run: {message}" in result.stderr
    assert workspaces("pkg") == []
