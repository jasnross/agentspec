"""Shared Arrange helpers: workspaces, tools, templating, capture polling, and prompting."""

import os
import re
import shutil
import sys
import tempfile
import time
from collections.abc import Sequence
from pathlib import Path
from typing import TextIO

from probe_harness import jq_cli
from probe_harness.contract import Option

# How long a human-driven probe waits for its capture, in seconds. It is generous because
# its job is diagnostic: an expiry means the hook never fired, which is worth an hour of an
# operator's patience to tell apart from a slow session. `PROBE_TIMEOUT_SECONDS` overrides
# it.
DEFAULT_TIMEOUT_SECONDS = 3600

POLL_INTERVAL_SECONDS = 2
PROGRESS_INTERVAL_SECONDS = 10

_TEMPLATE_KEY = re.compile(r"[A-Za-z0-9_]+")
_PLACEHOLDER = re.compile(r"\{\{[^}]*\}\}")


def create_workspace(name: str) -> Path:
    """A throwaway workspace for one run, always carrying a `capture/` directory."""
    ws = Path(tempfile.mkdtemp(prefix=f"probe-{name}.", dir=os.environ.get("TMPDIR") or "/tmp"))
    (ws / "capture").mkdir()
    return ws


def require_tools(*tools: str) -> None:
    """Exit 1 naming every missing tool, so a contributor gets one clear error."""
    missing = [tool for tool in tools if shutil.which(tool) is None]
    if missing:
        print(f"probe: missing required tool(s): {' '.join(missing)}", file=sys.stderr)
        raise SystemExit(1)


def template_file(src: Path, dst: Path, **pairs: str) -> None:
    """Copy <src> to <dst>, substituting `{{KEY}}` placeholders from <pairs>.

    This removes the hand-edited absolute path from a probe's setup: an operator asked to
    substitute a placeholder path by hand eventually forgets, and the result is a hook
    that silently never fires. A leftover placeholder is exactly that failure, so it is
    refused rather than written. Trailing newlines collapse to one.
    """
    if not src.is_file():
        print(f"probe: template source not found: {src}", file=sys.stderr)
        raise SystemExit(1)

    content = src.read_text(encoding="utf-8").rstrip("\n")
    for key, value in pairs.items():
        # The key becomes part of a pattern, so a metacharacter in it would match far
        # more than intended, and an empty key would substitute every `{{}}`.
        if not _TEMPLATE_KEY.fullmatch(key):
            print(
                f"probe: invalid template key {key or '<empty>'} (want [A-Za-z0-9_]+)",
                file=sys.stderr,
            )
            raise SystemExit(1)
        content = content.replace("{{" + key + "}}", value)

    if "{{" in content:
        tokens = "".join(f"{t} " for t in sorted(set(_PLACEHOLDER.findall(content))))
        print(f"probe: unsubstituted placeholder left in {dst}: {tokens}", file=sys.stderr)
        raise SystemExit(1)

    dst.parent.mkdir(parents=True, exist_ok=True)
    dst.write_text(content + "\n", encoding="utf-8")


def wait_for_capture(ws: Path, filter_expr: str, timeout: int) -> bool:
    """Poll `<ws>/capture/payloads.jsonl` until <filter_expr> holds against it.

    The filter is evaluated over the slurped payloads, so every jq expression in the
    contract sees the same array-shaped input. Returns whether it matched before the
    timeout, recording nothing either way.
    """
    payloads = ws / "capture" / "payloads.jsonl"
    waited = 0
    last_progress = 0
    while waited < timeout:
        if _capture_matches(payloads, filter_expr):
            return True
        time.sleep(POLL_INTERVAL_SECONDS)
        waited += POLL_INTERVAL_SECONDS
        if waited - last_progress >= PROGRESS_INTERVAL_SECONDS:
            print(
                f"probe: waiting for capture ({waited}s elapsed, timeout {timeout}s)",
                file=sys.stderr,
            )
            last_progress = waited
    # One final check past the deadline: a capture that landed during the last sleep is a
    # match, not a timeout.
    return _capture_matches(payloads, filter_expr)


def prompt_selection(options: Sequence[Option], question: str) -> str:
    """Present a pre-declared option set and return the selected id.

    Without the tty guard, a human-judged probe swept up by a batch run would block
    forever on a read that can never be answered.
    """
    if not sys.stdin.isatty():
        print(
            "probe: this probe requires a human selection, but stdin is not a terminal.",
            file=sys.stderr,
        )
        if question:
            print(f"probe: {question}", file=sys.stderr)
        sys.stderr.write(_render_options(options))
        print("probe: run this probe directly from an interactive terminal.", file=sys.stderr)
        raise SystemExit(2)
    return read_selection(options, question, sys.stdin)


def read_selection(options: Sequence[Option], question: str, stream: TextIO) -> str:
    """The prompt loop, re-prompting on an unrecognized id. End of input exits 2."""
    ids = {option.id for option in options}
    while True:
        sys.stderr.write("\n")
        if question:
            sys.stderr.write(f"{question}\n\n")
        sys.stderr.write(_render_options(options))
        sys.stderr.write("\nSelection (id): ")
        sys.stderr.flush()
        line = stream.readline()
        if not line:
            raise SystemExit(2)
        answer = line.rstrip("\n").strip(" \t")
        if answer in ids:
            return answer
        print(f'probe: "{answer}" is not one of the option ids.', file=sys.stderr)


def _capture_matches(payloads: Path, filter_expr: str) -> bool:
    try:
        if payloads.stat().st_size == 0:
            return False
    except OSError:
        return False
    return jq_cli.test(filter_expr, payloads, slurp=True)


def _render_options(options: Sequence[Option]) -> str:
    return "".join(f"  {option.id}  —  {option.text}\n" for option in options)
