"""The top-level argument parser in `cli.py`."""

from tests.conftest import Cli


def test_no_subcommand_exits_2(cli: Cli) -> None:
    result = cli()
    assert result.returncode == 2
    assert "probe: error: the following arguments are required: command" in result.stderr


def test_an_unknown_subcommand_exits_2(cli: Cli) -> None:
    result = cli("bogus")
    assert result.returncode == 2
    assert "probe: error: argument command: invalid choice: 'bogus'" in result.stderr


def test_an_abbreviated_subcommand_flag_is_refused(cli: Cli) -> None:
    result = cli("status", "--sum")
    assert result.returncode == 2
    assert "probe status: error: unrecognized arguments: --sum" in result.stderr
