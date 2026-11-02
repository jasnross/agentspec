"""`samples.py` and `probe sample`: the removal test, anonymization, the accounting check,
writing and regenerating samples, over stand-in packages built in the test's own tree."""

import json
import os
import textwrap
from pathlib import Path
from typing import Any

import pytest
from probe_harness import samples
from probe_harness.contract import SampleOrigin, parse_sample_file
from probe_harness.families import ProbeDefinition, load_package, parse_sample
from probe_harness.gates import GateFailure
from probe_harness.json_values import JsonValue
from probe_harness.samples import (
    Identity,
    SampleRefused,
    anonymize,
)

from tests.conftest import Cli

IDENTITY = Identity(home="/Users/tester", user="tester")

MARK = "AGENTSPEC-PROBE-MARKER-SAMPLE"

CUT_PROBE_PY = textwrap.dedent("""\
    from typing import Annotated

    from pydantic import Field
    from probe_harness.families.opencode_debug import OpenCodeDebugProbe
    from probe_harness.families.opencode_env import OpenCodeIsolation
    from probe_harness.models.raw import ProviderModel, RawJson, tag, type_is


    class Text(ProviderModel):
        type: str
        text: str


    class Other(ProviderModel):
        type: str


    type Block = Annotated[
        Annotated[Text, tag("text")] | Annotated[Other, tag("other")], type_is("text")
    ]


    class Doc(ProviderModel):
        variant: str
        note: str
        body: RawJson
        blocks: Annotated[tuple[Block, ...], Field(strict=False)]


    PROBE = OpenCodeDebugProbe(
        argv=("x",), output=Doc, isolation=OpenCodeIsolation(), assertion={assertion}
    )
""")


def _cut_definition(tree: Path, assertion: str = "lambda d: d.variant") -> ProbeDefinition:
    package = tree / "cut-pkg"
    (package / "fixtures" / "assertion").mkdir(parents=True)
    manifest: dict[str, Any] = {
        "schema_version": 2,
        "provider": "opencode",
        "driver": "unattended",
        "depth": "resolved-config",
        "question": "q",
        "version_source": {"kind": "none"},
        "assertion": {"expected": "high"},
    }
    (package / "probe.json").write_text(json.dumps(manifest), encoding="utf-8")
    (package / "probe.py").write_text(CUT_PROBE_PY.format(assertion=assertion), encoding="utf-8")
    definition, _ = load_package(package)
    return definition


def _doc(**fields: Any) -> dict[str, Any]:
    return {"variant": "high", "note": "", "body": {}, "blocks": [], **fields}


def _cut(definition: ProbeDefinition, document: dict[str, Any]) -> JsonValue:
    """<document> cut against what the definition's expressions yield on it."""
    oracle = samples.evaluate(definition, parse_sample(definition, document, "t"))
    return samples.cut(definition, document, oracle, "t")


def test_cut_empties_a_modelled_string_no_expression_reads(tree: Path) -> None:
    definition = _cut_definition(tree)

    assert _cut(definition, _doc(note="unread")) == _doc()


def test_cut_keeps_only_the_line_of_a_string_an_expression_needs(tree: Path) -> None:
    definition = _cut_definition(tree, 'lambda d: "needle" in d.note')

    cut = _cut(definition, _doc(note="first\nhas needle\nlast"))

    assert cut == _doc(variant="", note="has needle")


def test_cut_removes_raw_entries_and_keeps_the_undeclared_key(tree: Path) -> None:
    definition = _cut_definition(tree)
    document = _doc(body={"k": "v", "n": {"deep": 1}}, extra={"a": 1, "b": {"c": "x"}})

    cut = _cut(definition, document)

    assert cut == _doc(body={}, extra={})


def test_cut_never_empties_a_type_tag_that_selects_a_model(tree: Path) -> None:
    # Emptying `"text"` would parse the block as `Other`, which no expression reads; the
    # class change rejects the removal, so the block keeps its tag and `text` field.
    definition = _cut_definition(tree)

    cut = _cut(definition, _doc(blocks=[{"type": "text", "text": "body"}, {"type": "image"}]))

    assert cut == _doc(blocks=[{"type": "text", "text": ""}, {"type": ""}])


