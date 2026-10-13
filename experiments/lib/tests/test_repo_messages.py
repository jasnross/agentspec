"""No harness module or package definition spells a message prefix itself.

Every diagnostic line goes out through `console.say`, which prefixes it with the subcommand
the operator ran. A literal that starts with a prefix of its own would print it twice, or
speak for a subcommand that did not run.
"""

import ast
from collections.abc import Iterator
from pathlib import Path

import pytest

from tests.conftest import EXPERIMENTS_DIR

LEGACY_PREFIXES = ("probe:", "record:", "probe-run:", "probe-status:")


def _sources() -> list[Path]:
    harness = sorted((EXPERIMENTS_DIR / "lib" / "probe_harness").rglob("*.py"))
    return harness + sorted(EXPERIMENTS_DIR.glob("*/probe.py"))


@pytest.mark.parametrize("source", _sources(), ids=lambda p: str(p.relative_to(EXPERIMENTS_DIR)))
def test_no_string_literal_starts_with_a_message_prefix(source: Path) -> None:
    offending = [
        f"{source.name}:{node.lineno}: {text!r}"
        for node, text in _leading_strings(ast.parse(source.read_text(encoding="utf-8")))
        if text.startswith(LEGACY_PREFIXES)
    ]
    assert not offending, "\n".join(offending)


def _leading_strings(tree: ast.Module) -> Iterator[tuple[ast.expr, str]]:
    """Every string constant, and the literal text an f-string starts with."""
    for node in ast.walk(tree):
        match node:
            case ast.Constant(value=str() as text):
                yield node, text
            case ast.JoinedStr(values=[ast.Constant(value=str() as text), *_]):
                yield node, text
            case _:
                pass
