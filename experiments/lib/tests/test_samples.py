"""`samples.py` and `probe sample`: reduction, anonymization, the leak scan, writing and
regenerating samples, over stand-in packages built in the test's own tree."""

import json
import os
import textwrap
from pathlib import Path
from typing import Any

import pytest
from probe_harness import samples
from probe_harness.contract import SampleOrigin, parse_sample_file
from probe_harness.families import load_package
from probe_harness.gates.bodies import DEFERRED_LISTING, SUBAGENT_FLAG
from probe_harness.models.raw import ProviderModel, RawJson
from probe_harness.samples import (
    Identity,
    Needle,
    SampleRefused,
    Span,
    anonymize,
    leaks,
    reduce,
)

from tests.conftest import Cli

IDENTITY = Identity(home="/Users/tester", user="tester", email="tester@example.org", name="Tess")

MARK = "AGENTSPEC-PROBE-MARKER-SAMPLE"


class _Body(ProviderModel):
    name: str
    body: RawJson


def test_reduce_keeps_declared_keys_and_needle_strings_and_nulls_the_rest() -> None:
    source = {
        "name": "kept whole",
        "body": {"hit": f"a {MARK} b", "miss": "dropped", "n": 3},
        "extra": "undeclared and unmarked",
        "marked": {"deep": [f"x {MARK}", "blanked"]},
    }
    model = _Body.model_validate(source)

    reduced = reduce(model, (Needle(MARK, Span.LINE),), IDENTITY)

    assert reduced == {
        "name": "kept whole",
        "body": {"hit": f"a {MARK} b", "miss": "", "n": 3},
        "extra": None,
        "marked": {"deep": [f"x {MARK}", ""]},
    }
    assert isinstance(reduced, dict)
    assert _Body.model_validate(reduced).raw.keys() == model.raw.keys()


def test_reduce_keeps_only_the_listing_paragraph_without_connectors() -> None:
    text = "\n".join(
        [
            "Working directory: /Users/tester/x",
            "",
            DEFERRED_LISTING,
            "ToolSearch",
            "mcp__fx__alpha",
            "mcp__claude_ai_Docs__guide",
            "",
            "The following skills are available:",
            "- code-review: review code",
        ]
    )
    model = _Body.model_validate({"name": "n", "body": {"text": text}})

    reduced = reduce(model, samples.SEARCH_STRINGS, IDENTITY)

    assert reduced == {
        "name": "n",
        "body": {"text": "\n".join([DEFERRED_LISTING, "ToolSearch", "mcp__fx__alpha"])},
    }


def test_reduce_keeps_each_needle_line_once_in_order() -> None:
    text = "\n".join(
        [
            "preamble",
            DEFERRED_LISTING,
            "ToolSearch",
            f"marked {MARK}",
            "",
            "other paragraph",
            f"header {SUBAGENT_FLAG}; more",
            "trailing",
        ]
    )
    needles = (Needle(MARK, Span.LINE), *samples.SEARCH_STRINGS)
    model = _Body.model_validate({"name": "n", "body": [text]})

    reduced = reduce(model, needles, IDENTITY)

    assert reduced == {
        "name": "n",
        "body": [
            "\n".join(
                [DEFERRED_LISTING, "ToolSearch", f"marked {MARK}", f"header {SUBAGENT_FLAG}; more"]
            )
        ],
    }


def test_reduce_keeps_a_listing_running_to_the_end_of_its_string() -> None:
    text = "\n".join(["preamble", "", DEFERRED_LISTING, "ToolSearch", "mcp__fx__alpha"])
    model = _Body.model_validate({"name": "n", "body": [text]})

    reduced = reduce(model, samples.SEARCH_STRINGS, IDENTITY)

    assert reduced == {
        "name": "n",
        "body": ["\n".join([DEFERRED_LISTING, "ToolSearch", "mcp__fx__alpha"])],
    }


def test_reduce_keeps_a_needle_on_the_last_line() -> None:
    text = "\n".join(["first", "", DEFERRED_LISTING])
    model = _Body.model_validate({"name": "n", "body": [text, f"x\n{MARK}"]})

    reduced = reduce(model, (Needle(MARK, Span.LINE), *samples.SEARCH_STRINGS), IDENTITY)

    assert reduced == {"name": "n", "body": [DEFERRED_LISTING, MARK]}