def test_cut_is_a_fixed_point(tree: Path) -> None:
    definition = _cut_definition(tree, 'lambda d: [b.text for b in d.blocks if b.type == "text"]')
    document = _doc(
        note="a\nb",
        body={"k": ["x", {"y": "z"}]},
        blocks=[{"type": "text", "text": "kept\nalso kept"}, {"type": "text", "text": "x"}],
        extra="dropped",
    )
    oracle = samples.evaluate(definition, parse_sample(definition, document, "t"))

    once = samples.cut(definition, document, oracle, "t")

    assert samples.cut(definition, once, oracle, "t") == once
    assert once == _doc(
        variant="",
        blocks=[{"type": "text", "text": "kept\nalso kept"}, {"type": "text", "text": "x"}],
        extra="",
    )


def test_cut_holds_a_non_monotone_expression_to_its_oracle(tree: Path) -> None:
    # Removing the `Y` line or the `Z` line alone flips the result, but removing both
    # restores it; the `X` line is needed throughout, so `note` is never emptied whole.
    definition = _cut_definition(
        tree, 'lambda d: ("Y" in d.note) == ("Z" in d.note) and "X" in d.note'
    )
    document = _doc(note="X\nY\nZ")
    oracle = samples.evaluate(definition, parse_sample(definition, document, "t"))

    cut = samples.cut(definition, document, oracle, "t")

    assert samples.evaluate(definition, parse_sample(definition, cut, "t")) == oracle
    assert cut == _doc(variant="", note="X")


def test_cut_refuses_an_input_that_does_not_yield_its_oracle(tree: Path) -> None:
    definition = _cut_definition(tree)
    oracle = samples.evaluate(definition, parse_sample(definition, _doc(variant="low"), "t"))

    with pytest.raises(SampleRefused, match="does not yield the results it is cut to"):
        samples.cut(definition, _doc(), oracle, "t")


def test_anonymize_rewrites_a_workspace_root_outside_the_home_directory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    path = f"{tmp_path}/state/agentspec/probe-workspaces/pkg/2026-11-01T000000-assertion/a"
    encoded = path.replace("/", "-").replace(".", "-")

    assert anonymize(path, IDENTITY) == (
        "/home/probe/.local/state/agentspec/probe-workspaces/pkg/2026-11-01T000000-assertion/a"
    )
    assert anonymize(encoded, IDENTITY) == (
        "-home-probe--local-state-agentspec-probe-workspaces-pkg-2026-11-01T000000-assertion-a"
    )


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


