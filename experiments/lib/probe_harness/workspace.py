"""Shared Arrange helpers: workspaces, tools, templating, capture polling, and prompting."""

import os
import re
import shutil
import sys
import time
from collections.abc import Mapping, Sequence
from datetime import UTC, datetime
from pathlib import Path
from typing import TextIO

from probe_harness import console, jq_cli
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


def workspace_root(environ: Mapping[str, str] = os.environ) -> Path:
    """The directory every run's workspace lives under.

    `$XDG_STATE_HOME/agentspec/probe-workspaces`, or `~/.local/state/…` when that is unset
    or relative. It sits outside every repository on purpose: a provider opened on a
    workspace inside one would load the repository's instructions, and could search the
    repository for a marker.
    """
    state = environ.get("XDG_STATE_HOME", "")
    if state and Path(state).is_absolute():
        base = Path(state)
    else:
        home = Path(environ["HOME"]) if environ.get("HOME") else Path.home()
        base = home / ".local" / "state"
    return base / "agentspec" / "probe-workspaces"


def create_workspace(name: str, variant: str) -> Path:
    """A new workspace for one run of <name>'s <variant>, carrying a `capture/` directory.

    Named for the run's UTC start time and variant. The harness never deletes it.
    """
    package_dir = workspace_root() / name
    package_dir.mkdir(parents=True, exist_ok=True)
    stem = f"{datetime.now(UTC).strftime('%Y-%m-%dT%H%M%S')}-{variant}"
    ws = package_dir / stem
    suffix = 1
    while True:
        try:
            ws.mkdir()
            break
        except FileExistsError:
            suffix += 1
            ws = package_dir / f"{stem}-{suffix}"
    (ws / "capture").mkdir()
    return ws


def require_tools(*tools: str) -> None:
    """Exit 1 naming every missing tool, so a contributor gets one clear error."""
    missing = [tool for tool in tools if shutil.which(tool) is None]
    if missing:
        console.say(f"missing required tool(s): {' '.join(missing)}")
        raise SystemExit(1)


def template_file(src: Path, dst: Path, **pairs: str) -> None:
    """Copy <src> to <dst>, substituting `{{KEY}}` placeholders from <pairs>.

    This removes the hand-edited absolute path from a probe's setup: an operator asked to
    substitute a placeholder path by hand eventually forgets, and the result is a hook
    that silently never fires. A leftover placeholder is exactly that failure, so it is
    refused rather than written. Trailing newlines collapse to one.
    """
    if not src.is_file():
        console.say(f"template source not found: {src}")
        raise SystemExit(1)

    content = src.read_text(encoding="utf-8").rstrip("\n")
    for key, value in pairs.items():
        # The key becomes part of a pattern, so a metacharacter in it would match far
        # more than intended, and an empty key would substitute every `{{}}`.
        if not _TEMPLATE_KEY.fullmatch(key):
            console.say(f"invalid template key {key or '<empty>'} (want [A-Za-z0-9_]+)")
            raise SystemExit(1)
        content = content.replace("{{" + key + "}}", value)

    if "{{" in content:
        tokens = "".join(f"{t} " for t in sorted(set(_PLACEHOLDER.findall(content))))
        console.say(f"unsubstituted placeholder left in {dst}: {tokens}")
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
            console.say(f"waiting for capture ({waited}s elapsed, timeout {timeout}s)")
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
        console.say("this probe requires a human selection, but stdin is not a terminal.")
        if question:
            console.say(question)
        sys.stderr.write(_render_options(options))
        console.say("run this probe directly from an interactive terminal.")
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
        console.say(f'"{answer}" is not one of the option ids.')


def _capture_matches(payloads: Path, filter_expr: str) -> bool:
    try:
        if payloads.stat().st_size == 0:
            return False
    except OSError:
        return False
    return jq_cli.test(filter_expr, payloads, slurp=True)


def _render_options(options: Sequence[Option]) -> str:
    return "".join(f"  {option.id}  —  {option.text}\n" for option in options)