def test_reduce_keeps_a_declared_string_whole() -> None:
    name = f"first line\n{MARK}\nthird line"
    model = _Body.model_validate({"name": name, "body": {}})

    assert reduce(model, (Needle(MARK, Span.LINE),), IDENTITY) == {"name": name, "body": {}}


@pytest.mark.parametrize(
    ("text", "want"),
    [
        ("/Users/tester/project/file", "/home/probe/project/file"),
        ("/private/var/folders/ab/cd123/T/tmp.x/ws", "/tmp/tmp.x/ws"),
        ("/var/folders/ab/cd123/T//ws", "/tmp/ws"),
        ("/private/tmp/ws", "/tmp/ws"),
        ("-private-var-folders-ab-cd123-T-ws", "-tmp-ws"),
        ("-Users-tester--local-state", "-home-probe--local-state"),
        ("owner tester wrote ~tester/x", "owner probe wrote ~probe/x"),
        ("testers and tester_x stay", "testers and tester_x stay"),
    ],
    ids=repr,
)
def test_anonymize_rewrites_each_personal_path_form(text: str, want: str) -> None:
    assert anonymize(text, IDENTITY) == want


def test_leaks_finds_each_pattern() -> None:
    text = json.dumps(
        {
            "a": "/Users/someone/x",
            "b": "/var/folders/zz",
            "c": "-private-var-folders-zz",
            "d": "mail me: x.y@example.com",
            "e": 'name "Tess" quoted',
            "f": "mcp__claude_ai_Docs__guide",
        }
    )
    found = leaks(text, IDENTITY)
    assert "path: /Users/" in found
    assert "path: /var/folders/" in found
    assert "path: var-folders" in found
    assert "email: x.y@example.com" in found
    assert "identity: Tess" in found
    assert "connector: mcp__claude_ai_" in found


def test_identity_current_reads_instruction_lines_and_skill_names_from_home(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    home = tmp_path / "home"
    (home / ".claude" / "skills" / "my-own-skill" / "nested-dir").mkdir(parents=True)
    (home / ".claude" / "skills" / "pdf").mkdir()
    commands = home / ".claude" / "commands"
    (commands / "ns").mkdir(parents=True)
    (commands / "review-pr.md").write_text("x", encoding="utf-8")
    (commands / "ns" / "do-thing.md").write_text("x", encoding="utf-8")
    (home / ".claude" / "CLAUDE.md").write_text(
        "short\nThis line of my personal instructions is long enough.\n", encoding="utf-8"
    )
    monkeypatch.setenv("HOME", str(home))
    gitconfig = tmp_path / "gitconfig"
    gitconfig.write_text("[user]\n\temail = me@example.net\n", encoding="utf-8")
    monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(gitconfig))
    monkeypatch.setenv("GIT_CONFIG_NOSYSTEM", "1")

    identity = Identity.current()

    assert identity.home == str(home)
    assert identity.email == "me@example.net"
    assert identity.instruction_lines == ("This line of my personal instructions is long enough.",)
    assert identity.skill_names == (
        "do-thing",
        "my-own-skill",
        "nested-dir",
        "ns:do-thing",
        "review-pr",
    )
    text = "x This line of my personal instructions is long enough. y my-own-skill"
    found = leaks(text, identity)
    assert "identity: This line of my personal instructions is long enough." in found
    assert "identity: my-own-skill" in found


DEBUG_PROBE_PY = textwrap.dedent("""\
    from pydantic import MISSING
    from probe_harness.families.opencode_debug import OpenCodeDebugProbe
    from probe_harness.models.raw import ProviderModel


    class Doc(ProviderModel):
        variant: str
        mode: str | MISSING = MISSING


    PROBE = OpenCodeDebugProbe(argv=("agent", "x"), output=Doc, assertion={assertion})
""")


def _debug_package(tree: Path, *, schema: int = 2, assertion: str = "lambda d: d.variant") -> Path:
    package = tree / "debug-pkg"
    (package / "fixtures" / "assertion").mkdir(parents=True)
    manifest: dict[str, Any] = {
        "schema_version": schema,
        "provider": "opencode",
        "driver": "unattended",
        "depth": "resolved-config",
        "question": "q",
        "version_source": {"kind": "none"},
        "assertion": {"expected": "high"},
    }
    if schema == 1:
        manifest["assertion"] = {"projection": ".variant", "expected": "high"}
        assertion = "None"
    (package / "probe.json").write_text(json.dumps(manifest), encoding="utf-8")
    (package / "probe.py").write_text(DEBUG_PROBE_PY.format(assertion=assertion))
    return package


