"""The shared *Arrange* helpers in `workspace.py`.

Helpers that refuse raise `SystemExit`, so they are called in-process and the exit is
caught.
"""

import datetime
import io
import json
import subprocess
import threading
from pathlib import Path

import pytest
from probe_harness import jq_cli, workspace
from probe_harness.contract import Option
from probe_harness.workspace import (
    create_workspace,
    prompt_selection,
    read_selection,
    require_tools,
    template_file,
    wait_for_capture,
    workspace_root,
)

from tests.conftest import (
    REAL_ENVIRONMENT,
    make_capture_workspace,
)


class _FrozenClock(datetime.datetime):
    @classmethod
    def now(cls, tz: datetime.tzinfo | None = None) -> _FrozenClock:
        return cls(2026, 10, 21, 9, 30, 5, tzinfo=tz)


def test_create_workspace_makes_a_stamped_variant_directory_with_capture(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # `capture/` is the sibling of the project the operator opens.
    monkeypatch.setattr(workspace, "datetime", _FrozenClock)
    ws = create_workspace("unit-test", "assertion")
    assert ws == workspace_root() / "unit-test" / "2026-10-21T093005-assertion"
    assert (ws / "capture").is_dir()


def test_a_second_workspace_in_the_same_second_gets_a_suffix(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(workspace, "datetime", _FrozenClock)
    first = create_workspace("unit-test", "assertion")
    second = create_workspace("unit-test", "assertion")
    third = create_workspace("unit-test", "assertion")
    assert second == first.with_name(f"{first.name}-2")
    assert third == first.with_name(f"{first.name}-3")


def test_workspace_root_follows_an_absolute_xdg_state_home(tmp_path: Path) -> None:
    environ = {"XDG_STATE_HOME": str(tmp_path / "state"), "HOME": str(tmp_path / "home")}
    assert workspace_root(environ) == tmp_path / "state" / "agentspec" / "probe-workspaces"


@pytest.mark.parametrize("state", ["relative/state", ""], ids=["relative", "empty"])
def test_workspace_root_ignores_a_relative_or_empty_xdg_state_home(
    tmp_path: Path, state: str
) -> None:
    environ = {"XDG_STATE_HOME": state, "HOME": str(tmp_path)}
    assert workspace_root(environ) == (
        tmp_path / ".local" / "state" / "agentspec" / "probe-workspaces"
    )


_INSTRUCTION_FILES = ("CLAUDE.md", "AGENTS.md", ".claude", ".cursor")


def test_the_real_workspace_root_is_outside_every_repository() -> None:
    # A provider opened on a workspace inside a repository would load its instructions and
    # could search it for a marker.
    root = workspace_root(REAL_ENVIRONMENT)
    existing = root
    while not existing.exists():
        existing = existing.parent
    inside = subprocess.run(
        ["git", "rev-parse", "--is-inside-work-tree"],
        cwd=existing,
        capture_output=True,
        text=True,
        check=False,
    )
    assert inside.returncode != 0 or inside.stdout.strip() == "false", (
        f"the workspace root {root} lies inside a git work tree"
    )

    home = Path(REAL_ENVIRONMENT["HOME"])
    stop = home if root.is_relative_to(home) else Path("/")
    directory = root
    while directory != stop and directory != directory.parent:
        for name in _INSTRUCTION_FILES:
            assert not (directory / name).exists(), (
                f"{directory / name} is above the workspace root, where a provider would read it"
            )
        directory = directory.parent


def test_probe_require_tools_names_every_missing_tool(capsys: pytest.CaptureFixture[str]) -> None:
    with pytest.raises(SystemExit) as exc:
        require_tools("jq", "definitely-not-a-real-tool", "also-not-real")
    assert exc.value.code != 0
    # jq is present, so the rendered list must name the other two and only them.
    captured = capsys.readouterr()
    assert (
        "missing required tool(s): definitely-not-a-real-tool also-not-real"
        in captured.out + captured.err
    )


def test_probe_require_tools_succeeds_when_every_tool_is_present(
    capsys: pytest.CaptureFixture[str],
) -> None:
    require_tools("jq")
    captured = capsys.readouterr()
    assert captured.out + captured.err == ""


def test_probe_wait_for_capture_returns_0_once_a_matching_payload_is_appended(
    tmp_path: Path,
) -> None:
    ws = make_capture_workspace(tmp_path / "ws")

    def append() -> None:
        with (ws / "capture" / "payloads.jsonl").open("a", encoding="utf-8") as f:
            f.write('{"hook_event_name":"target"}\n')

    writer = threading.Timer(1, append)
    writer.start()
    try:
        matched = wait_for_capture(ws, 'any(.[]; .hook_event_name == "target")', 20)
    finally:
        writer.join()
    assert matched is True


def test_probe_wait_for_capture_returns_1_on_timeout_and_records_nothing(tmp_path: Path) -> None:
    ws = make_capture_workspace(tmp_path / "ws-timeout", '{"hook_event_name":"other"}\n')

    assert wait_for_capture(ws, 'any(.[]; .hook_event_name == "never")', 1) is False
    assert not (ws / "results").is_dir()


def test_probe_wait_for_capture_slurps_so_an_any_filter_matches_jsonl(tmp_path: Path) -> None:
    # Without slurping this filter never matches and the runner polls to timeout on a
    # capture that already holds the answer.
    ws = make_capture_workspace(tmp_path / "ws-slurp", '{"a":1}\n{"cursor_version":"3.16.17"}\n')

    assert wait_for_capture(ws, "any(.[]; .cursor_version != null)", 5) is True


def test_probe_template_file_substitutes_every_placeholder_and_keeps_json_valid(
    tmp_path: Path,
) -> None:
    src = tmp_path / "hooks.json.tmpl"
    src.write_text(
        '{"version":1,"hooks":{"subagentStart":[{"command":"{{CAPTURE_SCRIPT}}"}]}}\n',
        encoding="utf-8",
    )
    out = tmp_path / "out" / "hooks.json"

    template_file(src, out, CAPTURE_SCRIPT="/tmp/probe ws/dump-hook.sh")

    assert jq_cli.test(".", out)
    rendered = json.loads(out.read_text(encoding="utf-8"))
    assert rendered["hooks"]["subagentStart"][0]["command"] == "/tmp/probe ws/dump-hook.sh"


def test_probe_template_file_substitutes_several_placeholders_in_one_pass(tmp_path: Path) -> None:
    src = tmp_path / "multi.tmpl"
    src.write_text("a={{ONE}} b={{TWO}} a-again={{ONE}}\n", encoding="utf-8")
    out = tmp_path / "multi.out"

    template_file(src, out, ONE="first", TWO="second")

    assert out.read_text(encoding="utf-8").rstrip("\n") == "a=first b=second a-again=first"


def test_probe_template_file_fails_rather_than_emitting_an_unsubstituted_placeholder(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    # An unsubstituted path is exactly the silent-never-fires failure this helper exists to
    # prevent.
    src = tmp_path / "partial.tmpl"
    src.write_text("script={{CAPTURE_SCRIPT}} payloads={{PAYLOADS}}\n", encoding="utf-8")
    out = tmp_path / "partial.out"

    with pytest.raises(SystemExit) as exc:
        template_file(src, out, CAPTURE_SCRIPT="/tmp/x")
    assert exc.value.code != 0
    captured = capsys.readouterr()
    assert "unsubstituted placeholder" in captured.out + captured.err
    assert "{{PAYLOADS}}" in captured.out + captured.err
    assert not out.is_file()


def test_probe_template_file_substitutes_a_value_containing_an_ampersand_literally(
    tmp_path: Path,
) -> None:
    # bash >= 5.2 expanded `&` in a `${x//a/b}` replacement to the matched text while 3.2
    # did not; the port must keep substituting it literally.
    src = tmp_path / "amp.tmpl"
    src.write_text("path={{P}}\n", encoding="utf-8")
    out = tmp_path / "amp.out"

    template_file(src, out, P="/tmp/a&b/dump.sh")

    assert out.read_text(encoding="utf-8").rstrip("\n") == "path=/tmp/a&b/dump.sh"


def test_probe_template_file_substitutes_a_value_containing_shell_and_glob_metacharacters(
    tmp_path: Path,
) -> None:
    src = tmp_path / "meta.tmpl"
    src.write_text("path={{P}}\n", encoding="utf-8")
    out = tmp_path / "meta.out"

    template_file(src, out, P="/tmp/a$b*c?d[e]/dump.sh")

    assert out.read_text(encoding="utf-8").rstrip("\n") == "path=/tmp/a$b*c?d[e]/dump.sh"


def test_probe_template_file_rejects_a_key_containing_metacharacters(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    # A key becomes part of a pattern, so `P*` would match every placeholder beginning
    # with P.
    src = tmp_path / "badkey.tmpl"
    src.write_text("a={{PX}} b={{P}}\n", encoding="utf-8")

    with pytest.raises(SystemExit) as exc:
        template_file(src, tmp_path / "badkey.out", **{"P*": "Z"})
    assert exc.value.code != 0
    captured = capsys.readouterr()
    assert "invalid template key" in captured.out + captured.err


def test_probe_template_file_rejects_an_empty_key(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    src = tmp_path / "emptykey.tmpl"
    src.write_text("a={{K}}\n", encoding="utf-8")

    with pytest.raises(SystemExit) as exc:
        template_file(src, tmp_path / "emptykey.out", **{"": "value"})
    assert exc.value.code != 0
    captured = capsys.readouterr()
    assert "invalid template key" in captured.out + captured.err


def test_probe_template_file_fails_on_a_missing_source(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    with pytest.raises(SystemExit) as exc:
        template_file(tmp_path / "does-not-exist", tmp_path / "out", KEY="value")
    assert exc.value.code != 0
    captured = capsys.readouterr()
    assert "template source not found" in captured.out + captured.err


SELECTION_OPTIONS = (
    Option(id="neither", text="Neither marker"),
    Option(id="couldnt-tell", text="Could not determine", status="inconclusive"),
)


def test_probe_prompt_selection_exits_2_without_blocking_when_stdin_is_not_a_tty(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    # The guard that keeps a human-judged probe from hanging a batch run forever.
    monkeypatch.setattr("sys.stdin", io.StringIO(""))

    with pytest.raises(SystemExit) as exc:
        prompt_selection(
            (Option(id="a", text="first"), Option(id="couldnt-tell", text="Could not determine")),
            "Which one?",
        )
    assert exc.value.code == 2
    captured = capsys.readouterr()
    output = captured.out + captured.err
    assert "stdin is not a terminal" in output
    assert "Which one?" in output
    assert "couldnt-tell" in output


def test_the_selection_loop_re_prompts_on_an_unrecognized_answer_then_returns_the_id(
    capsys: pytest.CaptureFixture[str],
) -> None:
    # Drives `read_selection` rather than `prompt_selection`: the latter's tty guard fires
    # before the loop on any non-tty stdin.
    answer = read_selection(SELECTION_OPTIONS, "Which markers?", io.StringIO("nonsense\nneither\n"))

    captured = capsys.readouterr()
    assert '"nonsense" is not one of the option ids' in captured.out + captured.err
    assert answer == "neither"


def test_the_selection_loop_accepts_a_known_id_on_a_last_line_with_no_newline() -> None:
    assert read_selection(SELECTION_OPTIONS, "Which markers?", io.StringIO("neither")) == "neither"


def test_the_selection_loop_exits_2_on_an_empty_read() -> None:
    with pytest.raises(SystemExit) as exc:
        read_selection(SELECTION_OPTIONS, "Which markers?", io.StringIO("nonsense\n"))
    assert exc.value.code == 2