def test_identity_current_reads_home_and_user(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    monkeypatch.setenv("USER", "someone")
    monkeypatch.setenv("LOGNAME", "someone")

    assert Identity.current() == Identity(home=str(tmp_path / "home"), user="someone")


DEBUG_PROBE_PY = textwrap.dedent("""\
    from pydantic import MISSING
    from probe_harness.families.opencode_debug import OpenCodeDebugProbe
    from probe_harness.families.opencode_env import OpenCodeIsolation
    from probe_harness.models.raw import ProviderModel


    class Doc(ProviderModel):
        variant: str
        mode: str | MISSING = MISSING


    PROBE = OpenCodeDebugProbe(
        argv=("agent", "x"), output=Doc, isolation=OpenCodeIsolation(), assertion={assertion}
    )
""")


def _debug_package(
    tree: Path, *, assertion: str = "lambda d: d.variant", expected: JsonValue = "high"
) -> Path:
    package = tree / "debug-pkg"
    agent = package / "fixtures" / "assertion" / ".opencode" / "agent" / "probe.md"
    agent.parent.mkdir(parents=True)
    # The fields of the document OpenCode prints for this agent, which a sample keeps as keys.
    agent.write_text("---\nmode: primary\nvariant: high\n---\nprompt\n", encoding="utf-8")
    manifest: dict[str, Any] = {
        "schema_version": 2,
        "provider": "opencode",
        "driver": "unattended",
        "depth": "resolved-config",
        "question": "q",
        "version_source": {"kind": "none"},
        "assertion": {"expected": expected},
    }
    (package / "probe.json").write_text(json.dumps(manifest), encoding="utf-8")
    (package / "probe.py").write_text(DEBUG_PROBE_PY.format(assertion=assertion))
    return package


def _debug_ws(tmp_path: Path, document: dict[str, Any]) -> Path:
    ws = tmp_path / "ws"
    ws.mkdir(exist_ok=True)
    (ws / "view.json").write_text(json.dumps(document), encoding="utf-8")
    return ws


def test_write_sample_writes_a_cut_capture_with_the_results(tree: Path, tmp_path: Path) -> None:
    package = _debug_package(tree)
    ws = _debug_ws(tmp_path, {"variant": "high", "prompt": "/Users/tester/secret", "mode": "m"})
    definition, manifest = load_package(package)

    path = samples.write_sample(definition, manifest, ws, "confirming", identity=IDENTITY)

    sample = parse_sample_file(path)
    assert sample.origin == SampleOrigin.CAPTURE
    assert sample.input == {"variant": "high", "prompt": "", "mode": ""}
    assert sample.results.model_dump(mode="json") == {"assertion": "high"}


def test_write_sample_refuses_to_overwrite(tree: Path, tmp_path: Path) -> None:
    package = _debug_package(tree)
    ws = _debug_ws(tmp_path, {"variant": "high"})
    definition, manifest = load_package(package)
    samples.write_sample(definition, manifest, ws, "confirming", identity=IDENTITY)

    with pytest.raises(SampleRefused, match="are never overwritten"):
        samples.write_sample(definition, manifest, ws, "confirming", identity=IDENTITY)


def test_write_sample_refuses_a_result_anonymizing_changed(tree: Path, tmp_path: Path) -> None:
    # The assertion searches for the operator's home, which anonymizing rewrites.
    package = _debug_package(
        tree, assertion='lambda d: any("/Users/tester" in s for s in d.raw.strings())'
    )
    ws = _debug_ws(tmp_path, {"variant": "high", "prompt": "/Users/tester/x"})
    definition, manifest = load_package(package)

    with pytest.raises(SampleRefused) as exc:
        samples.write_sample(definition, manifest, ws, "confirming", identity=IDENTITY)
    assert exc.value.lines[1] == "  assertion: full true · anonymized false"
    assert not (package / "samples").exists()


def test_write_sample_refuses_an_unaccounted_string(tree: Path, tmp_path: Path) -> None:
    # The assertion reads `mode`, so the cut keeps it, and no authored file, catalog entry
    # or authored list accounts for its text.
    package = _debug_package(tree, assertion="lambda d: d.mode")
    ws = _debug_ws(tmp_path, {"variant": "high", "mode": "written by Tess"})
    definition, manifest = load_package(package)

    with pytest.raises(SampleRefused) as exc:
        samples.write_sample(definition, manifest, ws, "confirming", identity=IDENTITY)
    assert '  confirming.json: .input.mode: "written by Tess"' in exc.value.lines
    assert not (package / "samples").exists()


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
    # Neither expression needs the event's name to stay apart from "stop", so it is emptied.
    assert prefix.input == [{"hook_event_name": ""}]
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
    agent = package / "fixtures" / "assertion" / ".opencode" / "agent" / "agent.md"
    agent.parent.mkdir(parents=True)
    agent.write_text(f"you are {MARK}\n", encoding="utf-8")
    sink = tmp_path / "ws" / "one" / "sink"
    sink.mkdir(parents=True)
    governed = {"messages": [{"role": "system", "content": f"you are {MARK}"}], "stream": "y"}
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
                {"messages": [{"role": "system", "content": f"you are {MARK}"}], "stream": ""},
                # Ungoverned either way, so nothing in it is needed.
                {"messages": [{"role": "", "content": ""}]},
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
    assert again.returncode == 0, again.output
    # The sample was just written from this definition, so no result changed.
    assert again.stdout == ""


def _put_sample(package: Path, name: str, document: dict[str, Any]) -> Path:
    path = package / "samples" / f"{name}.json"
    path.parent.mkdir(exist_ok=True)
    path.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    return path


EDITED: dict[str, Any] = {
    "origin": "edited",
    "note": "variant changed by hand",
    "authored": ["low"],
    "input": {"variant": "low"},
    "results": {"assertion": "high"},
}


def test_regenerate_refuses_a_changed_result_and_writes_nothing(tree: Path) -> None:
    package = _debug_package(tree)
    path = _put_sample(package, "edited-x", EDITED)
    before = path.read_bytes()
    definition, manifest = load_package(package)

    with pytest.raises(SampleRefused) as exc:
        samples.regenerate(definition, manifest, accept_results=False)
    assert exc.value.lines[1] == "  edited-x.json: assertion: high → low"
    assert "--accept-results" in exc.value.lines[-1]
    assert path.read_bytes() == before


def test_a_refused_regenerate_leaves_a_sample_it_would_only_have_re_cut(tree: Path) -> None:
    package = _debug_package(tree)
    _put_sample(package, "edited-x", EDITED)
    uncut = _put_sample(
        package,
        "confirming",
        {
            "origin": "capture",
            "input": {"variant": "high", "mode": "m"},
            "results": {"assertion": "high"},
        },
    )
    before = uncut.read_bytes()
    definition, manifest = load_package(package)

    with pytest.raises(SampleRefused):
        samples.regenerate(definition, manifest, accept_results=False)
    assert uncut.read_bytes() == before


def test_regenerate_records_an_accepted_result(tree: Path) -> None:
    package = _debug_package(tree)
    path = _put_sample(package, "edited-x", EDITED)
    # Already right, and indented as no rewrite would leave it.
    unchanged = package / "samples" / "unchanged.json"
    right = {"origin": "capture", "input": {"variant": "high"}, "results": {"assertion": "high"}}
    unchanged.write_text(json.dumps(right, indent=4) + "\n", encoding="utf-8")
    before = unchanged.read_bytes()
    definition, manifest = load_package(package)

    changes = samples.regenerate(definition, manifest, accept_results=True)

    assert changes == ["edited-x.json: assertion: high → low"]
    assert json.loads(path.read_text()) == {**EDITED, "results": {"assertion": "low"}}
    assert unchanged.read_bytes() == before


def test_regenerate_re_cuts_an_uncut_input(tree: Path) -> None:
    package = _debug_package(tree)
    uncut = {
        "origin": "capture",
        "input": {"variant": "high", "mode": "first\nsecond", "prompt": "x"},
        "results": {"assertion": "high"},
    }
    path = _put_sample(package, "confirming", uncut)
    definition, manifest = load_package(package)

    changes = samples.regenerate(definition, manifest, accept_results=False)

    assert changes == ["confirming.json: cut 2 strings, 0 lines"]
    assert json.loads(path.read_text())["input"] == {"variant": "high", "mode": "", "prompt": ""}
    assert samples.regenerate(definition, manifest, accept_results=False) == []


def test_regenerate_refuses_an_unaccounted_string(tree: Path) -> None:
    # The assertion reads `mode`, so no cut can remove the operator's text from it.
    package = _debug_package(tree, assertion="lambda d: d.mode")
    uncut = _put_sample(
        package,
        "a-uncut",
        {
            "origin": "capture",
            "input": {"variant": "high", "mode": "m", "prompt": "x"},
            "results": {"assertion": "m"},
        },
    )
    _put_sample(
        package,
        "b-operator",
        {
            "origin": "capture",
            "input": {"variant": "", "mode": "my own notes"},
            "results": {"assertion": "my own notes"},
        },
    )
    before = uncut.read_bytes()
    definition, manifest = load_package(package)

    with pytest.raises(SampleRefused) as exc:
        samples.regenerate(definition, manifest, accept_results=False)
    assert '  b-operator.json: .input.mode: "my own notes"' in exc.value.lines
    assert '  b-operator.json: .results.assertion: "my own notes"' in exc.value.lines
    assert uncut.read_bytes() == before


def test_regenerate_refuses_an_authored_text_leak(tree: Path) -> None:
    package = _debug_package(tree)
    path = _put_sample(
        package, "edited-x", {**EDITED, "note": "variant changed by hand, ask me@example.net"}
    )
    before = path.read_bytes()
    definition, manifest = load_package(package)

    with pytest.raises(SampleRefused) as exc:
        samples.regenerate(definition, manifest, accept_results=True)
    assert "  edited-x.json: note: email: me@example.net" in exc.value.lines
    assert path.read_bytes() == before


def test_regenerate_refuses_a_cut_that_removes_an_authored_entry(tree: Path) -> None:
    # The assertion reads only `variant`, so the cut empties `mode`, whose text is the only
    # place the authored entry occurs.
    package = _debug_package(tree)
    path = _put_sample(
        package,
        "edited-x",
        {
            **EDITED,
            "authored": ["typed"],
            "input": {"variant": "high", "mode": "typed"},
            "results": {"assertion": "high"},
        },
    )
    before = path.read_bytes()
    definition, manifest = load_package(package)

    with pytest.raises(SampleRefused, match="would no longer be a sample once re-cut"):
        samples.regenerate(definition, manifest, accept_results=False)
    assert path.read_bytes() == before


def test_regenerate_leaves_every_sample_when_one_refuses(cli: Cli, tree: Path) -> None:
    package = _debug_package(tree)
    first = _put_sample(package, "a-edited", EDITED)
    before = first.read_bytes()
    # No `variant`, which the definition's output model requires.
    _put_sample(package, "b-unparsable", {"origin": "capture", "input": {}, "results": {}})
    definition, manifest = load_package(package)

    with pytest.raises(GateFailure):
        samples.regenerate(definition, manifest, accept_results=False)
    assert first.read_bytes() == before

    refused = cli("sample", "debug-pkg", "--regenerate")
    assert refused.returncode == 1, refused.output
    assert "b-unparsable.json" in refused.stderr
    assert first.read_bytes() == before


def test_regenerate_names_the_sample_an_expression_refused(tree: Path) -> None:
    package = _debug_package(tree, assertion="lambda d: d.mode")
    path = _put_sample(package, "edited-x", EDITED)
    definition, manifest = load_package(package)

    with pytest.raises(GateFailure) as exc:
        samples.regenerate(definition, manifest, accept_results=False)
    assert exc.value.lines[0].startswith(f"{path}: the assertion did not yield a JSON value")


def test_regenerate_leaves_an_unchanged_sample_untouched(tree: Path, tmp_path: Path) -> None:
    package = _debug_package(tree)
    definition, manifest = load_package(package)
    path = samples.write_sample(
        definition,
        manifest,
        _debug_ws(tmp_path, {"variant": "high"}),
        "confirming",
        identity=IDENTITY,
    )
    # Re-indented by hand, as a formatter would leave it.
    path.write_text(json.dumps(json.loads(path.read_text()), indent=4) + "\n", encoding="utf-8")
    before = path.read_bytes()

    assert samples.regenerate(definition, manifest, accept_results=False) == []
    assert path.read_bytes() == before


@pytest.mark.parametrize("name", ["../escape", "a/b", "Confirming", "", "-x"], ids=repr)
def test_a_sample_name_must_be_a_plain_slug(tree: Path, tmp_path: Path, name: str) -> None:
    package = _debug_package(tree)
    definition, manifest = load_package(package)
    with pytest.raises(SampleRefused, match="a sample name is"):
        samples.write_sample(
            definition, manifest, _debug_ws(tmp_path, {"variant": "high"}), name, identity=IDENTITY
        )


def test_a_lone_surrogate_survives_sampling_ascii_escaped(tree: Path, tmp_path: Path) -> None:
    # A body cut between the halves of an emoji carries an escaped lone surrogate, which
    # has no UTF-8 encoding. The assertion reads `mode`, so the cut keeps it.
    package = _debug_package(tree, assertion="lambda d: d.mode", expected="cut \ud83d")
    ws = tmp_path / "ws"
    ws.mkdir()
    (ws / "view.json").write_text('{"variant": "low", "mode": "cut \\ud83d"}', encoding="utf-8")
    definition, manifest = load_package(package)

    path = samples.write_sample(definition, manifest, ws, "refuting", identity=IDENTITY)

    assert "\\ud83d" in path.read_text(encoding="utf-8")
    assert parse_sample_file(path).input == {"variant": "", "mode": "cut \ud83d"}


def test_a_poll_counts_a_line_cut_inside_a_character_as_not_yet(tree: Path, tmp_path: Path) -> None:
    from probe_harness.families import manual

    package = _manual_package(tree)
    definition, _ = load_package(package)
    ws = _capture(tmp_path)
    payloads = ws / "capture" / "payloads.jsonl"
    # A hook mid-write: no newline yet, and the bytes end inside a character.
    with payloads.open("ab") as f:
        f.write('{"hook_event_name":"stop","x":"—'.encode()[:-1])
    assert isinstance(definition, manual.ManualProbe)

    assert manual._poll(ws, definition)() is False