def _debug_ws(tmp_path: Path, document: dict[str, Any]) -> Path:
    ws = tmp_path / "ws"
    ws.mkdir(exist_ok=True)
    (ws / "view.json").write_text(json.dumps(document), encoding="utf-8")
    return ws


def test_write_sample_writes_a_reduced_capture_with_the_results(tree: Path, tmp_path: Path) -> None:
    package = _debug_package(tree)
    ws = _debug_ws(tmp_path, {"variant": "high", "prompt": "/Users/tester/secret", "mode": "m"})
    definition, manifest = load_package(package)

    path = samples.write_sample(definition, manifest, ws, "confirming", identity=IDENTITY)

    sample = parse_sample_file(path)
    assert sample.origin == SampleOrigin.CAPTURE
    assert sample.input == {"variant": "high", "prompt": None, "mode": "m"}
    assert sample.results.model_dump(mode="json") == {"assertion": "high"}


def test_write_sample_refuses_to_overwrite(tree: Path, tmp_path: Path) -> None:
    package = _debug_package(tree)
    ws = _debug_ws(tmp_path, {"variant": "high"})
    definition, manifest = load_package(package)
    samples.write_sample(definition, manifest, ws, "confirming", identity=IDENTITY)

    with pytest.raises(SampleRefused, match="are never overwritten"):
        samples.write_sample(definition, manifest, ws, "confirming", identity=IDENTITY)


def test_write_sample_refuses_a_result_reduction_changed(tree: Path, tmp_path: Path) -> None:
    # The assertion searches a string the models do not declare, which reduction blanks.
    package = _debug_package(
        tree, assertion='lambda d: any("needle" in s for s in d.raw.strings())'
    )
    ws = _debug_ws(tmp_path, {"variant": "high", "prompt": "a needle"})
    definition, manifest = load_package(package)

    with pytest.raises(SampleRefused) as exc:
        samples.write_sample(definition, manifest, ws, "confirming", identity=IDENTITY)
    assert exc.value.lines[1] == "  assertion: full true · reduced false"
    assert not (package / "samples").exists()


def test_write_sample_refuses_a_leak(tree: Path, tmp_path: Path) -> None:
    package = _debug_package(tree)
    # A name is not one of the forms anonymization rewrites, so only the scan catches it.
    ws = _debug_ws(tmp_path, {"variant": "high", "mode": "written by Tess"})
    definition, manifest = load_package(package)

    with pytest.raises(SampleRefused) as exc:
        samples.write_sample(definition, manifest, ws, "confirming", identity=IDENTITY)
    assert "  identity: Tess" in exc.value.lines
    assert not (package / "samples").exists()


def test_a_v1_package_s_results_come_from_its_jq(tree: Path, tmp_path: Path) -> None:
    package = _debug_package(tree, schema=1)
    ws = _debug_ws(tmp_path, {"variant": "low"})
    definition, manifest = load_package(package)

    path = samples.write_sample(definition, manifest, ws, "refuting", identity=IDENTITY)

    assert parse_sample_file(path).results.model_dump(mode="json") == {"assertion": "low"}


def test_regenerate_rewrites_only_results(tree: Path, tmp_path: Path) -> None:
    package = _debug_package(tree, schema=1)
    definition, manifest = load_package(package)
    samples.write_sample(
        definition,
        manifest,
        _debug_ws(tmp_path, {"variant": "high"}),
        "edited-x",
        identity=IDENTITY,
    )
    path = package / "samples" / "edited-x.json"
    document = json.loads(path.read_text())
    document.update(origin="edited", note="variant changed by hand")
    document["input"]["variant"] = "low"
    path.write_text(json.dumps(document), encoding="utf-8")

    changes = samples.regenerate(definition, manifest)

    assert changes == ["edited-x.json: assertion: high → low"]
    rewritten = json.loads(path.read_text())
    assert rewritten == {**document, "results": {"assertion": "low"}}


MANUAL_PROBE_PY = textwrap.dedent("""\
    from probe_harness.families.manual import ManualProbe
    from probe_harness.models.raw import ProviderModel


    class Line(ProviderModel):
        hook_event_name: str


    PROBE = ManualProbe(
        instructions="",
        payload=Line,
        wait_for=lambda lines: any(
            getattr(line, "hook_event_name", None) == "stop" for line in lines
        ),
        assertion=lambda lines: len(lines),
    )
""")


