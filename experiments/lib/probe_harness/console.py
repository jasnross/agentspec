"""The harness's voice: every diagnostic line it writes names the subcommand that wrote it.

`cli.main` names the subcommand once with `set_command`, and every refusal, warning and
progress line goes out through `say` as `<command>: <line>`. Output the harness did not
write — a provider's stderr, jq's diagnostics — goes out through `verbatim`, unprefixed.
"""

import sys
from collections.abc import Sequence
from typing import Protocol

_command = "probe"


class Refusal(Protocol):
    """A refusal: the harness's own `lines`, and the output it passes through as `detail`."""

    @property
    def lines(self) -> tuple[str, ...]: ...

    @property
    def detail(self) -> tuple[str, ...]: ...


def set_command(name: str) -> None:
    """Name the subcommand every later `say` line speaks for, such as `probe run`."""
    global _command
    _command = name


def say(*lines: str) -> None:
    """Each line to stderr as `<command>: <line>`."""
    for line in lines:
        print(f"{_command}: {line}", file=sys.stderr)


def verbatim(lines: Sequence[str]) -> None:
    """Each line to stderr as it is: output the harness passes through but did not write."""
    for line in lines:
        print(line, file=sys.stderr)


def say_refusal(refusal: Refusal) -> None:
    """A refusal's own lines through `say`, then its pass-through output through `verbatim`."""
    say(*refusal.lines)
    verbatim(refusal.detail)
