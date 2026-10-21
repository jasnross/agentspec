"""The harness's voice in `console.py`."""

import pytest
from probe_harness import console


@pytest.fixture(autouse=True)
def _restore_command() -> object:
    yield None
    console.set_command("probe")


def test_say_prefixes_each_line_with_the_command(capsys: pytest.CaptureFixture[str]) -> None:
    console.set_command("probe run")
    console.say("first", "  second")
    assert capsys.readouterr().err == "probe run: first\nprobe run:   second\n"


def test_say_before_a_command_is_named_speaks_as_probe(
    capsys: pytest.CaptureFixture[str],
) -> None:
    console.say("line")
    assert capsys.readouterr().err == "probe: line\n"


def test_verbatim_prints_each_line_unprefixed(capsys: pytest.CaptureFixture[str]) -> None:
    console.set_command("probe record")
    console.verbatim(["jq: error", "  at line 1"])
    assert capsys.readouterr().err == "jq: error\n  at line 1\n"