def _manual_package(tree: Path) -> Path:
    package = tree / "manual-pkg"
    (package / "fixtures" / "assertion").mkdir(parents=True)
    (package / "fixtures" / "capture").mkdir(parents=True)
    (package / "fixtures" / "capture" / "dump-hook.sh").write_text("#!/bin/sh\n")
    manifest = {
        "schema_version": 2,
        "provider": "cursor",
        "driver": "manual",
        "depth": None,
        "question": "q",
        "version_source": {"kind": "none"},
        "assertion": {"expected": 2},
    }
    (package / "probe.json").write_text(json.dumps(manifest), encoding="utf-8")
    (package / "probe.py").write_text(MANUAL_PROBE_PY)
    return package


def _capture(tmp_path: Path, *lines: str) -> Path:
    ws = tmp_path / "ws"
    (ws / "capture").mkdir(parents=True)
    (ws / "capture" / "payloads.jsonl").write_text("".join(f"{line}\n" for line in lines))
    return ws


def test_a_manual_sample_and_its_prefixes(tree: Path, tmp_path: Path) -> None:
    package = _manual_package(tree)
    ws = _capture(tmp_path, '{"hook_event_name":"start"}', '{"hook_event_name":"stop"}')
    definition, manifest = load_package(package)

    full = samples.write_sample(definition, manifest, ws, "confirming", identity=IDENTITY)
    before = samples.write_sample(
        definition, manifest, ws, "before-stop", lines=1, identity=IDENTITY
    )
    empty = samples.write_sample(definition, manifest, ws, "empty", lines=0, identity=IDENTITY)

    assert parse_sample_file(full).results.model_dump(mode="json") == {
        "assertion": 2,
        "wait_for": True,
    }
    prefix = parse_sample_file(before)
    assert prefix.origin == SampleOrigin.PREFIX
    assert prefix.input == [{"hook_event_name": "start"}]
    assert prefix.results.model_dump(mode="json") == {"assertion": 1, "wait_for": False}
    assert parse_sample_file(empty).input == []


def test_a_manual_sample_refuses_an_unrecognized_line(tree: Path, tmp_path: Path) -> None:
    package = _manual_package(tree)
    ws = _capture(tmp_path, '{"hook_event_name":"start"}', '{"other":1}')
    definition, manifest = load_package(package)

    with pytest.raises(SampleRefused) as exc:
        samples.write_sample(definition, manifest, ws, "confirming", identity=IDENTITY)
    assert exc.value.lines[1] == "  line 2: hook_event_name: Field required"


FAKE_PROBE_PY = textwrap.dedent(f"""\
    from probe_harness.families.base import CustomCheck
    from probe_harness.families.opencode_fake import OpenCodeArm, OpenCodeArms, OpenCodeFakeProbe
    from probe_harness.gates.opencode import governed_requests


    def _governed(arms: OpenCodeArms) -> int:
        return len(governed_requests(arms["one"].requests, "{MARK}"))


    PROBE = OpenCodeFakeProbe(
        marker="{MARK}",
        arms=(OpenCodeArm("one", "agent"),),
        assertion=_governed,
        custom_checks=(CustomCheck("some", lambda arms: _governed(arms) > 0, ("none",)),),
    )
""")


def test_a_fake_opencode_sample_keeps_the_marker_its_assertion_searches(
    tree: Path, tmp_path: Path
) -> None:
    package = tree / "fake-pkg"
    (package / "fixtures" / "assertion").mkdir(parents=True)
    manifest = {
        "schema_version": 2,
        "provider": "opencode",
        "driver": "unattended",
        "depth": "outbound-request",
        "question": "q",
        "version_source": {"kind": "none"},
        "assertion": {"expected": 1},
    }
    (package / "probe.json").write_text(json.dumps(manifest), encoding="utf-8")
    (package / "probe.py").write_text(FAKE_PROBE_PY)
    sink = tmp_path / "ws" / "one" / "sink"
    sink.mkdir(parents=True)
    governed = {"messages": [{"role": "system", "content": f"you are {MARK}"}], "x": "y"}
    title = {"messages": [{"role": "system", "content": "make a title"}]}
    (sink / "0000.request.json").write_text(json.dumps(governed))
    (sink / "0001.request.json").write_text(json.dumps(title))
    definition, loaded = load_package(package)

    path = samples.write_sample(
        definition, loaded, tmp_path / "ws", "confirming", identity=IDENTITY
    )

    sample = parse_sample_file(path)
    assert sample.input == {
        "one": {
            "requests": [
                {"messages": [{"role": "system", "content": f"you are {MARK}"}], "x": None},
                # `content` is a declared string, which a sample keeps whole.
                {"messages": [{"role": "system", "content": "make a title"}]},
            ]
        }
    }
    assert sample.results.model_dump(mode="json") == {
        "assertion": 1,
        "custom_checks": {"some": True},
    }


