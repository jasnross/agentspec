"""Gates: checks that decide whether a run measured what its package claims to.

A gate failure is a statement about the run, never about the provider, so it never
records: the runner prints the gate's lines, keeps the workspace, and exits 1.
"""

from pathlib import Path


class GateFailure(Exception):
    """A refusal. <lines> are the harness's own words; <detail> is output it passes through."""

    def __init__(self, *lines: str, detail: tuple[str, ...] = ()) -> None:
        super().__init__("\n".join(lines))
        self.lines = lines
        self.detail = detail


APPARATUS_FAILURE = "this is an apparatus failure, not a refutation; no record written."


def stamp(ws: Path, arm: str, key: str) -> None:
    """The arm's `<key>` MCP server answered `tools/list`, which writes `<arm>/<key>.stamp`.

    Without a connected server, an arm's tool set says nothing about MCP tools: an empty one
    would read as "the configuration hid the tool" when the tool never existed.
    """
    if not (ws / arm / f"{key}.stamp").exists():
        raise GateFailure(
            f"arm {arm}: the {key} server never answered tools/list, so the arm describes nothing.",
            APPARATUS_FAILURE,
        )
