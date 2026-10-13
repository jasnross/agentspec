"""The jq boundary's own behavior: compile checks."""

from probe_harness import jq_cli


def test_compile_error_returns_jq_diagnostic_for_a_broken_expression() -> None:
    error = jq_cli.compile_error(".[")
    assert isinstance(error, str)
    assert error


def test_compile_error_returns_none_for_a_valid_expression() -> None:
    assert jq_cli.compile_error(".") is None


def test_compile_check_accepts_an_expression_ending_in_a_comment() -> None:
    assert jq_cli.compile_error(". # trailing comment") is None