def test_the_cli_writes_a_sample_and_refuses_usage_errors(
    cli: Cli, tree: Path, tmp_path: Path
) -> None:
    _debug_package(tree)
    ws = _debug_ws(tmp_path, {"variant": "high"})
    env = {"HOME": str(tmp_path / "home"), "GIT_CONFIG_GLOBAL": os.devnull}

    written = cli("sample", "debug-pkg", ws, "confirming", env=env)
    assert written.returncode == 0, written.output
    assert written.stdout == f"{tree / 'debug-pkg' / 'samples' / 'confirming.json'}\n"

    lines = cli("sample", "debug-pkg", ws, "other", "--lines", "1", env=env)
    assert lines.returncode == 2
    assert "--lines applies only to a manual package" in lines.stderr

    regenerate = cli("sample", "debug-pkg", ws, "--regenerate", env=env)
    assert regenerate.returncode == 2
    assert "--regenerate takes only a package" in regenerate.stderr

    missing = cli("sample", "debug-pkg", env=env)
    assert missing.returncode == 2

    again = cli("sample", "debug-pkg", "--regenerate", env=env)
    assert again.returncode == 1
    assert "is schema 2" in again.stderr


def test_regenerate_refuses_a_schema_2_package(tree: Path, tmp_path: Path) -> None:
    package = _debug_package(tree)
    definition, manifest = load_package(package)
    samples.write_sample(
        definition,
        manifest,
        _debug_ws(tmp_path, {"variant": "high"}),
        "confirming",
        identity=IDENTITY,
    )
    before = (package / "samples" / "confirming.json").read_text()

    with pytest.raises(SampleRefused, match="is schema 2"):
        samples.regenerate(definition, manifest)
    assert (package / "samples" / "confirming.json").read_text() == before


@pytest.mark.parametrize("name", ["../escape", "a/b", "Confirming", "", "-x"], ids=repr)
def test_a_sample_name_must_be_a_plain_slug(tree: Path, tmp_path: Path, name: str) -> None:
    package = _debug_package(tree)
    definition, manifest = load_package(package)
    with pytest.raises(SampleRefused, match="a sample name is"):
        samples.write_sample(
            definition, manifest, _debug_ws(tmp_path, {"variant": "high"}), name, identity=IDENTITY
        )


def test_a_lone_surrogate_survives_sampling_ascii_escaped(tree: Path, tmp_path: Path) -> None:
    # A body cut between the halves of an emoji carries an escaped lone surrogate. (jq
    # refuses one, so a schema 1 package's judge and sample both fail on it alike.)
    package = _debug_package(tree)
    ws = tmp_path / "ws"
    ws.mkdir()
    (ws / "view.json").write_text('{"variant": "low", "mode": "cut \\ud83d"}', encoding="utf-8")
    definition, manifest = load_package(package)

    path = samples.write_sample(definition, manifest, ws, "refuting", identity=IDENTITY)

    assert "\\ud83d" in path.read_text(encoding="utf-8")
    assert parse_sample_file(path).input == {"variant": "low", "mode": "cut \ud83d"}


def test_a_poll_counts_a_line_cut_inside_a_character_as_not_yet(tree: Path, tmp_path: Path) -> None:
    from probe_harness.families import manual

    package = _manual_package(tree)
    definition, manifest = load_package(package)
    ws = _capture(tmp_path)
    payloads = ws / "capture" / "payloads.jsonl"
    # A hook mid-write: no newline yet, and the bytes end inside a character.
    with payloads.open("ab") as f:
        f.write('{"hook_event_name":"stop","x":"—'.encode()[:-1])
    assert isinstance(definition, manual.ManualProbe)

    assert manual._poll(manifest, ws, definition)() is False
